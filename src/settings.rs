//! Settings window: a page list on the left and one child dialog per page.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config;
use crate::dlg::{self as d, DialogTemplate};
use crate::icon::{self, FontSpec};
use crate::mainwnd;
use crate::memory::{MASK_DEFAULT, MASK_FREEZES, REGIONS};
use crate::sys::{self, APP_NAME};
use crate::theme;
use crate::util::{color_to_hex, contrast_text};
use crate::win;

pub const PAGE_GENERAL: usize = 0;
pub const PAGE_MEMORY: usize = 1;
pub const PAGE_APPEARANCE: usize = 2;
pub const PAGE_TRAY: usize = 3;
pub const PAGE_THEME: usize = 4;
#[allow(dead_code)]
pub const PAGE_ADVANCED: usize = 5;

const PAGE_NAMES: [&str; 6] = ["General", "Memory cleaning", "Tray icon style", "Tray icon", "Theme", "Advanced"];

const IDC_PAGES: i32 = 500;
const IDC_RESET: i32 = 501;
const IDCANCEL: i32 = 2;

// General
const IDC_ALWAYSONTOP: i32 = 600;
const IDC_LOADONSTARTUP: i32 = 601;
const IDC_STARTMINIMIZED: i32 = 602;
const IDC_REDUCTCONFIRM: i32 = 603;
const IDC_SKIPUAC: i32 = 604;
// Memory
const IDC_REGIONS: i32 = 610;
const IDC_AUTOREDUCT_CHK: i32 = 611;
const IDC_AUTOREDUCT_EDIT: i32 = 612;
const IDC_AUTOREDUCT_UD: i32 = 613;
const IDC_INTERVAL_CHK: i32 = 614;
const IDC_INTERVAL_EDIT: i32 = 615;
const IDC_INTERVAL_UD: i32 = 616;
const IDC_HOTKEY_CHK: i32 = 617;
const IDC_HOTKEY: i32 = 618;
// Tray icon style
const IDC_TRANSPARENT: i32 = 620;
const IDC_BORDER: i32 = 621;
const IDC_ROUND: i32 = 622;
const IDC_CHANGEBG: i32 = 623;
const IDC_ANTIALIAS: i32 = 624;
const IDC_FONT: i32 = 625;
const IDC_TRAY_COLORS: i32 = 626;
const IDC_THEME_COLORS: i32 = 627;
// Tray icon
const IDC_WARN_EDIT: i32 = 630;
const IDC_WARN_UD: i32 = 631;
const IDC_DANGER_EDIT: i32 = 632;
const IDC_DANGER_UD: i32 = 633;
const IDC_ACTION_SC: i32 = 634;
const IDC_ACTION_MC: i32 = 635;
const IDC_SHOW_RESULT: i32 = 636;
const IDC_SOUND: i32 = 637;
// Theme
const IDC_THEME: i32 = 640;
const IDC_UI_COLORS: i32 = 641;
const IDC_RESET_CUSTOM: i32 = 642;
// Advanced
const IDC_ALLOW_STANDBY: i32 = 650;
const IDC_LOG_RESULTS: i32 = 651;
const IDC_OPEN_CONFIG: i32 = 652;
const IDC_OPEN_LOG: i32 = 653;

const IDC_HEADING: i32 = 700; // + n, only used to find headings
const IDC_STATIC: i32 = -1;

const TRAY_COLORS: [(&str, &str, u32); 4] = [
    ("TrayColorText", "Text color", icon::COLOR_TEXT),
    ("TrayColorBg", "Background color", icon::COLOR_BG),
    ("TrayColorWarning", "\"Warning\" level color", icon::COLOR_WARNING),
    ("TrayColorDanger", "\"Danger\" level color", icon::COLOR_DANGER),
];

const TRAY_ACTIONS: [&str; 3] = ["Show/Hide", "Clean memory", "Open task manager"];

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static PAGES: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    static LOADING: Cell<bool> = const { Cell::new(false) };
    static HEADING_FONTS: RefCell<Vec<HFONT>> = const { RefCell::new(Vec::new()) };
    static CUSTOM_COLORS: RefCell<[u32; 16]> = const { RefCell::new([0xFFFFFF; 16]) };
}

fn loading() -> bool {
    LOADING.with(|l| l.get())
}

fn while_loading(f: impl FnOnce()) {
    let prev = LOADING.with(|l| l.replace(true));
    f();
    LOADING.with(|l| l.set(prev));
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

fn heading(t: &mut DialogTemplate, n: i32, text: &str, y: i16) {
    t.item(d::CLASS_STATIC, text, IDC_HEADING + n, d::SS_LEFTNOWORDWRAP | d::SS_NOPREFIX, 0, 0, y, 252, 10);
}

fn page_template(page: usize) -> Vec<u32> {
    let mut t = DialogTemplate::new(d::WS_CHILD | d::DS_CONTROL | d::WS_CLIPSIBLINGS, d::WS_EX_CONTROLPARENT, 0, 0, 252, 164, "");
    match page {
        PAGE_GENERAL => {
            heading(&mut t, 0, "General configuration", 0);
            t.checkbox(IDC_ALWAYSONTOP, "Always on top", 6, 14, 240, 12);
            t.checkbox(IDC_LOADONSTARTUP, "Load on system startup", 6, 28, 240, 12);
            t.checkbox(IDC_STARTMINIMIZED, "Start minimized", 6, 42, 240, 12);
            t.checkbox(IDC_REDUCTCONFIRM, "Confirm memory cleaning start", 6, 56, 240, 12);
            t.checkbox(IDC_SKIPUAC, "Skip \"User Account Control\" prompt warning", 6, 70, 240, 12);
        }
        PAGE_MEMORY => {
            heading(&mut t, 0, "Memory regions to be cleaned", 0);
            t.listview(IDC_REGIONS, d::WS_BORDER, 6, 12, 240, 64);
            t.label(IDC_STATIC, "* may freeze the computer for a while when cleaned", 6, 78, 240, 10);
            heading(&mut t, 1, "Automatic cleaning", 92);
            t.checkbox(IDC_AUTOREDUCT_CHK, "Clean when memory usage is above (%)", 6, 105, 186, 12);
            t.number(IDC_AUTOREDUCT_EDIT, IDC_AUTOREDUCT_UD, 196, 104, 50, 12);
            t.checkbox(IDC_INTERVAL_CHK, "Clean every (minutes)", 6, 120, 186, 12);
            t.number(IDC_INTERVAL_EDIT, IDC_INTERVAL_UD, 196, 119, 50, 12);
            heading(&mut t, 2, "Global hotkey", 136);
            t.checkbox(IDC_HOTKEY_CHK, "Clean memory:", 6, 149, 90, 12);
            t.item(d::CLASS_HOTKEY, "", IDC_HOTKEY, d::WS_TABSTOP | d::WS_BORDER, 0, 100, 148, 146, 13);
        }
        PAGE_APPEARANCE => {
            heading(&mut t, 0, "Tray icon style", 0);
            t.checkbox(IDC_THEME_COLORS, "Use interface theme colors (accent background)", 6, 12, 240, 12);
            t.checkbox(IDC_TRANSPARENT, "Use transparent background", 6, 24, 240, 12);
            t.checkbox(IDC_BORDER, "Show border", 6, 36, 240, 12);
            t.checkbox(IDC_ROUND, "Round corners", 6, 48, 240, 12);
            t.checkbox(IDC_CHANGEBG, "Change background color (instead of text color) on warning", 6, 60, 240, 12);
            t.checkbox(IDC_ANTIALIAS, "Use antialiasing", 6, 72, 240, 12);
            heading(&mut t, 1, "Font and colors (double-click a color to change it)", 88);
            t.label(IDC_STATIC, "Font:", 6, 101, 60, 12);
            t.button(IDC_FONT, "", 70, 100, 176, 14);
            t.listview(IDC_TRAY_COLORS, d::WS_BORDER, 6, 118, 240, 44);
        }
        PAGE_TRAY => {
            heading(&mut t, 0, "Color indication", 0);
            t.label(IDC_STATIC, "\"Warning\" level starts at (%):", 6, 13, 186, 12);
            t.number(IDC_WARN_EDIT, IDC_WARN_UD, 196, 12, 50, 12);
            t.label(IDC_STATIC, "\"Danger\" level starts at (%):", 6, 28, 186, 12);
            t.number(IDC_DANGER_EDIT, IDC_DANGER_UD, 196, 27, 50, 12);
            heading(&mut t, 1, "Mouse control", 46);
            t.label(IDC_STATIC, "Single click:", 6, 59, 90, 12);
            t.combo(IDC_ACTION_SC, 100, 58, 146, 80);
            t.label(IDC_STATIC, "Middle click:", 6, 75, 90, 12);
            t.combo(IDC_ACTION_MC, 100, 74, 146, 80);
            heading(&mut t, 2, "Notifications", 94);
            t.checkbox(IDC_SHOW_RESULT, "Show memory cleaning results", 6, 107, 240, 12);
            t.checkbox(IDC_SOUND, "Enable notification sound", 6, 120, 240, 12);
        }
        PAGE_THEME => {
            heading(&mut t, 0, "Interface theme", 0);
            t.label(IDC_STATIC, "Theme:", 6, 13, 60, 12);
            t.combo(IDC_THEME, 70, 12, 176, 120);
            heading(&mut t, 1, "Colors (double-click to change)", 32);
            t.listview(IDC_UI_COLORS, d::WS_BORDER, 6, 44, 240, 92);
            t.label(IDC_STATIC, "Editing a color switches to the \"Custom\" theme.", 6, 138, 240, 10);
            t.button(IDC_RESET_CUSTOM, "Reset custom colors", 6, 150, 100, 14);
        }
        _ => {
            heading(&mut t, 0, "Advanced", 0);
            t.checkbox(IDC_ALLOW_STANDBY, "Allow \"Standby list\" and \"Modified page list\" on auto-clean", 6, 12, 240, 12);
            t.checkbox(IDC_LOG_RESULTS, "Log cleaning results into memreduct.log", 6, 25, 240, 12);
            heading(&mut t, 1, "Files", 44);
            t.button(IDC_OPEN_CONFIG, "Open configuration folder", 6, 56, 120, 14);
            t.button(IDC_OPEN_LOG, "Open log file", 130, 56, 116, 14);
        }
    }
    t.build()
}

fn window_template() -> Vec<u32> {
    let mut t = DialogTemplate::new(
        d::WS_POPUP | d::WS_CAPTION | d::WS_SYSMENU | d::WS_MINIMIZEBOX | d::DS_MODALFRAME | d::DS_CENTER | d::WS_CLIPCHILDREN,
        d::WS_EX_APPWINDOW | d::WS_EX_CONTROLPARENT,
        0,
        0,
        350,
        196,
        "Settings",
    );
    t.item(
        d::CLASS_LISTBOX,
        "",
        IDC_PAGES,
        d::LBS_NOTIFY | d::LBS_OWNERDRAWFIXED | d::LBS_HASSTRINGS | d::LBS_NOINTEGRALHEIGHT | d::WS_TABSTOP,
        0,
        6,
        6,
        78,
        164,
    );
    t.button(IDC_RESET, "Reset", 6, 176, 60, 14);
    t.button(IDCANCEL, "Close", 284, 176, 60, 14);
    t.build()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn open(_owner: HWND, page: Option<usize>) {
    let existing = WINDOW.with(|w| w.get());
    if !existing.is_null() {
        if let Some(p) = page {
            select_page(existing, p);
        }
        unsafe {
            ShowWindow(existing, SW_RESTORE);
            SetForegroundWindow(existing);
        }
        return;
    }
    let start = page.unwrap_or_else(|| (config::get_u32("SettingsLastPage", 0) as usize).min(PAGE_NAMES.len() - 1));
    let tpl = window_template();
    unsafe {
        let hwnd = CreateDialogIndirectParamW(
            GetModuleHandleW(std::ptr::null()),
            tpl.as_ptr() as *const DLGTEMPLATE,
            std::ptr::null_mut(),
            Some(window_proc),
            start as isize,
        );
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
    }
}

pub fn close() {
    let hwnd = WINDOW.with(|w| w.get());
    if !hwnd.is_null() {
        unsafe { DestroyWindow(hwnd) };
    }
}

pub fn apply_theme() {
    let hwnd = WINDOW.with(|w| w.get());
    if hwnd.is_null() {
        return;
    }
    // EnumChildWindows inside apply() also reaches the page dialogs' controls
    theme::apply(hwnd);
    unsafe { InvalidateRect(win::item(hwnd, IDC_PAGES), std::ptr::null(), 1) };
}

/// Re-reads every value shown in the settings pages (after a change made
/// from the tray or main menu).
pub fn reload_page_values() {
    PAGES.with(|p| {
        for (i, &page) in p.borrow().iter().enumerate() {
            while_loading(|| load_page(i, page));
        }
    });
}

// ---------------------------------------------------------------------------
// Settings window
// ---------------------------------------------------------------------------

fn select_page(hwnd: HWND, page: usize) {
    let page = page.min(PAGE_NAMES.len() - 1);
    win::send(win::item(hwnd, IDC_PAGES), LB_SETCURSEL, page, 0);
    PAGES.with(|p| {
        for (i, &h) in p.borrow().iter().enumerate() {
            unsafe { ShowWindow(h, if i == page { SW_SHOW } else { SW_HIDE }) };
        }
    });
    config::set_u32("SettingsLastPage", page as u32);
}

fn init_window(hwnd: HWND, start: usize) {
    WINDOW.with(|w| w.set(hwnd));
    let main = mainwnd::main_hwnd();
    let big = win::send(main, WM_GETICON, ICON_BIG as usize, 0);
    let small = win::send(main, WM_GETICON, ICON_SMALL as usize, 0);
    win::send(hwnd, WM_SETICON, ICON_BIG as usize, big);
    win::send(hwnd, WM_SETICON, ICON_SMALL as usize, small);

    let list = win::item(hwnd, IDC_PAGES);
    for name in PAGE_NAMES {
        let w = crate::util::wide(name);
        win::send(list, LB_ADDSTRING, 0, w.as_ptr() as isize);
    }

    // page area in dialog units -> pixels
    let mut area = RECT { left: 92, top: 6, right: 92 + 252, bottom: 6 + 164 };
    unsafe { MapDialogRect(hwnd, &mut area) };

    let mut pages = Vec::new();
    for i in 0..PAGE_NAMES.len() {
        let tpl = page_template(i);
        let page = unsafe {
            CreateDialogIndirectParamW(
                GetModuleHandleW(std::ptr::null()),
                tpl.as_ptr() as *const DLGTEMPLATE,
                hwnd,
                Some(page_proc),
                i as isize,
            )
        };
        unsafe {
            SetWindowPos(
                page,
                std::ptr::null_mut(),
                area.left,
                area.top,
                area.right - area.left,
                area.bottom - area.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        pages.push(page);
    }
    PAGES.with(|p| *p.borrow_mut() = pages);

    select_page(hwnd, start);
    apply_theme();
}

fn reset_all(hwnd: HWND) {
    let (ok, _) = mainwnd::confirm(hwnd, "Are you really sure you want to reset all application settings?", "", true);
    if !ok {
        return;
    }
    config::reset();
    mainwnd::apply_theme_everywhere();
    mainwnd::refresh_icon();
    mainwnd::init_hotkey();
    mainwnd::update_menu_checks();
    reload_page_values();
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            init_window(hwnd, lparam as usize);
            0
        }
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            theme::ctl_color(msg, wparam as HDC, lparam as HWND).unwrap_or(0)
        }
        WM_MEASUREITEM => {
            let mis = &mut *(lparam as *mut MEASUREITEMSTRUCT);
            if mis.CtlID as i32 == IDC_PAGES {
                let dpi = GetDpiForWindow(hwnd).max(96);
                mis.itemHeight = (26 * dpi / 96) as u32;
                return 1;
            }
            0
        }
        WM_DRAWITEM => {
            let dis = &*(lparam as *const DRAWITEMSTRUCT);
            if dis.CtlID as i32 == IDC_PAGES {
                let text = PAGE_NAMES.get(dis.itemID as usize).copied().unwrap_or("");
                theme::draw_listbox_item(dis, text);
                return 1;
            }
            0
        }
        WM_NOTIFY => {
            let hdr = &*(lparam as *const NMHDR);
            if hdr.code == NM_CUSTOMDRAW {
                if let Some(r) = theme::custom_draw_button(&*(lparam as *const NMCUSTOMDRAW)) {
                    return win::dlg_result(hwnd, r);
                }
            }
            0
        }
        WM_COMMAND => {
            let id = win::loword(wparam) as i32;
            let code = win::hiword(wparam) as u32;
            match id {
                IDC_PAGES if code == LBN_SELCHANGE => {
                    let sel = win::send(win::item(hwnd, IDC_PAGES), LB_GETCURSEL, 0, 0);
                    if sel >= 0 {
                        select_page(hwnd, sel as usize);
                    }
                }
                IDC_RESET => reset_all(hwnd),
                IDCANCEL => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            1
        }
        WM_DESTROY => {
            WINDOW.with(|w| w.set(std::ptr::null_mut()));
            PAGES.with(|p| p.borrow_mut().clear());
            HEADING_FONTS.with(|f| {
                for font in f.borrow_mut().drain(..) {
                    DeleteObject(font);
                }
            });
            0
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

unsafe extern "system" fn set_heading_font(child: HWND, font: LPARAM) -> i32 {
    let id = GetDlgCtrlID(child);
    if (IDC_HEADING..IDC_HEADING + 10).contains(&id) {
        win::set_font(child, font as HFONT);
        theme::mark_heading(child);
    }
    1
}

fn setup_headings(page: HWND) {
    unsafe {
        let base = win::font(page);
        let mut lf: LOGFONTW = std::mem::zeroed();
        GetObjectW(base, std::mem::size_of::<LOGFONTW>() as i32, &mut lf as *mut _ as *mut _);
        lf.lfWeight = FW_SEMIBOLD as i32;
        let bold = CreateFontIndirectW(&lf);
        HEADING_FONTS.with(|f| f.borrow_mut().push(bold));
        EnumChildWindows(page, Some(set_heading_font), bold as isize);
    }
}

fn setup_color_list(lv: HWND, two_columns: bool) {
    win::lv_set_ex_style(lv, LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP);
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(lv, &mut rc) };
    let w = rc.right - rc.left;
    if two_columns {
        win::lv_add_column(lv, 0, w * 65 / 100, LVCFMT_LEFT);
        win::lv_add_column(lv, 1, w - w * 65 / 100, LVCFMT_LEFT);
    } else {
        win::lv_add_column(lv, 0, w, LVCFMT_LEFT);
    }
}

fn setup_page(page: usize, hwnd: HWND) {
    setup_headings(hwnd);
    let elevated = sys::is_elevated();
    match page {
        PAGE_GENERAL => {
            win::enable(hwnd, IDC_SKIPUAC, elevated);
        }
        PAGE_MEMORY => {
            let lv = win::item(hwnd, IDC_REGIONS);
            win::lv_set_ex_style(lv, LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP);
            win::lv_add_column(lv, 0, 100, LVCFMT_LEFT);
            for (i, r) in REGIONS.iter().enumerate() {
                win::lv_add_item(lv, i as i32, &r.label(), r.mask as isize);
            }
            // fill the client width left after the vertical scrollbar appears
            win::lv_set_column_width(lv, 0, LVSCW_AUTOSIZE_USEHEADER);
            win::updown_set_range(hwnd, IDC_AUTOREDUCT_UD, 1, 99);
            win::updown_set_range(hwnd, IDC_INTERVAL_UD, 1, 1440);
            for id in [IDC_REGIONS, IDC_AUTOREDUCT_CHK, IDC_AUTOREDUCT_EDIT, IDC_AUTOREDUCT_UD, IDC_INTERVAL_CHK, IDC_INTERVAL_EDIT, IDC_INTERVAL_UD, IDC_HOTKEY_CHK, IDC_HOTKEY] {
                win::enable(hwnd, id, elevated);
            }
        }
        PAGE_APPEARANCE => {
            let lv = win::item(hwnd, IDC_TRAY_COLORS);
            setup_color_list(lv, false);
            for (i, (_, label, _)) in TRAY_COLORS.iter().enumerate() {
                win::lv_add_item(lv, i as i32, label, 0);
            }
        }
        PAGE_TRAY => {
            win::updown_set_range(hwnd, IDC_WARN_UD, 1, 100);
            win::updown_set_range(hwnd, IDC_DANGER_UD, 1, 100);
            for id in [IDC_ACTION_SC, IDC_ACTION_MC] {
                for a in TRAY_ACTIONS {
                    win::combo_add(hwnd, id, a);
                }
            }
        }
        PAGE_THEME => {
            for p in theme::PRESETS.iter() {
                win::combo_add(hwnd, IDC_THEME, p.name);
            }
            let lv = win::item(hwnd, IDC_UI_COLORS);
            setup_color_list(lv, true);
            for (i, (_, label)) in theme::COLOR_SLOTS.iter().enumerate() {
                win::lv_add_item(lv, i as i32, label, 0);
            }
        }
        _ => {}
    }
    while_loading(|| load_page(page, hwnd));
}

fn load_page(page: usize, hwnd: HWND) {
    match page {
        PAGE_GENERAL => {
            win::set_check(hwnd, IDC_ALWAYSONTOP, config::get_bool("AlwaysOnTop", false));
            win::set_check(hwnd, IDC_LOADONSTARTUP, sys::autorun_is_enabled());
            win::set_check(hwnd, IDC_STARTMINIMIZED, config::get_bool("IsStartMinimized", false));
            win::set_check(hwnd, IDC_REDUCTCONFIRM, config::get_bool("IsShowReductConfirmation", true));
            win::set_check(hwnd, IDC_SKIPUAC, config::get_bool("SkipUacEnabled", false));
        }
        PAGE_MEMORY => {
            let mask = config::get_u32("ReductMask2", MASK_DEFAULT);
            let lv = win::item(hwnd, IDC_REGIONS);
            for (i, r) in REGIONS.iter().enumerate() {
                win::lv_set_check(lv, i as i32, mask & r.mask != 0);
            }
            win::set_check(hwnd, IDC_AUTOREDUCT_CHK, config::get_bool("AutoreductEnable", false));
            win::updown_set(hwnd, IDC_AUTOREDUCT_UD, mainwnd::limit_value() as i32);
            win::set_check(hwnd, IDC_INTERVAL_CHK, config::get_bool("AutoreductIntervalEnable", false));
            win::updown_set(hwnd, IDC_INTERVAL_UD, mainwnd::interval_value() as i32);
            let hotkey_on = config::get_bool("HotkeyCleanEnable", false);
            win::set_check(hwnd, IDC_HOTKEY_CHK, hotkey_on);
            win::send(win::item(hwnd, IDC_HOTKEY), HKM_SETHOTKEY, config::get_u32("HotkeyClean", 0x0270) as usize, 0);
            win::enable(hwnd, IDC_HOTKEY, hotkey_on && sys::is_elevated());
        }
        PAGE_APPEARANCE => {
            win::set_check(hwnd, IDC_TRANSPARENT, config::get_bool("TrayUseTransparency", false));
            win::set_check(hwnd, IDC_BORDER, config::get_bool("TrayShowBorder", false));
            win::set_check(hwnd, IDC_ROUND, config::get_bool("TrayRoundCorners", false));
            win::set_check(hwnd, IDC_CHANGEBG, config::get_bool("TrayChangeBg", true));
            win::set_check(hwnd, IDC_ANTIALIAS, config::get_bool("TrayUseAntialiasing", false));
            win::set_check(hwnd, IDC_THEME_COLORS, config::get_bool("TrayUseThemeColors", false));
            win::set_item_text(hwnd, IDC_FONT, &FontSpec::load().describe());
            let lv = win::item(hwnd, IDC_TRAY_COLORS);
            for (i, (key, _, default)) in TRAY_COLORS.iter().enumerate() {
                win::lv_set_param(lv, i as i32, config::get_u32(key, *default) as isize);
            }
            win::lv_redraw(lv);
        }
        PAGE_TRAY => {
            win::updown_set(hwnd, IDC_WARN_UD, icon::warning_level() as i32);
            win::updown_set(hwnd, IDC_DANGER_UD, icon::danger_level() as i32);
            win::combo_set(hwnd, IDC_ACTION_SC, config::get_u32("TrayActionDc", 0).min(2) as i32);
            win::combo_set(hwnd, IDC_ACTION_MC, config::get_u32("TrayActionMc", 1).min(2) as i32);
            win::set_check(hwnd, IDC_SHOW_RESULT, config::get_bool("BalloonCleanResults", true));
            win::set_check(hwnd, IDC_SOUND, config::get_bool("IsNotificationsSound", true));
        }
        PAGE_THEME => {
            win::combo_set(hwnd, IDC_THEME, theme::current_index() as i32);
            let lv = win::item(hwnd, IDC_UI_COLORS);
            let p = theme::editable_palette();
            for i in 0..theme::COLOR_SLOTS.len() {
                let c = p.get(i);
                win::lv_set_param(lv, i as i32, c as isize);
                win::lv_set_text(lv, i as i32, 1, &color_to_hex(c));
            }
            win::lv_redraw(lv);
        }
        _ => {
            win::set_check(hwnd, IDC_ALLOW_STANDBY, config::get_bool("IsAllowStandbyListCleanup", false));
            win::set_check(hwnd, IDC_LOG_RESULTS, config::get_bool("LogCleanResults", false));
        }
    }
}

fn pick_color(owner: HWND, initial: u32) -> Option<u32> {
    CUSTOM_COLORS.with(|cust| {
        let mut cust = cust.borrow_mut();
        unsafe {
            let mut cc: CHOOSECOLORW = std::mem::zeroed();
            cc.lStructSize = std::mem::size_of::<CHOOSECOLORW>() as u32;
            cc.hwndOwner = owner;
            cc.rgbResult = initial;
            cc.lpCustColors = cust.as_mut_ptr();
            cc.Flags = CC_RGBINIT | CC_FULLOPEN;
            if ChooseColorW(&mut cc) != 0 {
                Some(cc.rgbResult)
            } else {
                None
            }
        }
    })
}

fn pick_font(owner: HWND) {
    let spec = FontSpec::load();
    let dpi = unsafe { GetDpiForSystem() };
    let mut lf = spec.to_logfont(dpi);
    unsafe {
        let mut cf: CHOOSEFONTW = std::mem::zeroed();
        cf.lStructSize = std::mem::size_of::<CHOOSEFONTW>() as u32;
        cf.hwndOwner = owner;
        cf.lpLogFont = &mut lf;
        cf.Flags = CF_INITTOLOGFONTSTRUCT | CF_FORCEFONTEXIST | CF_SCREENFONTS;
        if ChooseFontW(&mut cf) == 0 {
            return;
        }
        let new_spec = FontSpec {
            face: crate::util::from_wide(&lf.lfFaceName),
            size: (cf.iPointSize / 10).max(1),
            weight: lf.lfWeight,
        };
        new_spec.save();
        win::set_item_text(owner, IDC_FONT, &new_spec.describe());
    }
    mainwnd::refresh_icon();
}

fn on_page_command(page: HWND, id: i32, code: u32) {
    if loading() {
        return;
    }
    let checked = |id| win::is_checked(page, id);
    match id {
        IDC_ALWAYSONTOP => {
            config::set_bool("AlwaysOnTop", checked(id));
            let main = mainwnd::main_hwnd();
            unsafe {
                SetWindowPos(
                    main,
                    if checked(id) { HWND_TOPMOST } else { HWND_NOTOPMOST },
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
        IDC_LOADONSTARTUP => {
            sys::autorun_enable(checked(id));
            while_loading(|| win::set_check(page, id, sys::autorun_is_enabled()));
        }
        IDC_STARTMINIMIZED => config::set_bool("IsStartMinimized", checked(id)),
        IDC_REDUCTCONFIRM => config::set_bool("IsShowReductConfirmation", checked(id)),
        IDC_SKIPUAC => {
            mainwnd::set_skipuac(page, checked(id));
            while_loading(|| win::set_check(page, id, config::get_bool("SkipUacEnabled", false)));
        }
        IDC_AUTOREDUCT_CHK => config::set_bool("AutoreductEnable", checked(id)),
        IDC_INTERVAL_CHK => config::set_bool("AutoreductIntervalEnable", checked(id)),
        IDC_AUTOREDUCT_EDIT if code == EN_CHANGE => {
            config::set_u32("AutoreductValue", win::updown_get(page, IDC_AUTOREDUCT_UD).clamp(1, 99) as u32)
        }
        IDC_INTERVAL_EDIT if code == EN_CHANGE => {
            config::set_u32("AutoreductIntervalValue", win::updown_get(page, IDC_INTERVAL_UD).clamp(1, 1440) as u32)
        }
        IDC_HOTKEY_CHK => {
            config::set_bool("HotkeyCleanEnable", checked(id));
            win::enable(page, IDC_HOTKEY, checked(id));
            mainwnd::init_hotkey();
        }
        IDC_HOTKEY if code == EN_CHANGE => {
            let value = win::send(win::item(page, IDC_HOTKEY), HKM_GETHOTKEY, 0, 0) as u32 & 0xFFFF;
            config::set_u32("HotkeyClean", value);
            mainwnd::init_hotkey();
        }
        IDC_TRANSPARENT | IDC_BORDER | IDC_ROUND | IDC_CHANGEBG | IDC_ANTIALIAS | IDC_THEME_COLORS => {
            let key = match id {
                IDC_THEME_COLORS => "TrayUseThemeColors",
                IDC_TRANSPARENT => "TrayUseTransparency",
                IDC_BORDER => "TrayShowBorder",
                IDC_ROUND => "TrayRoundCorners",
                IDC_CHANGEBG => "TrayChangeBg",
                _ => "TrayUseAntialiasing",
            };
            config::set_bool(key, checked(id));
            mainwnd::refresh_icon();
        }
        IDC_FONT => pick_font(page),
        IDC_WARN_EDIT | IDC_DANGER_EDIT if code == EN_CHANGE => {
            let (key, ud) = if id == IDC_WARN_EDIT { ("TrayLevelWarning", IDC_WARN_UD) } else { ("TrayLevelDanger", IDC_DANGER_UD) };
            config::set_u32(key, win::updown_get(page, ud).clamp(1, 100) as u32);
            mainwnd::refresh_icon();
        }
        IDC_ACTION_SC | IDC_ACTION_MC if code == CBN_SELCHANGE => {
            let key = if id == IDC_ACTION_SC { "TrayActionDc" } else { "TrayActionMc" };
            config::set_u32(key, win::combo_get(page, id).max(0) as u32);
        }
        IDC_SHOW_RESULT => config::set_bool("BalloonCleanResults", checked(id)),
        IDC_SOUND => config::set_bool("IsNotificationsSound", checked(id)),
        IDC_THEME if code == CBN_SELCHANGE => {
            let i = win::combo_get(page, id).max(0) as usize;
            theme::set_theme(theme::PRESETS[i.min(theme::PRESETS.len() - 1)].key);
            mainwnd::apply_theme_everywhere();
            reload_page_values();
        }
        IDC_RESET_CUSTOM => {
            for (key, _) in theme::COLOR_SLOTS {
                config::remove(key);
            }
            theme::reload();
            mainwnd::apply_theme_everywhere();
            reload_page_values();
        }
        IDC_ALLOW_STANDBY => config::set_bool("IsAllowStandbyListCleanup", checked(id)),
        IDC_LOG_RESULTS => config::set_bool("LogCleanResults", checked(id)),
        IDC_OPEN_CONFIG => {
            let dir = config::dir();
            let _ = std::fs::create_dir_all(&dir);
            sys::shell_open(&dir.to_string_lossy());
        }
        IDC_OPEN_LOG => {
            let log = config::dir().join("memreduct.log");
            if log.exists() {
                sys::shell_open(&log.to_string_lossy());
            } else {
                win::message_box(page, "The log file does not exist yet.", APP_NAME, MB_OK | MB_ICONINFORMATION);
            }
        }
        _ => return,
    }
    mainwnd::update_menu_checks();
}

/// Paints color rows with their own color (tray colors, UI colors) and
/// greys out regions not supported on this Windows version.
unsafe fn page_custom_draw(cd: &mut NMLVCUSTOMDRAW) -> isize {
    let id = cd.nmcd.hdr.idFrom as i32;
    match cd.nmcd.dwDrawStage {
        CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as isize,
        CDDS_ITEMPREPAINT => {
            let row = cd.nmcd.dwItemSpec;
            match id {
                IDC_TRAY_COLORS | IDC_UI_COLORS => {
                    let color = cd.nmcd.lItemlParam as u32;
                    cd.clrTextBk = color;
                    cd.clrText = contrast_text(color);
                    CDRF_NEWFONT as isize
                }
                IDC_REGIONS => {
                    if let Some(p) = theme::palette() {
                        cd.clrTextBk = p.control;
                    }
                    let supported = REGIONS.get(row).map(|r| r.supported()).unwrap_or(true);
                    cd.clrText = if supported {
                        theme::text_color()
                    } else {
                        crate::util::blend(theme::text_color(), cd.clrTextBk, 0.55)
                    };
                    CDRF_NEWFONT as isize
                }
                _ => CDRF_DODEFAULT as isize,
            }
        }
        _ => CDRF_DODEFAULT as isize,
    }
}

unsafe fn on_page_notify(page: HWND, lparam: LPARAM) -> isize {
    let hdr = &*(lparam as *const NMHDR);
    let id = hdr.idFrom as i32;
    match hdr.code {
        NM_CUSTOMDRAW => {
            if win::class_name(hdr.hwndFrom).eq_ignore_ascii_case("SysListView32") {
                return win::dlg_result(page, page_custom_draw(&mut *(lparam as *mut NMLVCUSTOMDRAW)));
            }
            match theme::custom_draw_button(&*(lparam as *const NMCUSTOMDRAW)) {
                Some(r) => win::dlg_result(page, r),
                None => 0,
            }
        }
        LVN_ITEMCHANGED if id == IDC_REGIONS && !loading() => {
            let nm = &*(lparam as *const NMLISTVIEW);
            if nm.uChanged & LVIF_STATE == 0 || nm.iItem < 0 {
                return 0;
            }
            let new_on = (nm.uNewState & LVIS_STATEIMAGEMASK) >> 12 == 2;
            let old_on = (nm.uOldState & LVIS_STATEIMAGEMASK) >> 12 == 2;
            if (nm.uOldState & LVIS_STATEIMAGEMASK) == 0 || new_on == old_on {
                return 0;
            }
            let region = &REGIONS[nm.iItem as usize];
            let revert = |on: bool| while_loading(|| win::lv_set_check(hdr.hwndFrom, nm.iItem, on));
            if !region.supported() {
                revert(old_on);
                return 0;
            }
            if new_on && region.mask & MASK_FREEZES != 0 && !mainwnd::confirm_freeze(page) {
                revert(false);
                return 0;
            }
            let mask = config::get_u32("ReductMask2", MASK_DEFAULT);
            config::set_u32("ReductMask2", if new_on { mask | region.mask } else { mask & !region.mask });
            0
        }
        NM_DBLCLK if id == IDC_TRAY_COLORS || id == IDC_UI_COLORS => {
            let nm = &*(lparam as *const NMITEMACTIVATE);
            if nm.iItem < 0 {
                return 0;
            }
            let row = nm.iItem as usize;
            let current = win::lv_get_param(hdr.hwndFrom, nm.iItem) as u32;
            if let Some(color) = pick_color(page, current) {
                if id == IDC_TRAY_COLORS {
                    config::set_u32(TRAY_COLORS[row].0, color);
                    win::lv_set_param(hdr.hwndFrom, nm.iItem, color as isize);
                    win::lv_redraw(hdr.hwndFrom);
                    mainwnd::refresh_icon();
                } else {
                    theme::set_color(row, color);
                    mainwnd::apply_theme_everywhere();
                    reload_page_values();
                }
            }
            0
        }
        _ => 0,
    }
}

unsafe extern "system" fn page_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            // setting ranges/items fires EN_CHANGE etc.; none of that is user input
            while_loading(|| setup_page(lparam as usize, hwnd));
            // mark ready (index + 1, so page 0 is non-zero too)
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam + 1);
            0
        }
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            theme::ctl_color(msg, wparam as HDC, lparam as HWND).unwrap_or(0)
        }
        WM_NOTIFY => on_page_notify(hwnd, lparam),
        WM_COMMAND => {
            // up-down controls update their buddy edits while the dialog is
            // still being created, before WM_INITDIALOG: ignore those
            if GetWindowLongPtrW(hwnd, GWLP_USERDATA) != 0 {
                on_page_command(hwnd, win::loword(wparam) as i32, win::hiword(wparam) as u32);
            }
            0
        }
        _ => 0,
    }
}
