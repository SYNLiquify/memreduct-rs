//! Mem Reduct (Rust) — real-time memory monitor and cleaner for Windows.
//!
//! Rust rewrite by Liquify: https://github.com/SYNLiquify/memreduct-rs
//! Based on Mem Reduct by Henry++ (original author), https://github.com/henrypp/memreduct
//! Licensed under GPL-3.0-or-later.
//!
//! Command line:
//!   -clean          clean the configured/default regions and exit
//!   -clean:full     clean every region and exit
//!   -silent         with -clean: no result message box
//!   -minimized      start hidden in the tray
//!   -help, /?       show the options

#![windows_subsystem = "windows"]

mod config;
mod dlg;
mod icon;
mod mainwnd;
mod memory;
mod settings;
mod sys;
mod theme;
mod tray;
mod util;
mod win;

use std::cell::Cell;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Controls::{InitCommonControlsEx, ICC_STANDARD_CLASSES, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_F1, VK_F2, VK_F4};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::sys::APP_NAME;
use crate::util::wide;

const MUTEX_NAME: &str = "memreduct-rs-single-instance";

thread_local! {
    static INSTANCE_MUTEX: Cell<HANDLE> = const { Cell::new(std::ptr::null_mut()) };
}

/// Returns false when another instance already owns the mutex.
pub fn acquire_instance_mutex() -> bool {
    let name = wide(MUTEX_NAME);
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        let exists = GetLastError() == ERROR_ALREADY_EXISTS;
        if exists {
            if !h.is_null() {
                CloseHandle(h);
            }
            return false;
        }
        INSTANCE_MUTEX.with(|m| m.set(h));
        true
    }
}

pub fn release_instance_mutex() {
    INSTANCE_MUTEX.with(|m| {
        let h = m.replace(std::ptr::null_mut());
        if !h.is_null() {
            unsafe { CloseHandle(h) };
        }
    });
}

#[derive(Default, Debug, PartialEq)]
struct Args {
    clean: Option<u32>,
    silent: bool,
    minimized: bool,
    help: bool,
}

fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Args {
    let mut out = Args::default();
    for raw in args {
        let arg = raw.trim_start_matches(['-', '/']).to_ascii_lowercase();
        let (name, value) = match arg.split_once([':', '=']) {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        match name.as_str() {
            "clean" => {
                out.clean = Some(if value.as_deref() == Some("full") {
                    memory::MASK_ALL
                } else {
                    config::get_u32("ReductMask2", memory::MASK_DEFAULT)
                })
            }
            "silent" | "quiet" => out.silent = true,
            "minimized" | "minimize" | "tray" => out.minimized = true,
            "help" | "?" | "h" => out.help = true,
            _ => {}
        }
    }
    out
}

fn command_line_clean(mask: u32, silent: bool) -> i32 {
    if !sys::is_elevated() {
        // relaunch elevated with the same arguments
        let args: Vec<String> = std::env::args().skip(1).collect();
        if sys::run_as_admin(std::ptr::null_mut(), &args.join(" ")) {
            return 0;
        }
        if !silent {
            win::message_box(
                std::ptr::null_mut(),
                "Administrator rights are required to clean memory. Please run the program as administrator.",
                APP_NAME,
                MB_OK | MB_ICONSTOP,
            );
        }
        return 1;
    }

    sys::enable_cleaning_privileges();
    let result = memory::clean(mask);
    config::set_i64("StatisticLastReduct", util::unix_now());
    for e in &result.errors {
        util::log(util::LogLevel::Error, e);
    }
    let freed = util::format_bytes(result.freed);
    if config::get_bool("LogCleanResults", false) {
        util::log(util::LogLevel::Info, &format!("Cleanup (Command-line): {} released", freed));
    }
    if !silent {
        let text = format!(
            "Memory was released with {} result.\r\n\r\nMemory regions cleaned:\r\n{}",
            freed,
            memory::describe_mask(mask)
        );
        win::message_box(std::ptr::null_mut(), &text, APP_NAME, MB_OK | MB_ICONINFORMATION);
    }
    if result.errors.is_empty() {
        0
    } else {
        2
    }
}

unsafe extern "system" fn find_instance(hwnd: HWND, out: LPARAM) -> i32 {
    let prop = mainwnd::instance_prop();
    if !GetPropW(hwnd, prop.as_ptr()).is_null() {
        *(out as *mut HWND) = hwnd;
        return 0;
    }
    1
}

fn activate_existing_instance() {
    let mut found: HWND = std::ptr::null_mut();
    unsafe {
        EnumWindows(Some(find_instance), &mut found as *mut HWND as LPARAM);
        if !found.is_null() {
            PostMessageW(found, mainwnd::activate_message(), 0, 0);
        }
    }
}

fn accelerators() -> HACCEL {
    let accels = [
        ACCEL { fVirt: FVIRTKEY, key: VK_F1, cmd: mainwnd::IDM_ABOUT as u16 },
        ACCEL { fVirt: FVIRTKEY, key: VK_F2, cmd: mainwnd::IDM_SETTINGS as u16 },
        ACCEL { fVirt: (FVIRTKEY | FALT), key: VK_F4, cmd: mainwnd::IDM_TRAY_SHOW as u16 },
    ];
    unsafe { CreateAcceleratorTableW(accels.as_ptr(), accels.len() as i32) }
}

fn run() -> i32 {
    config::init();
    let mut args = parse_args(std::env::args().skip(1));

    if args.help {
        win::message_box(
            std::ptr::null_mut(),
            "Available options for memreduct.exe:\r\n\r\n\
             -clean\t\tclean the configured memory regions\r\n\
             -clean:full\tclean all memory regions\r\n\
             -silent\t\tdo not show the result of -clean\r\n\
             -minimized\tstart hidden in the notification area",
            APP_NAME,
            MB_OK | MB_ICONINFORMATION,
        );
        return 0;
    }

    if let Some(mask) = args.clean {
        return command_line_clean(mask, args.silent);
    }

    if !acquire_instance_mutex() {
        activate_existing_instance();
        return 0;
    }

    if sys::is_elevated() {
        sys::enable_cleaning_privileges();
        // the task may have been removed outside the app (Task Scheduler)
        if config::get_bool("SkipUacEnabled", false) {
            std::thread::spawn(|| {
                if !sys::skipuac_is_enabled() {
                    config::set_bool("SkipUacEnabled", false);
                }
            });
        }
        // arguments handed over by a non-elevated instance through "skip UAC"
        if config::get_bool("SkipUacPendingMinimized", false) {
            args.minimized = true;
            config::remove("SkipUacPendingMinimized");
        }
    } else if config::get_bool("SkipUacEnabled", false) {
        // restart elevated through the scheduled task, without a UAC prompt
        release_instance_mutex();
        if args.minimized {
            config::set_bool("SkipUacPendingMinimized", true);
        }
        if sys::skipuac_run() {
            return 0;
        }
        config::remove("SkipUacPendingMinimized");
        if !acquire_instance_mutex() {
            activate_existing_instance();
            return 0;
        }
    }

    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_WIN95_CLASSES,
        };
        InitCommonControlsEx(&icc);
    }

    theme::reload();

    let hwnd = mainwnd::create();
    if hwnd.is_null() {
        win::message_box(std::ptr::null_mut(), "Could not create the main window.", APP_NAME, MB_OK | MB_ICONSTOP);
        return 1;
    }

    if !(args.minimized || config::get_bool("IsStartMinimized", false)) {
        mainwnd::show(hwnd);
    }

    let accel = accelerators();
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            let root = GetAncestor(msg.hwnd, GA_ROOT);
            if root == mainwnd::main_hwnd() && TranslateAcceleratorW(root, accel, &msg) != 0 {
                continue;
            }
            if !root.is_null() && IsDialogMessageW(root, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        DestroyAcceleratorTable(accel);
    }

    release_instance_mutex();
    0
}

fn main() {
    std::process::exit(run());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_options() {
        assert_eq!(args(&["-clean:full"]).clean, Some(memory::MASK_ALL));
        assert_eq!(args(&["/CLEAN=full", "-silent"]).clean, Some(memory::MASK_ALL));
        assert!(args(&["-clean", "-silent"]).silent);
        assert!(args(&["-clean"]).clean.is_some());
        assert!(args(&["-minimized"]).minimized);
        assert!(args(&["/?"]).help);
        assert_eq!(args(&["-unknown"]), Args::default());
    }
}
