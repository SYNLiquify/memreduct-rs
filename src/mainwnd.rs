//! Main window (memory statistics), tray icon handling, cleaning workflow,
//! auto-clean timer and the global hotkey.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config;
use crate::dlg::{self as d, DialogTemplate};
use crate::icon;
use crate::memory::{self, MemInfo, MASK_DEFAULT, MASK_FREEZES, REGIONS};
use crate::settings;
use crate::sys::{self, APP_NAME};
use crate::theme;
use crate::tray;
use crate::util::{format_bytes, log, unix_now, wide, LogLevel};
use crate::win;

// Control and command ids ---------------------------------------------------

pub const IDC_LISTVIEW: i32 = 100;
pub const IDC_CLEAN: i32 = 101;

pub const IDM_SETTINGS: i32 = 201;
pub const IDM_EXIT: i32 = 202;
pub const IDM_ALWAYSONTOP: i32 = 203;
pub const IDM_REDUCTCONFIRM: i32 = 204;
pub const IDM_LOADONSTARTUP: i32 = 205;
pub const IDM_STARTMINIMIZED: i32 = 206;
pub const IDM_SKIPUAC: i32 = 207;
pub const IDM_WEBSITE: i32 = 208;
pub const IDM_ABOUT: i32 = 209;
pub const IDM_OPENCONFIG: i32 = 210;
pub const IDM_CUSTOMIZE_COLORS: i32 = 211;
pub const IDM_REQUIREADMIN: i32 = 212;
pub const IDM_CHECKUPDATES: i32 = 213;
pub const IDM_THEME_BASE: i32 = 220; // + preset index

pub const IDM_TRAY_SHOW: i32 = 300;
pub const IDM_TRAY_CLEAN: i32 = 301;
pub const IDM_TRAY_DISABLE_LIMIT: i32 = 302;
pub const IDM_TRAY_DISABLE_INTERVAL: i32 = 303;

pub const IDM_REGION_TOGGLE_BASE: i32 = 320; // + region index
pub const IDM_REGION_CLEAN_BASE: i32 = 340; // + region index
pub const IDX_LIMIT_BASE: i32 = 400; // + value index
pub const IDX_INTERVAL_BASE: i32 = 430; // + value index

const IDOK: i32 = 1;
const IDCANCEL: i32 = 2;

const NIN_KEYSELECT: u32 = WM_USER + 1;
const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;

pub const WM_TRAY: u32 = WM_APP + 1;
pub const WM_CLEAN_DONE: u32 = WM_APP + 2;

const TIMER_ID: usize = 1337;
const HOTKEY_ID: i32 = 1337;

const DEFAULT_LIMIT: u32 = 90;
const DEFAULT_INTERVAL: u32 = 30;
/// Minimum pause between usage-based automatic cleanups (seconds).
const AUTOREDUCT_COOLDOWN: i64 = 30;

const MENU_TITLES: [&str; 4] = ["&File", "&View", "&Settings", "&Help"];
const GROUPS: [&str; 3] = ["Physical memory", "Pagefile", "System working set"];
const ROWS: [&str; 3] = ["Usage", "Available", "Total"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Auto = 0,
    Manual = 1,
    Hotkey = 2,
    Region = 3,
}

impl Source {
    fn from(v: usize) -> Self {
        match v {
            0 => Source::Auto,
            2 => Source::Hotkey,
            3 => Source::Region,
            _ => Source::Manual,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Source::Auto => "Cleanup (Auto)",
            Source::Manual => "Cleanup (Manual)",
            Source::Hotkey => "Cleanup (Hotkey)",
            Source::Region => "Cleanup (Single region)",
        }
    }
}

thread_local! {
    static MAIN: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static RENDERER: RefCell<Option<icon::Renderer>> = const { RefCell::new(None) };
    static TRAY_ICON: Cell<HICON> = const { Cell::new(std::ptr::null_mut()) };
    static LAST_PERCENT: Cell<u32> = const { Cell::new(u32::MAX) };
    static GROUP_PERCENT: Cell<[u32; 3]> = const { Cell::new([0; 3]) };
    static BOLD_FONT: Cell<(HFONT, HFONT)> = const { Cell::new((std::ptr::null_mut(), std::ptr::null_mut())) };
    static CLEANING: Cell<bool> = const { Cell::new(false) };
    static LIMITS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static INTERVALS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static APP_ICONS: Cell<(HICON, HICON)> = const { Cell::new((std::ptr::null_mut(), std::ptr::null_mut())) };
    static MSG_TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static MSG_ACTIVATE: Cell<u32> = const { Cell::new(0) };
}

pub fn main_hwnd() -> HWND {
    MAIN.with(|m| m.get())
}

pub fn limit_value() -> u32 {
    config::get_u32("AutoreductValue", DEFAULT_LIMIT).min(100)
}

pub fn interval_value() -> u32 {
    config::get_u32("AutoreductIntervalValue", DEFAULT_INTERVAL).clamp(1, 1440)
}

pub fn activate_message() -> u32 {
    MSG_ACTIVATE.with(|m| {
        if m.get() == 0 {
            let name = wide(if cfg!(debug_assertions) { "memreduct-rs-activate-debug" } else { "memreduct-rs-activate" });
            m.set(unsafe { RegisterWindowMessageW(name.as_ptr()) });
        }
        m.get()
    })
}

pub fn instance_prop() -> Vec<u16> {
    wide(if cfg!(debug_assertions) { "memreduct-rs-main-debug" } else { "memreduct-rs-main" })
}

// ---------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------

pub fn create() -> HWND {
    let mut t = DialogTemplate::new(
        d::WS_POPUP | d::WS_CAPTION | d::WS_SYSMENU | d::WS_MINIMIZEBOX | d::DS_MODALFRAME | d::WS_CLIPCHILDREN,
        0,
        0,
        0,
        224,
        214,
        APP_NAME,
    );
    t.listview(IDC_LISTVIEW, 0, 8, 8, 208, 174);
    t.item(d::CLASS_BUTTON, "Clean memory", IDC_CLEAN, d::BS_SPLITBUTTON | d::WS_TABSTOP, 0, 8, 192, 208, 15);
    let tpl = t.build();
    unsafe {
        CreateDialogIndirectParamW(
            GetModuleHandleW(std::ptr::null()),
            tpl.as_ptr() as *const DLGTEMPLATE,
            std::ptr::null_mut(),
            Some(dlg_proc),
            0,
        )
    }
}

fn load_app_icons(hwnd: HWND) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let hinst = GetModuleHandleW(std::ptr::null());
        let big_size = GetSystemMetricsForDpi(SM_CXICON, dpi);
        let small_size = GetSystemMetricsForDpi(SM_CXSMICON, dpi);
        let mut big: HICON = std::ptr::null_mut();
        let mut small: HICON = std::ptr::null_mut();
        LoadIconWithScaleDown(hinst, 1 as _, big_size, big_size, &mut big);
        LoadIconWithScaleDown(hinst, 1 as _, small_size, small_size, &mut small);
        let (old_big, old_small) = APP_ICONS.with(|c| c.replace((big, small)));
        win::send(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
        win::send(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
        icon::destroy(old_big);
        icon::destroy(old_small);
    }
}

fn tray_dpi() -> u32 {
    unsafe {
        let class = wide("Shell_TrayWnd");
        let taskbar = FindWindowW(class.as_ptr(), std::ptr::null());
        if !taskbar.is_null() {
            let dpi = GetDpiForWindow(taskbar);
            if dpi != 0 {
                return dpi;
            }
        }
        GetDpiForSystem()
    }
}

/// Rebuilds the tray icon renderer (font, size) and redraws the icon.
pub fn refresh_icon() {
    RENDERER.with(|r| *r.borrow_mut() = Some(icon::Renderer::new(tray_dpi())));
    LAST_PERCENT.with(|l| l.set(u32::MAX));
    let hwnd = main_hwnd();
    if !hwnd.is_null() {
        on_timer(hwnd, false);
        win::lv_redraw(win::item(hwnd, IDC_LISTVIEW));
    }
}

fn render_tray_icon(percent: u32) -> HICON {
    RENDERER.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_none() {
            *r = Some(icon::Renderer::new(tray_dpi()));
        }
        r.as_ref().unwrap().render(&percent.to_string(), &icon::Style::for_percent(percent))
    })
}

fn add_tray(hwnd: HWND) {
    let info = memory::query();
    let ico = render_tray_icon(info.physical.percent);
    let old = TRAY_ICON.with(|t| t.replace(ico));
    tray::add(hwnd, WM_TRAY, ico, APP_NAME);
    icon::destroy(old);
    LAST_PERCENT.with(|l| l.set(info.physical.percent));
    on_timer(hwnd, false);
}

pub fn init_hotkey() {
    let hwnd = main_hwnd();
    if hwnd.is_null() {
        return;
    }
    unsafe { UnregisterHotKey(hwnd, HOTKEY_ID) };
    if !sys::is_elevated() || !config::get_bool("HotkeyCleanEnable", false) {
        return;
    }
    // HOTKEYF_* in the high byte, virtual key in the low byte
    let hotkey = config::get_u32("HotkeyClean", 0x0270 /* Ctrl+F1 */);
    let vk = hotkey & 0xFF;
    let hk = (hotkey >> 8) & 0xFF;
    if vk == 0 {
        return;
    }
    let mods = (hk & 2) | ((hk & 4) >> 2) | ((hk & 1) << 2) | 0x4000 /* MOD_NOREPEAT */;
    if unsafe { RegisterHotKey(hwnd, HOTKEY_ID, mods as HOT_KEY_MODIFIERS, vk) } == 0 {
        win::message_box(hwnd, "Could not register the global hotkey. It may already be used by another program.", APP_NAME, MB_OK | MB_ICONWARNING);
    }
}

fn init_listview(hwnd: HWND) {
    let lv = win::item(hwnd, IDC_LISTVIEW);
    theme::mark_plain(lv);
    win::lv_set_ex_style(lv, LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP);
    win::lv_add_column(lv, 0, 100, LVCFMT_LEFT);
    win::lv_add_column(lv, 1, 100, LVCFMT_RIGHT);
    let mut row = 0;
    for g in GROUPS {
        win::lv_add_item(lv, row, g, 0);
        row += 1;
        for r in ROWS {
            win::lv_add_item(lv, row, &format!("    {}", r), 0);
            row += 1;
        }
    }
    resize_columns(hwnd);
}

fn resize_columns(hwnd: HWND) {
    let lv = win::item(hwnd, IDC_LISTVIEW);
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(lv, &mut rc) };
    let w = rc.right - rc.left;
    win::lv_set_column_width(lv, 0, w / 2);
    win::lv_set_column_width(lv, 1, w - w / 2);
}

/// Bold variant of the list view font for group header rows.
fn bold_font(lv: HWND) -> HFONT {
    let base = win::font(lv);
    let (cached_base, cached_bold) = BOLD_FONT.with(|b| b.get());
    if cached_base == base && !cached_bold.is_null() {
        return cached_bold;
    }
    unsafe {
        let mut lf: LOGFONTW = std::mem::zeroed();
        GetObjectW(base, std::mem::size_of::<LOGFONTW>() as i32, &mut lf as *mut _ as *mut _);
        lf.lfWeight = FW_BOLD as i32;
        let bold = CreateFontIndirectW(&lf);
        if !cached_bold.is_null() {
            DeleteObject(cached_bold);
        }
        BOLD_FONT.with(|b| b.set((base, bold)));
        bold
    }
}

// ---------------------------------------------------------------------------
// Menus
// ---------------------------------------------------------------------------

fn append(menu: HMENU, flags: u32, id: usize, text: &str) {
    let w = wide(text);
    unsafe { AppendMenuW(menu, flags, id, w.as_ptr()) };
}

fn separator(menu: HMENU) {
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null()) };
}

fn check(menu: HMENU, id: i32, on: bool) {
    unsafe { CheckMenuItem(menu, id as u32, MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED }) };
}

fn enable_menu(menu: HMENU, id: i32, on: bool) {
    unsafe { EnableMenuItem(menu, id as u32, MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED }) };
}

/// (Re)builds the menu bar. Themed menus owner-draw the top-level items.
pub fn build_menu(hwnd: HWND) {
    unsafe {
        let themed = theme::palette().is_some();
        let bar = CreateMenu();

        let file = CreatePopupMenu();
        append(file, MF_STRING, IDM_SETTINGS as usize, "&Settings...\tF2");
        separator(file);
        append(file, MF_STRING, IDM_EXIT as usize, "E&xit");

        let themes = CreatePopupMenu();
        for (i, p) in theme::PRESETS.iter().enumerate() {
            append(themes, MF_STRING, (IDM_THEME_BASE + i as i32) as usize, p.name);
        }
        separator(themes);
        append(themes, MF_STRING, IDM_CUSTOMIZE_COLORS as usize, "Customize colors...");

        let view = CreatePopupMenu();
        append(view, MF_STRING, IDM_ALWAYSONTOP as usize, "Always on top");
        append(view, MF_STRING, IDM_REDUCTCONFIRM as usize, "Confirm memory cleaning start");
        separator(view);
        append(view, MF_POPUP, themes as usize, "&Theme");

        let settings_menu = CreatePopupMenu();
        append(settings_menu, MF_STRING, IDM_LOADONSTARTUP as usize, "Load on system startup");
        append(settings_menu, MF_STRING, IDM_STARTMINIMIZED as usize, "Start minimized");
        append(settings_menu, MF_STRING, IDM_REQUIREADMIN as usize, "Require administrator rights");
        append(settings_menu, MF_STRING, IDM_SKIPUAC as usize, "Skip \"User Account Control\" prompt warning");
        separator(settings_menu);
        append(settings_menu, MF_STRING, IDM_OPENCONFIG as usize, "Open configuration folder");

        let help = CreatePopupMenu();
        append(help, MF_STRING, IDM_WEBSITE as usize, "Website");
        append(help, MF_STRING, IDM_CHECKUPDATES as usize, "Check for updates");
        append(help, MF_STRING, IDM_ABOUT as usize, "&About\tF1");

        for (i, sub) in [file, view, settings_menu, help].into_iter().enumerate() {
            append(bar, MF_POPUP, sub as usize, MENU_TITLES[i]);
        }
        if themed {
            theme::prepare_menu(bar, true);
        }

        let old = GetMenu(hwnd);
        SetMenu(hwnd, bar);
        theme::destroy_menu(old);
        DrawMenuBar(hwnd);
    }
    update_menu_checks();
}

pub fn update_menu_checks() {
    let hwnd = main_hwnd();
    if hwnd.is_null() {
        return;
    }
    let menu = unsafe { GetMenu(hwnd) };
    if menu.is_null() {
        return;
    }
    check(menu, IDM_ALWAYSONTOP, config::get_bool("AlwaysOnTop", false));
    check(menu, IDM_REDUCTCONFIRM, config::get_bool("IsShowReductConfirmation", true));
    check(menu, IDM_LOADONSTARTUP, sys::autorun_is_enabled());
    check(menu, IDM_STARTMINIMIZED, config::get_bool("IsStartMinimized", false));
    check(menu, IDM_REQUIREADMIN, config::get_bool("RequireAdmin", true));
    check(menu, IDM_SKIPUAC, config::get_bool("SkipUacEnabled", false));
    enable_menu(menu, IDM_SKIPUAC, sys::is_elevated());
    let current = theme::current_index();
    for i in 0..theme::PRESETS.len() {
        check(menu, IDM_THEME_BASE + i as i32, i == current);
    }
}

/// Values offered by the "Clean when above" / "Clean every" submenus:
/// 10..80 in steps of 10 plus the neighbourhood of the current value.
pub fn menu_values(value: u32, max: u32) -> Vec<u32> {
    let mut set: BTreeSet<u32> = (1..=8).map(|i| i * 10).collect();
    for v in value.saturating_sub(2).max(1)..=value + 2 {
        if v >= 5 {
            set.insert(v);
        }
    }
    set.into_iter().filter(|&v| v <= max).take(13).collect()
}

fn values_submenu(values: &[u32], base: i32, disable_id: i32, current: u32, enabled: bool, suffix: &str) -> HMENU {
    unsafe {
        let menu = CreatePopupMenu();
        append(menu, MF_STRING, disable_id as usize, "Disable");
        separator(menu);
        for (i, v) in values.iter().enumerate() {
            let id = base + i as i32;
            append(menu, MF_STRING, id as usize, &format!("{}{}", v, suffix));
            check(menu, id, enabled && *v == current);
            enable_menu(menu, id, sys::is_elevated());
        }
        check(menu, disable_id, !enabled);
        menu
    }
}

fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        append(menu, MF_STRING, IDM_TRAY_SHOW as usize, "Show/Hide");
        SetMenuDefaultItem(menu, IDM_TRAY_SHOW as u32, 0);
        separator(menu);
        append(menu, MF_STRING, IDM_TRAY_CLEAN as usize, "Clean memory...");
        separator(menu);

        let regions = CreatePopupMenu();
        let mask = config::get_u32("ReductMask2", MASK_DEFAULT);
        for (i, r) in REGIONS.iter().enumerate() {
            let id = IDM_REGION_TOGGLE_BASE + i as i32;
            append(regions, MF_STRING, id as usize, &r.label());
            check(regions, id, mask & r.mask != 0);
            enable_menu(regions, id, sys::is_elevated() && r.supported());
        }
        append(menu, MF_POPUP, regions as usize, "Clean areas");

        let limits = menu_values(limit_value(), 99);
        let limit_menu = values_submenu(
            &limits,
            IDX_LIMIT_BASE,
            IDM_TRAY_DISABLE_LIMIT,
            limit_value(),
            config::get_bool("AutoreductEnable", false),
            "%",
        );
        LIMITS.with(|l| *l.borrow_mut() = limits);
        append(menu, MF_POPUP, limit_menu as usize, "Clean when above");

        let intervals = menu_values(interval_value(), 1440);
        let interval_menu = values_submenu(
            &intervals,
            IDX_INTERVAL_BASE,
            IDM_TRAY_DISABLE_INTERVAL,
            interval_value(),
            config::get_bool("AutoreductIntervalEnable", false),
            " min.",
        );
        INTERVALS.with(|l| *l.borrow_mut() = intervals);
        append(menu, MF_POPUP, interval_menu as usize, "Clean every");

        separator(menu);
        append(menu, MF_STRING, IDM_SETTINGS as usize, "Settings...");
        separator(menu);
        append(menu, MF_STRING, IDM_WEBSITE as usize, "Website");
        append(menu, MF_STRING, IDM_CHECKUPDATES as usize, "Check for updates");
        append(menu, MF_STRING, IDM_ABOUT as usize, "About");
        separator(menu);
        append(menu, MF_STRING, IDM_EXIT as usize, "Exit");

        theme::prepare_menu(menu, false);

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        TrackPopupMenuEx(menu, TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN, pt.x, pt.y, hwnd, std::ptr::null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        theme::destroy_menu(menu);
    }
}

fn show_region_menu(hwnd: HWND, button: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        for (i, r) in REGIONS.iter().enumerate() {
            let id = IDM_REGION_CLEAN_BASE + i as i32;
            append(menu, MF_STRING, id as usize, &r.label());
            enable_menu(menu, id, r.supported());
        }
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(button, &mut rc);
        theme::prepare_menu(menu, false);
        TrackPopupMenuEx(menu, TPM_LEFTALIGN | TPM_TOPALIGN, rc.left, rc.bottom, hwnd, std::ptr::null());
        theme::destroy_menu(menu);
    }
}

// ---------------------------------------------------------------------------
// Window visibility
// ---------------------------------------------------------------------------

fn save_position(hwnd: HWND) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut rc) };
    config::set_string("WindowPosition", &format!("{},{}", rc.left, rc.top));
}

fn restore_position(hwnd: HWND) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let saved = config::get_string("WindowPosition", "");
        let pos = saved.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<i32>().ok()?, y.trim().parse::<i32>().ok()?)));
        if let Some((x, y)) = pos {
            let target = RECT { left: x, top: y, right: x + w, bottom: y + h };
            if !MonitorFromRect(&target, MONITOR_DEFAULTTONULL).is_null() {
                SetWindowPos(hwnd, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                return;
            }
        }
        // default: bottom-right corner of the primary work area, near the tray
        let mut work: RECT = std::mem::zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut _ as *mut _, 0);
        let margin = 12;
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            work.right - w - margin,
            work.bottom - h - margin,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn show(hwnd: HWND) {
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            restore_position(hwnd);
        }
        apply_topmost(hwnd);
        ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
        SetForegroundWindow(hwnd);
    }
    on_timer(hwnd, false);
}

pub fn toggle(hwnd: HWND) {
    unsafe {
        if IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 {
            save_position(hwnd);
            ShowWindow(hwnd, SW_HIDE);
        } else {
            show(hwnd);
        }
    }
}

fn apply_topmost(hwnd: HWND) {
    let top = config::get_bool("AlwaysOnTop", false);
    unsafe {
        SetWindowPos(
            hwnd,
            if top { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Re-applies the theme to every open window after a theme change.
pub fn apply_theme_everywhere() {
    theme::reload();
    let hwnd = main_hwnd();
    if !hwnd.is_null() {
        build_menu(hwnd);
        theme::apply(hwnd);
        if config::get_bool("TrayUseThemeColors", false) {
            refresh_icon();
        }
    }
    settings::apply_theme();
}

// ---------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------

/// Yes/No task dialog with a "Do not ask again" box.
/// Returns (accepted, dont_ask_again).
pub fn confirm(hwnd: HWND, main: &str, content: &str, warning: bool) -> (bool, bool) {
    let title = wide(APP_NAME);
    let main_w = wide(main);
    let content_w = wide(content);
    let verify = wide("Do not ask again");
    unsafe {
        let mut cfg: TASKDIALOGCONFIG = std::mem::zeroed();
        cfg.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        cfg.hwndParent = hwnd;
        let mut flags = TDF_ALLOW_DIALOG_CANCELLATION;
        if !hwnd.is_null() && IsWindowVisible(hwnd) != 0 {
            flags |= TDF_POSITION_RELATIVE_TO_WINDOW;
        }
        cfg.dwFlags = flags;
        cfg.dwCommonButtons = TDCBF_YES_BUTTON | TDCBF_NO_BUTTON;
        cfg.pszWindowTitle = title.as_ptr();
        cfg.Anonymous1.pszMainIcon = if warning { TD_WARNING_ICON } else { TD_INFORMATION_ICON };
        cfg.pszMainInstruction = main_w.as_ptr();
        if !content.is_empty() {
            cfg.pszContent = content_w.as_ptr();
        }
        cfg.pszVerificationText = verify.as_ptr();

        let mut button = 0i32;
        let mut checked = 0i32;
        if TaskDialogIndirect(&cfg, &mut button, std::ptr::null_mut(), &mut checked) < 0 {
            // fallback when comctl32 v6 is unavailable
            let text = if content.is_empty() { main.to_string() } else { format!("{}\r\n\r\n{}", main, content) };
            let r = win::message_box(hwnd, &text, APP_NAME, MB_YESNO | if warning { MB_ICONWARNING } else { MB_ICONQUESTION });
            return (r == IDYES, false);
        }
        (button == IDYES, checked != 0)
    }
}

unsafe extern "system" fn about_callback(_: HWND, msg: TASKDIALOG_NOTIFICATIONS, _: WPARAM, lparam: LPARAM, _: isize) -> i32 {
    if msg == TDN_HYPERLINK_CLICKED {
        let href = lparam as *const u16;
        let mut len = 0;
        while *href.add(len) != 0 {
            len += 1;
        }
        sys::shell_open(&String::from_utf16_lossy(std::slice::from_raw_parts(href, len)));
    }
    0
}

pub fn about(hwnd: HWND) {
    let (major, minor, build) = sys::os_version();
    let title = wide("About");
    let main = wide(&format!("{} {}", APP_NAME, sys::APP_VERSION));
    let content = wide(&format!(
        "Rust rewrite by {author}.\n<a href=\"{url}\">{url}</a>\n\n\
         Based on Mem Reduct, created by {orig}, the original author.\n\
         Licensed under the GNU General Public License v3.0.",
        author = sys::APP_AUTHOR,
        url = sys::REPO_URL,
        orig = sys::ORIGINAL_AUTHOR,
    ));
    let footer = wide(&format!(
        "Administrator: {} | Windows {}.{} build {}\nSettings: {}",
        if sys::is_elevated() { "yes" } else { "no" },
        major,
        minor,
        build,
        config::path().display(),
    ));
    unsafe {
        let mut cfg: TASKDIALOGCONFIG = std::mem::zeroed();
        cfg.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        cfg.hwndParent = hwnd;
        cfg.dwFlags = TDF_ENABLE_HYPERLINKS | TDF_ALLOW_DIALOG_CANCELLATION | TDF_USE_HICON_MAIN;
        if IsWindowVisible(hwnd) != 0 {
            cfg.dwFlags |= TDF_POSITION_RELATIVE_TO_WINDOW;
        }
        cfg.dwCommonButtons = TDCBF_CLOSE_BUTTON;
        cfg.pszWindowTitle = title.as_ptr();
        cfg.Anonymous1.hMainIcon = APP_ICONS.with(|c| c.get().0);
        cfg.pszMainInstruction = main.as_ptr();
        cfg.pszContent = content.as_ptr();
        cfg.pszFooter = footer.as_ptr();
        cfg.pfCallback = Some(about_callback);
        if TaskDialogIndirect(&cfg, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) < 0 {
            let text = format!(
                "{} {}\r\n\r\nRust rewrite by {}.\r\n{}\r\n\r\nBased on Mem Reduct by {}.\r\nLicensed under GPL-3.0.",
                APP_NAME,
                sys::APP_VERSION,
                sys::APP_AUTHOR,
                sys::REPO_URL,
                sys::ORIGINAL_AUTHOR
            );
            win::message_box(hwnd, &text, "About", MB_OK | MB_ICONINFORMATION);
        }
    }
}

fn no_privileges(hwnd: HWND) {
    win::message_box(
        hwnd,
        "Administrator rights are required to clean memory. Please run the program as administrator.",
        APP_NAME,
        MB_OK | MB_ICONSTOP,
    );
}

/// Relaunches elevated. On success the caller should close this instance.
pub fn restart_elevated(hwnd: HWND) -> bool {
    crate::release_instance_mutex();
    let visible = !hwnd.is_null() && unsafe { IsWindowVisible(hwnd) } != 0;
    if sys::run_as_admin(hwnd, if visible { "" } else { "-minimized" }) {
        true
    } else {
        crate::acquire_instance_mutex();
        false
    }
}

// ---------------------------------------------------------------------------
// Cleaning
// ---------------------------------------------------------------------------

pub fn request_clean(hwnd: HWND, src: Source, mask_override: Option<u32>) {
    if !sys::is_elevated() {
        if src == Source::Auto {
            return;
        }
        if restart_elevated(hwnd) {
            unsafe { DestroyWindow(hwnd) };
        } else {
            no_privileges(hwnd);
        }
        return;
    }

    if CLEANING.with(|c| c.get()) {
        return;
    }

    let mut mask = mask_override.unwrap_or_else(|| config::get_u32("ReductMask2", MASK_DEFAULT));
    if src == Source::Auto && !config::get_bool("IsAllowStandbyListCleanup", false) {
        mask &= !MASK_FREEZES; // freezing regions are never auto-cleaned unless allowed
    }
    if mask == 0 {
        return;
    }

    if src == Source::Manual && config::get_bool("IsShowReductConfirmation", true) {
        let (ok, dont_ask) = confirm(
            hwnd,
            "Are you sure you want to clean the memory?",
            &memory::describe_mask(mask),
            false,
        );
        if dont_ask && ok {
            config::set_bool("IsShowReductConfirmation", false);
            update_menu_checks();
        }
        if !ok {
            return;
        }
    }

    CLEANING.with(|c| c.set(true));
    let target = hwnd as usize;
    std::thread::spawn(move || {
        let result = memory::clean(mask);
        let ptr = Box::into_raw(Box::new(result));
        unsafe {
            if PostMessageW(target as HWND, WM_CLEAN_DONE, src as usize, ptr as isize) == 0 {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

fn finish_clean(hwnd: HWND, src: Source, result: memory::CleanResult) {
    CLEANING.with(|c| c.set(false));
    config::set_i64("StatisticLastReduct", unix_now());

    let freed = format_bytes(result.freed);
    for e in &result.errors {
        log(LogLevel::Error, e);
    }
    if config::get_bool("LogCleanResults", false) {
        log(LogLevel::Info, &format!("{}: {} released", src.reason(), freed));
    }

    let regions: Vec<&str> = REGIONS.iter().filter(|r| result.mask & r.mask != 0).map(|r| r.name).collect();
    let mut text = format!("Memory was released with {} result.\nCleaned: {}.", freed, regions.join(", "));
    if !result.errors.is_empty() {
        text.push_str(&format!("\n{} step(s) failed, see memreduct.log.", result.errors.len()));
    }

    let mut shown = false;
    if config::get_bool("BalloonCleanResults", true) {
        shown = tray::balloon(
            hwnd,
            APP_NAME,
            &text,
            !result.errors.is_empty(),
            config::get_bool("IsNotificationsSound", true),
        );
    }
    if !shown && src == Source::Region {
        win::message_box(hwnd, &text.replace('\n', "\r\n"), "Memory cleaning", MB_OK | MB_ICONINFORMATION);
    }

    LAST_PERCENT.with(|l| l.set(u32::MAX));
    on_timer(hwnd, false);
}

// ---------------------------------------------------------------------------
// Periodic update
// ---------------------------------------------------------------------------

fn should_autoclean(info: &MemInfo) -> bool {
    let since = unix_now() - config::get_i64("StatisticLastReduct", 0);
    if config::get_bool("AutoreductEnable", false)
        && info.physical.percent >= limit_value()
        && since >= AUTOREDUCT_COOLDOWN
    {
        return true;
    }
    config::get_bool("AutoreductIntervalEnable", false) && since >= interval_value() as i64 * 60
}

fn on_timer(hwnd: HWND, allow_autoclean: bool) {
    let info = memory::query();

    if allow_autoclean && sys::is_elevated() && !CLEANING.with(|c| c.get()) && should_autoclean(&info) {
        request_clean(hwnd, Source::Auto, None);
    }
    if allow_autoclean {
        crate::update::maybe_auto_check(hwnd);
    }

    let percent = info.physical.percent;
    let new_icon = if LAST_PERCENT.with(|l| l.get()) != percent {
        LAST_PERCENT.with(|l| l.set(percent));
        Some(render_tray_icon(percent))
    } else {
        None
    };
    let tip = format!(
        "{}: {:.1}%\n{}: {:.1}%\n{}: {:.1}%",
        GROUPS[0], info.physical.percent_f, GROUPS[1], info.page_file.percent_f, GROUPS[2], info.system_cache.percent_f
    );
    tray::update(hwnd, new_icon, &tip);
    if let Some(ico) = new_icon {
        let old = TRAY_ICON.with(|t| t.replace(ico));
        icon::destroy(old);
    }

    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return;
    }

    GROUP_PERCENT.with(|g| g.set([info.physical.percent, info.page_file.percent, info.system_cache.percent]));
    let lv = win::item(hwnd, IDC_LISTVIEW);
    for (g, usage) in [info.physical, info.page_file, info.system_cache].iter().enumerate() {
        let base = (g * 4) as i32;
        win::lv_set_text(lv, base + 1, 1, &format!("{:.1}%", usage.percent_f));
        win::lv_set_text(lv, base + 2, 1, &format_bytes(usage.free));
        win::lv_set_text(lv, base + 3, 1, &format_bytes(usage.total));
    }
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

unsafe fn listview_custom_draw(hwnd: HWND, cd: &mut NMLVCUSTOMDRAW) -> isize {
    match cd.nmcd.dwDrawStage {
        CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as isize,
        CDDS_ITEMPREPAINT => {
            let row = cd.nmcd.dwItemSpec;
            let group = (row / 4).min(2);
            let lv = win::item(hwnd, IDC_LISTVIEW);
            if let Some(p) = theme::palette() {
                cd.clrTextBk = p.control;
            }
            if row.is_multiple_of(4) {
                SelectObject(cd.nmcd.hdc, bold_font(lv));
                cd.clrText = theme::heading_color();
            } else {
                let pct = GROUP_PERCENT.with(|g| g.get())[group];
                cd.clrText = if pct >= icon::danger_level() {
                    icon::danger_color()
                } else if pct >= icon::warning_level() {
                    icon::warning_color()
                } else {
                    theme::text_color()
                };
            }
            CDRF_NEWFONT as isize
        }
        _ => CDRF_DODEFAULT as isize,
    }
}

unsafe fn on_notify(hwnd: HWND, lparam: LPARAM) -> isize {
    let hdr = &*(lparam as *const NMHDR);
    let id = hdr.idFrom as i32;
    match hdr.code {
        NM_CUSTOMDRAW if id == IDC_LISTVIEW => {
            let cd = &mut *(lparam as *mut NMLVCUSTOMDRAW);
            win::dlg_result(hwnd, listview_custom_draw(hwnd, cd))
        }
        NM_CUSTOMDRAW if id == IDC_CLEAN => match theme::custom_draw_button(&*(lparam as *const NMCUSTOMDRAW)) {
            Some(r) => win::dlg_result(hwnd, r),
            None => 0,
        },
        LVN_ITEMCHANGING if id == IDC_LISTVIEW => {
            // the statistics list is read-only: never select rows
            let nm = &*(lparam as *const NMLISTVIEW);
            let block = nm.uChanged & LVIF_STATE != 0 && nm.uNewState & (LVIS_SELECTED | LVIS_FOCUSED) != 0;
            win::dlg_result(hwnd, block as isize)
        }
        BCN_DROPDOWN if id == IDC_CLEAN => {
            show_region_menu(hwnd, hdr.hwndFrom);
            win::dlg_result(hwnd, 0)
        }
        _ => 0,
    }
}

fn on_tray(hwnd: HWND, lparam: LPARAM) {
    let event = win::loword(lparam as usize) as u32;
    match event {
        WM_LBUTTONUP | WM_MBUTTONUP => {
            let action = if event == WM_MBUTTONUP {
                config::get_u32("TrayActionMc", 1)
            } else {
                config::get_u32("TrayActionDc", 0)
            };
            match action {
                1 => {
                    unsafe { SetForegroundWindow(hwnd) };
                    request_clean(hwnd, Source::Manual, None);
                }
                2 => sys::shell_open("taskmgr.exe"),
                _ => toggle(hwnd),
            }
        }
        NIN_KEYSELECT => show(hwnd),
        NIN_BALLOONUSERCLICK => crate::update::on_balloon_click(hwnd),
        WM_CONTEXTMENU => show_tray_menu(hwnd),
        _ => {}
    }
}

fn on_command(hwnd: HWND, id: i32) {
    if (IDX_LIMIT_BASE..IDX_LIMIT_BASE + 13).contains(&id) {
        if let Some(v) = LIMITS.with(|l| l.borrow().get((id - IDX_LIMIT_BASE) as usize).copied()) {
            config::set_bool("AutoreductEnable", true);
            config::set_u32("AutoreductValue", v);
            settings::reload_page_values();
        }
        return;
    }
    if (IDX_INTERVAL_BASE..IDX_INTERVAL_BASE + 13).contains(&id) {
        if let Some(v) = INTERVALS.with(|l| l.borrow().get((id - IDX_INTERVAL_BASE) as usize).copied()) {
            config::set_bool("AutoreductIntervalEnable", true);
            config::set_u32("AutoreductIntervalValue", v);
            settings::reload_page_values();
        }
        return;
    }
    if (IDM_THEME_BASE..IDM_THEME_BASE + theme::PRESETS.len() as i32).contains(&id) {
        theme::set_theme(theme::PRESETS[(id - IDM_THEME_BASE) as usize].key);
        apply_theme_everywhere();
        settings::reload_page_values();
        return;
    }
    if (IDM_REGION_TOGGLE_BASE..IDM_REGION_TOGGLE_BASE + REGIONS.len() as i32).contains(&id) {
        let region = &REGIONS[(id - IDM_REGION_TOGGLE_BASE) as usize];
        let mask = config::get_u32("ReductMask2", MASK_DEFAULT);
        let enabling = mask & region.mask == 0;
        if enabling && region.mask & MASK_FREEZES != 0 && !confirm_freeze(hwnd) {
            return;
        }
        config::set_u32("ReductMask2", mask ^ region.mask);
        settings::reload_page_values();
        return;
    }
    if (IDM_REGION_CLEAN_BASE..IDM_REGION_CLEAN_BASE + REGIONS.len() as i32).contains(&id) {
        let region = &REGIONS[(id - IDM_REGION_CLEAN_BASE) as usize];
        request_clean(hwnd, Source::Region, Some(region.mask));
        return;
    }

    match id {
        IDM_ALWAYSONTOP => {
            config::invert_bool("AlwaysOnTop", false);
            apply_topmost(hwnd);
            update_menu_checks();
            settings::reload_page_values();
        }
        IDM_REDUCTCONFIRM => {
            config::invert_bool("IsShowReductConfirmation", true);
            update_menu_checks();
            settings::reload_page_values();
        }
        IDM_STARTMINIMIZED => {
            config::invert_bool("IsStartMinimized", false);
            update_menu_checks();
            settings::reload_page_values();
        }
        IDM_LOADONSTARTUP => {
            sys::autorun_enable(!sys::autorun_is_enabled());
            update_menu_checks();
            settings::reload_page_values();
        }
        IDM_SKIPUAC => {
            set_skipuac(hwnd, !config::get_bool("SkipUacEnabled", false));
            settings::reload_page_values();
        }
        IDM_REQUIREADMIN => {
            set_require_admin(hwnd, !config::get_bool("RequireAdmin", true));
            settings::reload_page_values();
        }
        IDM_CHECKUPDATES => crate::update::check(hwnd, false),
        IDM_OPENCONFIG => {
            let dir = config::dir();
            let _ = std::fs::create_dir_all(&dir);
            sys::shell_open(&dir.to_string_lossy());
        }
        IDM_CUSTOMIZE_COLORS => settings::open(hwnd, Some(settings::PAGE_THEME)),
        IDM_TRAY_DISABLE_LIMIT => {
            config::set_bool("AutoreductEnable", false);
            settings::reload_page_values();
        }
        IDM_TRAY_DISABLE_INTERVAL => {
            config::set_bool("AutoreductIntervalEnable", false);
            settings::reload_page_values();
        }
        IDM_SETTINGS => settings::open(hwnd, None),
        IDM_EXIT => unsafe {
            DestroyWindow(hwnd);
        },
        IDCANCEL | IDM_TRAY_SHOW => toggle(hwnd),
        IDOK | IDC_CLEAN | IDM_TRAY_CLEAN => request_clean(hwnd, Source::Manual, None),
        IDM_WEBSITE => sys::shell_open(sys::REPO_URL),
        IDM_ABOUT => about(hwnd),
        _ => {}
    }
}

/// "Require administrator rights": when switched on in a non-elevated
/// session, restarts elevated right away.
pub fn set_require_admin(owner: HWND, enable: bool) {
    config::set_bool("RequireAdmin", enable);
    update_menu_checks();
    if enable && !sys::is_elevated() {
        let main = main_hwnd();
        if restart_elevated(main) {
            unsafe { DestroyWindow(main) };
        } else {
            win::message_box(
                owner,
                "Administrator rights were not granted. Mem Reduct will ask for them the next time it starts.",
                APP_NAME,
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
}

/// Warning shown before enabling standby/modified list cleaning.
pub fn confirm_freeze(hwnd: HWND) -> bool {
    if !config::get_bool("IsShowWarningConfirmation", true) {
        return true;
    }
    let (ok, dont_ask) = confirm(
        hwnd,
        "Enable a region that may freeze the computer?",
        "Cleaning the \"Standby list\" or the \"Modified page list\" can make the computer unresponsive for up to a minute. Are you sure you want to enable it?",
        true,
    );
    if ok && dont_ask {
        config::set_bool("IsShowWarningConfirmation", false);
    }
    ok
}

pub fn set_skipuac(hwnd: HWND, enable: bool) {
    if !sys::is_elevated() {
        return;
    }
    if sys::skipuac_enable(enable) {
        config::set_bool("SkipUacEnabled", enable);
    } else {
        win::message_box(hwnd, "Could not update the scheduled task used to skip the UAC prompt.", APP_NAME, MB_OK | MB_ICONWARNING);
    }
    update_menu_checks();
}

// ---------------------------------------------------------------------------
// Dialog procedure
// ---------------------------------------------------------------------------

fn init(hwnd: HWND) {
    MAIN.with(|m| m.set(hwnd));
    unsafe {
        let prop = instance_prop();
        SetPropW(hwnd, prop.as_ptr(), 1 as _);

        let tb = wide("TaskbarCreated");
        MSG_TASKBAR_CREATED.with(|m| m.set(RegisterWindowMessageW(tb.as_ptr())));
        // let a non-elevated instance / explorer talk to an elevated one
        ChangeWindowMessageFilterEx(hwnd, MSG_TASKBAR_CREATED.with(|m| m.get()), MSGFLT_ALLOW, std::ptr::null_mut());
        ChangeWindowMessageFilterEx(hwnd, activate_message(), MSGFLT_ALLOW, std::ptr::null_mut());
    }

    load_app_icons(hwnd);
    init_listview(hwnd);

    let clean = win::item(hwnd, IDC_CLEAN);
    if !sys::is_elevated() {
        win::send(clean, BCM_SETSHIELD, 0, 1);
        theme::mark_shield(clean, true);
    }

    // add the menu bar, then grow the window so the client area is unchanged
    unsafe {
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        build_menu(hwnd);
        let dpi = GetDpiForWindow(hwnd).max(96);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        AdjustWindowRectExForDpi(&mut client, style, 1, ex, dpi);
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            client.right - client.left,
            client.bottom - client.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    theme::apply(hwnd);
    add_tray(hwnd);
    crate::update::notify_if_updated(hwnd);
    init_hotkey();
    unsafe { SetTimer(hwnd, TIMER_ID, 1000, None) };
}

unsafe extern "system" fn dlg_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    if msg == MSG_TASKBAR_CREATED.with(|m| m.get()) && msg != 0 {
        refresh_icon();
        add_tray(hwnd);
        return 0;
    }
    if msg == activate_message() {
        show(hwnd);
        return 0;
    }

    match msg {
        WM_INITDIALOG => {
            init(hwnd);
            0
        }
        WM_TIMER if wparam == TIMER_ID => {
            on_timer(hwnd, true);
            0
        }
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            theme::ctl_color(msg, wparam as HDC, lparam as HWND).unwrap_or(0)
        }
        WM_NOTIFY => on_notify(hwnd, lparam),
        WM_MEASUREITEM => theme::measure_menu_item(hwnd, &mut *(lparam as *mut MEASUREITEMSTRUCT)) as isize,
        WM_DRAWITEM => theme::draw_menu_item(hwnd, &*(lparam as *const DRAWITEMSTRUCT)) as isize,
        WM_MENUCHAR => match theme::menu_char(lparam as HMENU, win::loword(wparam)) {
            Some(r) => win::dlg_result(hwnd, r),
            None => 0,
        },
        WM_COMMAND => {
            on_command(hwnd, win::loword(wparam) as i32);
            0
        }
        WM_HOTKEY => {
            if wparam as i32 == HOTKEY_ID {
                request_clean(hwnd, Source::Hotkey, None);
            }
            0
        }
        WM_TRAY => {
            on_tray(hwnd, lparam);
            0
        }
        WM_CLEAN_DONE => {
            let result = Box::from_raw(lparam as *mut memory::CleanResult);
            finish_clean(hwnd, Source::from(wparam), *result);
            0
        }
        crate::update::WM_UPDATE_CHECKED => {
            crate::update::on_checked(hwnd, wparam, lparam);
            0
        }
        crate::update::WM_UPDATE_DOWNLOADED => {
            crate::update::on_downloaded(hwnd, lparam);
            0
        }
        WM_SIZE => {
            resize_columns(hwnd);
            0
        }
        WM_DPICHANGED => {
            load_app_icons(hwnd);
            refresh_icon();
            0 // let the dialog manager rescale the layout
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED => {
            if theme::palette().is_none() {
                theme::apply(hwnd);
                // the Windows accent color may have changed
                if config::get_bool("TrayUseThemeColors", false) {
                    refresh_icon();
                }
            }
            0
        }
        WM_EXITSIZEMOVE => {
            save_position(hwnd);
            0
        }
        WM_CLOSE => {
            toggle(hwnd);
            1
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_ID);
            UnregisterHotKey(hwnd, HOTKEY_ID);
            if IsWindowVisible(hwnd) != 0 {
                save_position(hwnd);
            }
            tray::remove(hwnd);
            settings::close();
            let prop = instance_prop();
            RemovePropW(hwnd, prop.as_ptr());
            PostQuitMessage(0);
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_values_match_original() {
        assert_eq!(menu_values(90, 99), vec![10, 20, 30, 40, 50, 60, 70, 80, 88, 89, 90, 91, 92]);
        assert_eq!(menu_values(30, 1440), vec![10, 20, 28, 29, 30, 31, 32, 40, 50, 60, 70, 80]);
        assert!(menu_values(99, 99).iter().all(|&v| v <= 99));
        assert_eq!(menu_values(1, 99)[0], 10);
    }
}
