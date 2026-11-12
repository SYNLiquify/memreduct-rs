//! Notification-area icon wrapper (NOTIFYICON_VERSION_4).

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::HICON;

use crate::util::copy_to_buf;

pub const TRAY_ID: u32 = 1;

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    nid
}

pub fn add(hwnd: HWND, callback_msg: u32, icon: HICON, tip: &str) -> bool {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = callback_msg;
    nid.hIcon = icon;
    copy_to_buf(&mut nid.szTip, tip);
    unsafe {
        // remove a stale icon left by a crashed instance / explorer restart
        Shell_NotifyIconW(NIM_DELETE, &nid);
        let ok = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &nid);
        ok
    }
}

pub fn update(hwnd: HWND, icon: Option<HICON>, tip: &str) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_TIP | NIF_SHOWTIP;
    if let Some(icon) = icon {
        nid.uFlags |= NIF_ICON;
        nid.hIcon = icon;
    }
    copy_to_buf(&mut nid.szTip, tip);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

pub fn balloon(hwnd: HWND, title: &str, text: &str, error: bool, sound: bool) -> bool {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = if error { NIIF_WARNING } else { NIIF_INFO };
    if !sound {
        nid.dwInfoFlags |= NIIF_NOSOUND;
    }
    copy_to_buf(&mut nid.szInfoTitle, title);
    copy_to_buf(&mut nid.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) != 0 }
}

pub fn remove(hwnd: HWND) {
    let nid = base(hwnd);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
}
