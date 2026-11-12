//! OS-level helpers: elevation, privileges, version checks, autorun,
//! the "skip UAC" scheduled task, and launching processes/URLs.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, TokenElevation,
    LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, ShellExecuteW, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::util::{from_wide, wide};

pub const APP_NAME: &str = "Mem Reduct";
pub const APP_NAME_SHORT: &str = "memreduct-rs";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO_URL: &str = "https://github.com/SYNLiquify/memreduct-rs";
pub const APP_AUTHOR: &str = "Liquify";
pub const ORIGINAL_AUTHOR: &str = "Henry++";

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const SKIPUAC_TASK: &str = "memreduct-rs_skipuac";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------------------------------------------------------------------
// Elevation and privileges
// ---------------------------------------------------------------------------

pub fn is_elevated() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    })
}

fn enable_privilege(token: HANDLE, name: &str) -> bool {
    unsafe {
        let mut luid: LUID = std::mem::zeroed();
        let wname = wide(name);
        if LookupPrivilegeValueW(std::ptr::null(), wname.as_ptr(), &mut luid) == 0 {
            return false;
        }
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        AdjustTokenPrivileges(token, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut()) != 0
    }
}

/// Enables the privileges `NtSetSystemInformation` checks for memory lists
/// and the file cache working set.
pub fn enable_cleaning_privileges() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let a = enable_privilege(token, "SeProfileSingleProcessPrivilege");
        let b = enable_privilege(token, "SeIncreaseQuotaPrivilege");
        CloseHandle(token);
        a && b
    }
}

// ---------------------------------------------------------------------------
// OS version
// ---------------------------------------------------------------------------

#[repr(C)]
struct OsVersionInfoW {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
}

#[link(name = "ntdll", kind = "raw-dylib")]
extern "system" {
    fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
}

/// (major, minor, build), unaffected by compatibility shims.
pub fn os_version() -> (u32, u32, u32) {
    static CACHE: OnceLock<(u32, u32, u32)> = OnceLock::new();
    *CACHE.get_or_init(|| unsafe {
        let mut info: OsVersionInfoW = std::mem::zeroed();
        info.size = std::mem::size_of::<OsVersionInfoW>() as u32;
        if RtlGetVersion(&mut info) >= 0 {
            (info.major, info.minor, info.build)
        } else {
            (6, 1, 0)
        }
    })
}

pub fn is_win81_or_later() -> bool {
    let (major, minor, _) = os_version();
    major > 6 || (major == 6 && minor >= 3)
}

pub fn is_win10_or_later() -> bool {
    os_version().0 >= 10
}

/// Windows 11 starts at build 22000 (caption color support etc.).
pub fn is_win11_or_later() -> bool {
    let (major, _, build) = os_version();
    major >= 10 && build >= 22000
}

// ---------------------------------------------------------------------------
// Processes and shell
// ---------------------------------------------------------------------------

pub fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Relaunches this executable elevated ("runas"). Returns `true` when the
/// new process started (the user accepted the UAC prompt).
pub fn run_as_admin(hwnd: HWND, args: &str) -> bool {
    let verb = wide("runas");
    let file = wide(&exe_path());
    let params = wide(args);
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.hwnd = hwnd;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        ShellExecuteExW(&mut info) != 0
    }
}

/// Opens a URL, folder or program through the shell.
pub fn shell_open(target: &str) {
    let verb = wide("open");
    let file = wide(target);
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

// ---------------------------------------------------------------------------
// Load on startup (HKCU\...\Run)
// ---------------------------------------------------------------------------

fn autorun_command() -> String {
    format!("\"{}\" -minimized", exe_path())
}

pub fn autorun_is_enabled() -> bool {
    let key = wide(RUN_KEY);
    let name = wide(APP_NAME_SHORT);
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut size,
        )
    };
    if status != 0 {
        return false;
    }
    let value = from_wide(&buf).to_lowercase();
    value.contains(&exe_path().to_lowercase())
}

pub fn autorun_enable(enable: bool) -> bool {
    let key = wide(RUN_KEY);
    let name = wide(APP_NAME_SHORT);
    unsafe {
        if !enable {
            let status = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            return status == 0 || status == 2; // ERROR_FILE_NOT_FOUND is fine
        }
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        ) != 0
        {
            return false;
        }
        let value = wide(&autorun_command());
        let status = RegSetValueExW(
            hkey,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr() as *const u8,
            (value.len() * 2) as u32,
        );
        RegCloseKey(hkey);
        status == 0
    }
}

// ---------------------------------------------------------------------------
// Skip UAC prompt: a scheduled task that runs this exe with highest privileges
// ---------------------------------------------------------------------------

fn schtasks(args: &[&str]) -> Option<std::process::Output> {
    Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// True when the task exists and points at this executable.
pub fn skipuac_is_enabled() -> bool {
    match schtasks(&["/Query", "/TN", SKIPUAC_TASK, "/XML"]) {
        Some(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
            text.contains(&xml_escape(&exe_path()).to_lowercase())
        }
        _ => false,
    }
}

/// Creating the task needs an elevated process.
pub fn skipuac_enable(enable: bool) -> bool {
    if !enable {
        return match schtasks(&["/Delete", "/TN", SKIPUAC_TASK, "/F"]) {
            Some(out) => out.status.success(),
            None => false,
        };
    }

    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts {name} with administrator rights without a UAC prompt.</Description>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>false</AllowHardTerminate>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{cmd}</Command>
    </Exec>
  </Actions>
</Task>
"#,
        name = xml_escape(APP_NAME),
        cmd = xml_escape(&exe_path()),
    );

    // schtasks wants UTF-16 LE with BOM for /XML
    let mut bytes = vec![0xFFu8, 0xFE];
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let path = std::env::temp_dir().join("memreduct-rs-task.xml");
    if std::fs::write(&path, bytes).is_err() {
        return false;
    }
    let path_s = path.to_string_lossy().into_owned();
    let ok = matches!(
        schtasks(&["/Create", "/TN", SKIPUAC_TASK, "/XML", &path_s, "/F"]),
        Some(out) if out.status.success()
    );
    let _ = std::fs::remove_file(&path);
    ok
}

/// Starts the elevated task. Returns `true` on success.
pub fn skipuac_run() -> bool {
    matches!(schtasks(&["/Run", "/TN", SKIPUAC_TASK]), Some(out) if out.status.success())
}
