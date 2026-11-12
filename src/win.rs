//! Thin wrappers over common Win32 control calls.

#![allow(dead_code)]

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::HFONT;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::util::{from_wide, wide};

pub const BST_CHECKED: usize = 1;

pub fn send(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { SendMessageW(hwnd, msg, wparam, lparam) }
}

pub fn item(hwnd: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(hwnd, id) }
}

pub fn set_text(hwnd: HWND, text: &str) {
    let w = wide(text);
    unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
}

pub fn get_text(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    let mut buf = vec![0u16; len as usize + 1];
    unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    from_wide(&buf)
}

pub fn set_item_text(dlg: HWND, id: i32, text: &str) {
    set_text(item(dlg, id), text);
}

pub fn set_check(dlg: HWND, id: i32, checked: bool) {
    send(item(dlg, id), BM_SETCHECK, if checked { BST_CHECKED } else { 0 }, 0);
}

pub fn is_checked(dlg: HWND, id: i32) -> bool {
    send(item(dlg, id), BM_GETCHECK, 0, 0) as usize == BST_CHECKED
}

pub fn enable(dlg: HWND, id: i32, enabled: bool) {
    unsafe { EnableWindow(item(dlg, id), enabled as i32) };
}

pub fn is_enabled(dlg: HWND, id: i32) -> bool {
    unsafe { IsWindowEnabled(item(dlg, id)) != 0 }
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    from_wide(&buf)
}

pub fn style(hwnd: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 }
}

pub fn font(hwnd: HWND) -> HFONT {
    send(hwnd, WM_GETFONT, 0, 0) as HFONT
}

pub fn set_font(hwnd: HWND, font: HFONT) {
    send(hwnd, WM_SETFONT, font as usize, 1);
}

/// Returns a value from a dialog procedure through DWLP_MSGRESULT.
pub fn dlg_result(hwnd: HWND, value: isize) -> isize {
    unsafe { SetWindowLongPtrW(hwnd, 0 /* DWLP_MSGRESULT */, value) };
    1
}

pub fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}

pub fn message_box(hwnd: HWND, text: &str, title: &str, flags: u32) -> i32 {
    let t = wide(text);
    let c = wide(title);
    unsafe { MessageBoxW(hwnd, t.as_ptr(), c.as_ptr(), flags) }
}

// Up-down -------------------------------------------------------------------

pub fn updown_set_range(dlg: HWND, id: i32, min: i32, max: i32) {
    send(item(dlg, id), UDM_SETRANGE32, min as usize, max as isize);
}

pub fn updown_set(dlg: HWND, id: i32, value: i32) {
    send(item(dlg, id), UDM_SETPOS32, 0, value as isize);
}

pub fn updown_get(dlg: HWND, id: i32) -> i32 {
    let mut err = 0i32;
    send(item(dlg, id), UDM_GETPOS32, 0, &mut err as *mut i32 as isize) as i32
}

// Combo box -----------------------------------------------------------------

pub fn combo_add(dlg: HWND, id: i32, text: &str) {
    let w = wide(text);
    send(item(dlg, id), CB_ADDSTRING, 0, w.as_ptr() as isize);
}

pub fn combo_clear(dlg: HWND, id: i32) {
    send(item(dlg, id), CB_RESETCONTENT, 0, 0);
}

pub fn combo_set(dlg: HWND, id: i32, index: i32) {
    send(item(dlg, id), CB_SETCURSEL, index as usize, 0);
}

pub fn combo_get(dlg: HWND, id: i32) -> i32 {
    send(item(dlg, id), CB_GETCURSEL, 0, 0) as i32
}

// List view -----------------------------------------------------------------

pub fn lv_set_ex_style(lv: HWND, style: u32) {
    send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, style as usize, style as isize);
}

pub fn lv_add_column(lv: HWND, index: i32, width: i32, fmt: i32) {
    unsafe {
        let mut col: LVCOLUMNW = std::mem::zeroed();
        col.mask = (LVCF_WIDTH | LVCF_FMT | LVCF_SUBITEM) as _;
        col.fmt = fmt as _;
        col.cx = width;
        col.iSubItem = index;
        send(lv, LVM_INSERTCOLUMNW, index as usize, &col as *const _ as isize);
    }
}

pub fn lv_set_column_width(lv: HWND, index: i32, width: i32) {
    send(lv, LVM_SETCOLUMNWIDTH, index as usize, width as isize);
}

pub fn lv_add_item(lv: HWND, index: i32, text: &str, lparam: isize) {
    let w = wide(text);
    unsafe {
        let mut it: LVITEMW = std::mem::zeroed();
        it.mask = (LVIF_TEXT | LVIF_PARAM) as _;
        it.iItem = index;
        it.pszText = w.as_ptr() as *mut u16;
        it.lParam = lparam;
        send(lv, LVM_INSERTITEMW, 0, &it as *const _ as isize);
    }
}

pub fn lv_set_text(lv: HWND, index: i32, sub: i32, text: &str) {
    let w = wide(text);
    unsafe {
        let mut it: LVITEMW = std::mem::zeroed();
        it.iSubItem = sub;
        it.pszText = w.as_ptr() as *mut u16;
        send(lv, LVM_SETITEMTEXTW, index as usize, &it as *const _ as isize);
    }
}

pub fn lv_set_param(lv: HWND, index: i32, lparam: isize) {
    unsafe {
        let mut it: LVITEMW = std::mem::zeroed();
        it.mask = LVIF_PARAM as _;
        it.iItem = index;
        it.lParam = lparam;
        send(lv, LVM_SETITEMW, 0, &it as *const _ as isize);
    }
}

pub fn lv_get_param(lv: HWND, index: i32) -> isize {
    unsafe {
        let mut it: LVITEMW = std::mem::zeroed();
        it.mask = LVIF_PARAM as _;
        it.iItem = index;
        send(lv, LVM_GETITEMW, 0, &mut it as *mut _ as isize);
        it.lParam
    }
}

pub fn lv_count(lv: HWND) -> i32 {
    send(lv, LVM_GETITEMCOUNT, 0, 0) as i32
}

pub fn lv_delete_all(lv: HWND) {
    send(lv, LVM_DELETEALLITEMS, 0, 0);
}

pub fn lv_set_check(lv: HWND, index: i32, checked: bool) {
    unsafe {
        let mut it: LVITEMW = std::mem::zeroed();
        it.stateMask = LVIS_STATEIMAGEMASK as _;
        it.state = ((if checked { 2u32 } else { 1u32 }) << 12) as _;
        send(lv, LVM_SETITEMSTATE, index as usize, &it as *const _ as isize);
    }
}

pub fn lv_is_checked(lv: HWND, index: i32) -> bool {
    let state = send(lv, LVM_GETITEMSTATE, index as usize, LVIS_STATEIMAGEMASK as isize) as u32;
    (state >> 12) == 2
}

pub fn lv_redraw(lv: HWND) {
    let count = lv_count(lv);
    if count > 0 {
        send(lv, LVM_REDRAWITEMS, 0, (count - 1) as isize);
    }
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(lv, std::ptr::null(), 1) };
}
