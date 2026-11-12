//! UI color theming: presets, custom palettes and all custom drawing that
//! makes standard Win32 controls follow the chosen colors.
//!
//! "System" leaves every control to Windows. Any other theme paints dialog
//! backgrounds, labels, edits, list views, buttons, checkboxes, the settings
//! page list and the main menu bar with the palette, and sets a matching
//! title bar (dark mode flag on Windows 10, exact caption color on Windows 11).

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config;
use crate::dlg::{BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_DEFSPLITBUTTON, BS_PUSHBUTTON, BS_SPLITBUTTON, BS_TYPEMASK};
use crate::sys;
use crate::util::{blend, is_dark, rgb, wide};
use crate::win;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub bg: u32,
    pub text: u32,
    pub accent: u32,
    pub accent_text: u32,
    pub control: u32,
    pub border: u32,
}

impl Palette {
    pub fn get(&self, index: usize) -> u32 {
        match index {
            0 => self.bg,
            1 => self.text,
            2 => self.accent,
            3 => self.accent_text,
            4 => self.control,
            _ => self.border,
        }
    }

    pub fn set(&mut self, index: usize, color: u32) {
        match index {
            0 => self.bg = color,
            1 => self.text = color,
            2 => self.accent = color,
            3 => self.accent_text = color,
            4 => self.control = color,
            _ => self.border = color,
        }
    }
}

pub struct Preset {
    pub key: &'static str,
    pub name: &'static str,
    pub palette: Option<Palette>,
}

pub const LIGHT: Palette = Palette {
    bg: rgb(0xF3, 0xF3, 0xF3),
    text: rgb(0x1B, 0x1B, 0x1B),
    accent: rgb(0x00, 0x67, 0xC0),
    accent_text: rgb(0xFF, 0xFF, 0xFF),
    control: rgb(0xFF, 0xFF, 0xFF),
    border: rgb(0xC8, 0xC8, 0xC8),
};

pub const DARK: Palette = Palette {
    bg: rgb(0x20, 0x20, 0x20),
    text: rgb(0xE8, 0xE8, 0xE8),
    accent: rgb(0x0E, 0x63, 0x9C),
    accent_text: rgb(0xFF, 0xFF, 0xFF),
    control: rgb(0x2B, 0x2B, 0x2B),
    border: rgb(0x45, 0x45, 0x45),
};

pub const MIDNIGHT: Palette = Palette {
    bg: rgb(0x0F, 0x17, 0x2A),
    text: rgb(0xE2, 0xE8, 0xF0),
    accent: rgb(0x63, 0x66, 0xF1),
    accent_text: rgb(0xFF, 0xFF, 0xFF),
    control: rgb(0x1E, 0x29, 0x3B),
    border: rgb(0x33, 0x41, 0x55),
};

pub const FOREST: Palette = Palette {
    bg: rgb(0x1B, 0x2A, 0x22),
    text: rgb(0xDD, 0xEB, 0xDD),
    accent: rgb(0x2F, 0x85, 0x5A),
    accent_text: rgb(0xFF, 0xFF, 0xFF),
    control: rgb(0x24, 0x38, 0x2D),
    border: rgb(0x35, 0x55, 0x43),
};

pub const SUNSET: Palette = Palette {
    bg: rgb(0xFF, 0xF7, 0xED),
    text: rgb(0x43, 0x14, 0x07),
    accent: rgb(0xEA, 0x58, 0x0C),
    accent_text: rgb(0xFF, 0xFF, 0xFF),
    control: rgb(0xFF, 0xFF, 0xFF),
    border: rgb(0xFD, 0xBA, 0x74),
};

pub const CUSTOM_KEY: &str = "custom";

pub const PRESETS: [Preset; 7] = [
    Preset { key: "system", name: "System default", palette: None },
    Preset { key: "light", name: "Light", palette: Some(LIGHT) },
    Preset { key: "dark", name: "Dark", palette: Some(DARK) },
    Preset { key: "midnight", name: "Midnight", palette: Some(MIDNIGHT) },
    Preset { key: "forest", name: "Forest", palette: Some(FOREST) },
    Preset { key: "sunset", name: "Sunset", palette: Some(SUNSET) },
    Preset { key: CUSTOM_KEY, name: "Custom", palette: None },
];

/// Config key and label of each palette slot, in `Palette::get` order.
pub const COLOR_SLOTS: [(&str, &str); 6] = [
    ("UiColorBg", "Window background"),
    ("UiColorText", "Text"),
    ("UiColorAccent", "Accent (buttons, headings)"),
    ("UiColorAccentText", "Text on accent"),
    ("UiColorControl", "Lists and input fields"),
    ("UiColorBorder", "Borders"),
];

pub fn current_key() -> String {
    let key = config::get_string("UiTheme", "system").to_lowercase();
    if PRESETS.iter().any(|p| p.key == key) {
        key
    } else {
        "system".to_string()
    }
}

pub fn current_index() -> usize {
    let key = current_key();
    PRESETS.iter().position(|p| p.key == key).unwrap_or(0)
}

pub fn custom_palette() -> Palette {
    let mut p = DARK;
    for (i, (key, _)) in COLOR_SLOTS.iter().enumerate() {
        p.set(i, config::get_u32(key, DARK.get(i)));
    }
    p
}

fn save_custom_palette(p: &Palette) {
    for (i, (key, _)) in COLOR_SLOTS.iter().enumerate() {
        config::set_u32(key, p.get(i));
    }
}

pub fn palette_for(key: &str) -> Option<Palette> {
    if key == CUSTOM_KEY {
        return Some(custom_palette());
    }
    PRESETS.iter().find(|p| p.key == key).and_then(|p| p.palette)
}

/// Colors shown in the theme editor: the active palette, or Light for System.
pub fn editable_palette() -> Palette {
    palette_for(&current_key()).unwrap_or(LIGHT)
}

pub fn set_theme(key: &str) {
    config::set_string("UiTheme", key);
    reload();
}

/// Changes one color. Editing a preset turns it into the custom theme,
/// starting from that preset's colors.
pub fn set_color(slot: usize, color: u32) {
    let mut p = editable_palette();
    p.set(slot, color);
    save_custom_palette(&p);
    set_theme(CUSTOM_KEY);
}

// ---------------------------------------------------------------------------
// Live state: palette and GDI brushes
// ---------------------------------------------------------------------------

struct State {
    palette: Option<Palette>,
    bg_brush: HBRUSH,
    control_brush: HBRUSH,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State {
        palette: None,
        bg_brush: std::ptr::null_mut(),
        control_brush: std::ptr::null_mut(),
    }) };
}

/// Re-reads the theme from config and rebuilds brushes.
pub fn reload() {
    let palette = palette_for(&current_key());
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        unsafe {
            if !s.bg_brush.is_null() {
                DeleteObject(s.bg_brush);
            }
            if !s.control_brush.is_null() {
                DeleteObject(s.control_brush);
            }
        }
        s.palette = palette;
        s.bg_brush = palette.map(|p| unsafe { CreateSolidBrush(p.bg) }).unwrap_or(std::ptr::null_mut());
        s.control_brush = palette.map(|p| unsafe { CreateSolidBrush(p.control) }).unwrap_or(std::ptr::null_mut());
    });
}

/// `None` means "System default": let Windows draw everything.
pub fn palette() -> Option<Palette> {
    STATE.with(|s| s.borrow().palette)
}

pub fn bg_brush() -> HBRUSH {
    STATE.with(|s| s.borrow().bg_brush)
}

fn control_brush() -> HBRUSH {
    STATE.with(|s| s.borrow().control_brush)
}

fn sys_color(index: SYS_COLOR_INDEX) -> u32 {
    unsafe { GetSysColor(index) }
}

/// Text color for normal content (palette or system).
pub fn text_color() -> u32 {
    palette().map(|p| p.text).unwrap_or_else(|| sys_color(COLOR_WINDOWTEXT))
}

/// Color for section headings.
pub fn heading_color() -> u32 {
    match palette() {
        Some(p) => {
            // keep the accent readable when it is too close to the background:
            // move it toward the text color until the contrast is sufficient
            let lum_bg = crate::util::luminance(p.bg);
            let mut c = p.accent;
            for step in 1..=5 {
                if (crate::util::luminance(c) - lum_bg).abs() >= 0.3 {
                    break;
                }
                c = blend(p.accent, p.text, step as f32 * 0.15);
            }
            c
        }
        None => sys_color(COLOR_HOTLIGHT),
    }
}

// ---------------------------------------------------------------------------
// Heading labels
// ---------------------------------------------------------------------------

const HEADING_PROP: &str = "mr_heading";
const SHIELD_PROP: &str = "mr_shield";
const PLAIN_PROP: &str = "mr_plain";

/// List views that should never get a visual style (no column separator).
pub fn mark_plain(ctl: HWND) {
    let name = wide(PLAIN_PROP);
    unsafe { SetPropW(ctl, name.as_ptr(), 1 as _) };
}

pub fn mark_heading(ctl: HWND) {
    let name = wide(HEADING_PROP);
    unsafe { SetPropW(ctl, name.as_ptr(), 1 as _) };
}

fn is_heading(ctl: HWND) -> bool {
    let name = wide(HEADING_PROP);
    unsafe { !GetPropW(ctl, name.as_ptr()).is_null() }
}

/// Remembers that a button shows the UAC shield (custom drawing needs it).
pub fn mark_shield(ctl: HWND, on: bool) {
    let name = wide(SHIELD_PROP);
    unsafe {
        if on {
            SetPropW(ctl, name.as_ptr(), 1 as _);
        } else {
            RemovePropW(ctl, name.as_ptr());
        }
    }
}

fn has_shield(ctl: HWND) -> bool {
    let name = wide(SHIELD_PROP);
    unsafe { !GetPropW(ctl, name.as_ptr()).is_null() }
}

// ---------------------------------------------------------------------------
// Message helpers used by every dialog procedure
// ---------------------------------------------------------------------------

/// Handles WM_CTLCOLOR*; returns the brush to give back to Windows.
pub fn ctl_color(msg: u32, hdc: HDC, ctl: HWND) -> Option<isize> {
    let heading = is_heading(ctl);
    let p = match palette() {
        Some(p) => p,
        None => {
            if heading && msg == WM_CTLCOLORSTATIC {
                unsafe {
                    SetTextColor(hdc, heading_color());
                    SetBkMode(hdc, TRANSPARENT as i32);
                    return Some(GetSysColorBrush(COLOR_BTNFACE) as isize);
                }
            }
            return None;
        }
    };
    unsafe {
        match msg {
            WM_CTLCOLORDLG => Some(bg_brush() as isize),
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                let enabled = IsWindowEnabled(ctl) != 0;
                let color = if heading {
                    heading_color()
                } else if enabled {
                    p.text
                } else {
                    blend(p.text, p.bg, 0.5)
                };
                SetTextColor(hdc, color);
                SetBkColor(hdc, p.bg);
                Some(bg_brush() as isize)
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                SetTextColor(hdc, p.text);
                SetBkColor(hdc, p.control);
                Some(control_brush() as isize)
            }
            _ => None,
        }
    }
}

fn set_dwm_bool(hwnd: HWND, attr: u32, value: bool) {
    let v: BOOL = value as BOOL;
    unsafe { DwmSetWindowAttribute(hwnd, attr, &v as *const _ as *const _, 4) };
}

fn set_dwm_color(hwnd: HWND, attr: u32, value: u32) {
    unsafe { DwmSetWindowAttribute(hwnd, attr, &value as *const _ as *const _, 4) };
}

fn set_theme_name(hwnd: HWND, name: Option<&str>) {
    unsafe {
        match name {
            Some(n) => {
                let w = wide(n);
                SetWindowTheme(hwnd, w.as_ptr(), std::ptr::null());
            }
            None => {
                SetWindowTheme(hwnd, std::ptr::null(), std::ptr::null());
            }
        }
    }
}

unsafe extern "system" fn apply_child(child: HWND, _: LPARAM) -> BOOL {
    let p = palette();
    let dark = p.map(|p| is_dark(p.bg)).unwrap_or(false);
    let class = win::class_name(child).to_ascii_lowercase();
    match class.as_str() {
        "syslistview32" => {
            let bg = p.map(|p| p.control).unwrap_or_else(|| sys_color(COLOR_WINDOW));
            let fg = p.map(|p| p.text).unwrap_or_else(|| sys_color(COLOR_WINDOWTEXT));
            win::send(child, LVM_SETBKCOLOR, 0, bg as isize);
            win::send(child, LVM_SETTEXTBKCOLOR, 0, bg as isize);
            win::send(child, LVM_SETTEXTCOLOR, 0, fg as isize);
            // themed report views draw a column separator line; plain lists skip it
            let plain = !GetPropW(child, wide(PLAIN_PROP).as_ptr()).is_null();
            set_theme_name(child, if dark && !plain { Some("DarkMode_Explorer") } else { None });
        }
        "combobox" | "edit" => set_theme_name(child, if dark { Some("DarkMode_CFD") } else { None }),
        "msctls_updown32" | "scrollbar" | "listbox" => {
            set_theme_name(child, if dark { Some("DarkMode_Explorer") } else { None })
        }
        _ => {}
    }
    InvalidateRect(child, std::ptr::null(), 1);
    1
}

/// Applies the current theme to a top-level window and all its children.
pub fn apply(top: HWND) {
    if top.is_null() {
        return;
    }
    let p = palette();
    let dark = p.map(|p| is_dark(p.bg)).unwrap_or(false);
    // 20 = DWMWA_USE_IMMERSIVE_DARK_MODE (19 on early Windows 10 builds)
    set_dwm_bool(top, 20, dark);
    set_dwm_bool(top, 19, dark);
    if sys::is_win11_or_later() {
        const DWMWA_CAPTION_COLOR: u32 = 35;
        const DWMWA_TEXT_COLOR: u32 = 36;
        const DWMWA_COLOR_DEFAULT: u32 = 0xFFFF_FFFF;
        set_dwm_color(top, DWMWA_CAPTION_COLOR, p.map(|p| p.bg).unwrap_or(DWMWA_COLOR_DEFAULT));
        set_dwm_color(top, DWMWA_TEXT_COLOR, p.map(|p| p.text).unwrap_or(DWMWA_COLOR_DEFAULT));
    }
    unsafe {
        EnumChildWindows(top, Some(apply_child), 0);
        RedrawWindow(top, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME);
    }
}

// ---------------------------------------------------------------------------
// Custom drawing
// ---------------------------------------------------------------------------

fn scale(hwnd: HWND, px: i32) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
    (px * dpi + 48) / 96
}

unsafe fn fill(hdc: HDC, rc: &RECT, color: u32) {
    let b = CreateSolidBrush(color);
    FillRect(hdc, rc, b);
    DeleteObject(b);
}

unsafe fn round_rect(hdc: HDC, rc: &RECT, radius: i32, fill_color: u32, border: u32) {
    let brush = CreateSolidBrush(fill_color);
    let pen = CreatePen(PS_SOLID, 1, border);
    let ob = SelectObject(hdc, brush);
    let op = SelectObject(hdc, pen);
    RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius, radius);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    DeleteObject(brush);
    DeleteObject(pen);
}

unsafe fn draw_text(hdc: HDC, text: &str, rc: &mut RECT, color: u32, flags: u32) {
    let mut w: Vec<u16> = text.encode_utf16().collect();
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, color);
    DrawTextW(hdc, w.as_mut_ptr(), w.len() as i32, rc, flags);
}

unsafe fn text_width(hdc: HDC, text: &str) -> i32 {
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut w: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(hdc, w.as_mut_ptr(), w.len() as i32, &mut rc, DT_CALCRECT | DT_SINGLELINE);
    rc.right - rc.left
}

fn shield_icon() -> HICON {
    thread_local! {
        static ICON: std::cell::Cell<HICON> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    }
    ICON.with(|c| {
        if c.get().is_null() {
            unsafe {
                use windows_sys::Win32::UI::Shell::{SHGetStockIconInfo, SHGSI_ICON, SHGSI_SMALLICON, SHSTOCKICONINFO, SIID_SHIELD};
                let mut info: SHSTOCKICONINFO = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<SHSTOCKICONINFO>() as u32;
                if SHGetStockIconInfo(SIID_SHIELD, SHGSI_ICON | SHGSI_SMALLICON, &mut info) >= 0 {
                    c.set(info.hIcon);
                }
            }
        }
        c.get()
    })
}

/// NM_CUSTOMDRAW for buttons. Returns the DWLP_MSGRESULT value when handled.
pub fn custom_draw_button(nmcd: &NMCUSTOMDRAW) -> Option<isize> {
    let p = palette()?;
    if nmcd.dwDrawStage != CDDS_PREPAINT {
        return None;
    }
    let hwnd = nmcd.hdr.hwndFrom;
    if !win::class_name(hwnd).eq_ignore_ascii_case("button") {
        return None;
    }
    let kind = win::style(hwnd) & BS_TYPEMASK;
    unsafe {
        let hdc = nmcd.hdc;
        let old_font = SelectObject(hdc, win::font(hwnd));
        let handled = match kind {
            BS_AUTOCHECKBOX | 0x2 /* BS_CHECKBOX */ => {
                draw_checkbox(hwnd, nmcd, &p);
                true
            }
            BS_PUSHBUTTON | BS_DEFPUSHBUTTON | BS_SPLITBUTTON | BS_DEFSPLITBUTTON => {
                draw_push(hwnd, nmcd, &p, kind == BS_SPLITBUTTON || kind == BS_DEFSPLITBUTTON);
                true
            }
            _ => false,
        };
        SelectObject(hdc, old_font);
        if handled {
            Some(CDRF_SKIPDEFAULT as isize)
        } else {
            None
        }
    }
}

unsafe fn draw_push(hwnd: HWND, nmcd: &NMCUSTOMDRAW, p: &Palette, split: bool) {
    let hdc = nmcd.hdc;
    let rc = nmcd.rc;
    let state = nmcd.uItemState;
    let enabled = IsWindowEnabled(hwnd) != 0;
    let pressed = state & CDIS_SELECTED != 0;
    let hot = state & CDIS_HOT != 0;
    let focus = state & CDIS_FOCUS != 0 && state & CDIS_SHOWKEYBOARDCUES != 0;

    let face = if !enabled {
        blend(p.accent, p.bg, 0.6)
    } else if pressed {
        blend(p.accent, rgb(0, 0, 0), 0.2)
    } else if hot {
        blend(p.accent, rgb(0xFF, 0xFF, 0xFF), 0.15)
    } else {
        p.accent
    };
    let fg = if enabled { p.accent_text } else { blend(p.accent_text, face, 0.5) };

    fill(hdc, &rc, p.bg);
    round_rect(hdc, &rc, scale(hwnd, 6), face, if focus { fg } else { blend(face, p.border, 0.5) });

    let mut content = rc;
    if split {
        let w = (rc.bottom - rc.top).max(scale(hwnd, 16));
        content.right -= w;
        // divider
        let pen = CreatePen(PS_SOLID, 1, blend(face, fg, 0.4));
        let op = SelectObject(hdc, pen);
        MoveToEx(hdc, content.right, rc.top + scale(hwnd, 4), std::ptr::null_mut());
        LineTo(hdc, content.right, rc.bottom - scale(hwnd, 4));
        SelectObject(hdc, op);
        DeleteObject(pen);
        // arrow
        let cx = content.right + w / 2;
        let cy = (rc.top + rc.bottom) / 2;
        let a = scale(hwnd, 4);
        let pts = [
            POINT { x: cx - a, y: cy - a / 2 },
            POINT { x: cx + a, y: cy - a / 2 },
            POINT { x: cx, y: cy + a / 2 + 1 },
        ];
        let brush = CreateSolidBrush(fg);
        let pen = CreatePen(PS_SOLID, 1, fg);
        let ob = SelectObject(hdc, brush);
        let op = SelectObject(hdc, pen);
        Polygon(hdc, pts.as_ptr(), 3);
        SelectObject(hdc, ob);
        SelectObject(hdc, op);
        DeleteObject(brush);
        DeleteObject(pen);
    }

    let text = win::get_text(hwnd);
    let shield = if has_shield(hwnd) { shield_icon() } else { std::ptr::null_mut() };
    let icon_size = scale(hwnd, 16);
    let gap = scale(hwnd, 6);
    let tw = text_width(hdc, &text);
    let total = tw + if shield.is_null() { 0 } else { icon_size + gap };
    let mut x = content.left + ((content.right - content.left) - total) / 2;
    if !shield.is_null() {
        let y = (rc.top + rc.bottom - icon_size) / 2;
        DrawIconEx(hdc, x, y, shield, icon_size, icon_size, 0, std::ptr::null_mut(), DI_NORMAL);
        x += icon_size + gap;
    }
    let mut trc = RECT { left: x, top: rc.top, right: x + tw + 2, bottom: rc.bottom };
    if pressed {
        OffsetRect(&mut trc, 0, 1);
    }
    draw_text(hdc, &text, &mut trc, fg, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOCLIP);
}

unsafe fn draw_checkbox(hwnd: HWND, nmcd: &NMCUSTOMDRAW, p: &Palette) {
    let hdc = nmcd.hdc;
    let rc = nmcd.rc;
    let state = nmcd.uItemState;
    let enabled = IsWindowEnabled(hwnd) != 0;
    let hot = state & CDIS_HOT != 0;
    let checked = win::send(hwnd, BM_GETCHECK, 0, 0) as usize == win::BST_CHECKED;
    let focus = state & CDIS_FOCUS != 0 && state & CDIS_SHOWKEYBOARDCUES != 0;

    fill(hdc, &rc, p.bg);

    let size = scale(hwnd, 14);
    let top = rc.top + ((rc.bottom - rc.top) - size) / 2;
    let boxr = RECT { left: rc.left, top, right: rc.left + size, bottom: top + size };

    let (face, edge) = if !enabled {
        let c = blend(p.control, p.bg, 0.5);
        (if checked { blend(p.accent, p.bg, 0.6) } else { c }, blend(p.border, p.bg, 0.5))
    } else if checked {
        (if hot { blend(p.accent, rgb(0xFF, 0xFF, 0xFF), 0.15) } else { p.accent }, p.accent)
    } else {
        (p.control, if hot { p.accent } else { blend(p.border, p.text, 0.25) })
    };
    round_rect(hdc, &boxr, scale(hwnd, 4), face, edge);

    if checked {
        let s = size as f32;
        let (x, y) = (boxr.left as f32, boxr.top as f32);
        let pts = [
            POINT { x: (x + s * 0.22) as i32, y: (y + s * 0.52) as i32 },
            POINT { x: (x + s * 0.42) as i32, y: (y + s * 0.72) as i32 },
            POINT { x: (x + s * 0.78) as i32, y: (y + s * 0.30) as i32 },
        ];
        let pen = CreatePen(PS_SOLID, scale(hwnd, 2), if enabled { p.accent_text } else { blend(p.accent_text, face, 0.5) });
        let op = SelectObject(hdc, pen);
        Polyline(hdc, pts.as_ptr(), 3);
        SelectObject(hdc, op);
        DeleteObject(pen);
    }

    let text = win::get_text(hwnd);
    let mut trc = RECT { left: boxr.right + scale(hwnd, 6), top: rc.top, right: rc.right, bottom: rc.bottom };
    let color = if enabled { p.text } else { blend(p.text, p.bg, 0.5) };
    draw_text(hdc, &text, &mut trc, color, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS);

    if focus {
        let tw = text_width(hdc, &text);
        let f = RECT { left: trc.left - 2, top: trc.top + 1, right: (trc.left + tw + 2).min(rc.right), bottom: trc.bottom - 1 };
        SetTextColor(hdc, p.text);
        SetBkColor(hdc, p.bg);
        DrawFocusRect(hdc, &f);
    }
}

// ---------------------------------------------------------------------------
// Owner-drawn list box (settings page list) and menu bar
// ---------------------------------------------------------------------------

pub fn draw_listbox_item(dis: &DRAWITEMSTRUCT, text: &str) {
    if dis.itemID == u32::MAX {
        return;
    }
    unsafe {
        let hdc = dis.hDC;
        let selected = dis.itemState & ODS_SELECTED != 0;
        let (bg, fg) = match palette() {
            Some(p) if selected => (p.accent, p.accent_text),
            Some(p) => (p.control, p.text),
            None if selected => (sys_color(COLOR_HIGHLIGHT), sys_color(COLOR_HIGHLIGHTTEXT)),
            None => (sys_color(COLOR_WINDOW), sys_color(COLOR_WINDOWTEXT)),
        };
        fill(hdc, &dis.rcItem, bg);
        let mut rc = dis.rcItem;
        rc.left += scale(dis.hwndItem, 8);
        draw_text(hdc, text, &mut rc, fg, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS | DT_NOPREFIX);
        if dis.itemState & ODS_FOCUS != 0 && dis.itemState & ODS_NOFOCUSRECT == 0 {
            let f = dis.rcItem;
            SetTextColor(hdc, fg);
            SetBkColor(hdc, bg);
            DrawFocusRect(hdc, &f);
        }
    }
}

fn menu_font(hwnd: HWND) -> HFONT {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
        ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
        SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS, ncm.cbSize, &mut ncm as *mut _ as *mut _, 0, dpi);
        CreateFontIndirectW(&ncm.lfMenuFont)
    }
}

pub fn measure_menu_bar_item(hwnd: HWND, mis: &mut MEASUREITEMSTRUCT, text: &str) {
    unsafe {
        let hdc = GetDC(hwnd);
        let font = menu_font(hwnd);
        let old = SelectObject(hdc, font);
        let clean: String = text.replace('&', "");
        mis.itemWidth = (text_width(hdc, &clean) + scale(hwnd, 12)) as u32;
        mis.itemHeight = GetSystemMetricsForDpiSafe(hwnd, SM_CYMENU) as u32;
        SelectObject(hdc, old);
        DeleteObject(font);
        ReleaseDC(hwnd, hdc);
    }
}

#[allow(non_snake_case)]
fn GetSystemMetricsForDpiSafe(hwnd: HWND, metric: SYSTEM_METRICS_INDEX) -> i32 {
    unsafe { windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(metric, GetDpiForWindow(hwnd).max(96)) }
}

pub fn draw_menu_bar_item(hwnd: HWND, dis: &DRAWITEMSTRUCT, text: &str) {
    let p = match palette() {
        Some(p) => p,
        None => return,
    };
    unsafe {
        let hdc = dis.hDC;
        let active = dis.itemState & (ODS_SELECTED | ODS_HOTLIGHT) != 0;
        let bg = if active { blend(p.bg, p.text, 0.15) } else { p.bg };
        fill(hdc, &dis.rcItem, bg);
        let font = menu_font(hwnd);
        let old = SelectObject(hdc, font);
        let mut rc = dis.rcItem;
        let mut flags = DT_SINGLELINE | DT_VCENTER | DT_CENTER;
        if dis.itemState & ODS_NOACCEL != 0 {
            flags |= DT_HIDEPREFIX;
        }
        let color = if dis.itemState & ODS_INACTIVE != 0 { blend(p.text, p.bg, 0.4) } else { p.text };
        draw_text(hdc, text, &mut rc, color, flags);
        SelectObject(hdc, old);
        DeleteObject(font);
    }
}

/// Paints the strip of menu bar to the right of the last item.
pub fn set_menu_bar_background(menu: HMENU) {
    unsafe {
        let mut mi: MENUINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MENUINFO>() as u32;
        mi.fMask = MIM_BACKGROUND;
        mi.hbrBack = bg_brush();
        SetMenuInfo(menu, &mi);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_slots_roundtrip() {
        let mut p = LIGHT;
        for i in 0..COLOR_SLOTS.len() {
            p.set(i, i as u32 + 1);
        }
        for i in 0..COLOR_SLOTS.len() {
            assert_eq!(p.get(i), i as u32 + 1);
        }
    }

    #[test]
    fn preset_keys_unique() {
        for (i, a) in PRESETS.iter().enumerate() {
            for b in PRESETS.iter().skip(i + 1) {
                assert_ne!(a.key, b.key);
            }
        }
    }
}
