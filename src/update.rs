//! Self-update from GitHub releases.
//!
//! The latest release of the project repository is queried through the GitHub
//! API. When it is newer than the running build, its `.exe` asset is
//! downloaded, checked against the SHA-256 listed in the release notes, and
//! swapped in place of the running executable (Windows allows renaming a
//! running exe). The app then restarts. Settings are never touched: they live
//! in %APPDATA% or in the portable memreduct.ini next to the exe, which stays
//! where it is.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::Networking::WinHttp::*;
use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config;
use crate::sys::{self, APP_NAME, APP_VERSION};
use crate::tray;
use crate::util::{log, unix_now, wide, LogLevel};
use crate::win;

const API_URL: &str = "https://api.github.com/repos/SYNLiquify/memreduct-rs/releases/latest";
const CHECK_INTERVAL: i64 = 24 * 60 * 60;

pub const WM_UPDATE_CHECKED: u32 = WM_APP + 3;
pub const WM_UPDATE_DOWNLOADED: u32 = WM_APP + 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub page_url: String,
    pub exe_name: String,
    pub exe_url: String,
    pub sha256: Option<String>,
}

enum CheckResult {
    Available(Release),
    UpToDate(String),
    Failed(String),
}

thread_local! {
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<Option<Release>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------
// Versions and release parsing (pure, unit-tested)
// ---------------------------------------------------------------------------

/// "v.3.5.2" / "v3.5.2" / "3.5.2" -> [3, 5, 2]
pub fn parse_version(s: &str) -> Vec<u32> {
    let digits = s.trim_start_matches(|c: char| !c.is_ascii_digit());
    let mut v: Vec<u32> = digits
        .split('.')
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
        .take_while(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect();
    while v.len() > 1 && v.last() == Some(&0) {
        v.pop();
    }
    v
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (a, b) = (parse_version(candidate), parse_version(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// Reads the JSON string value that follows `"key":` starting at `from`.
/// Returns the unescaped value and the position after it.
fn json_string(json: &str, key: &str, from: usize) -> Option<(String, usize)> {
    let needle = format!("\"{}\"", key);
    let start = from + json.get(from..)?.find(&needle)? + needle.len();
    let rest = &json[start..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    if !after.starts_with('"') {
        return None;
    }
    let body_start = json.len() - after.len() + 1;
    let mut out = String::new();
    let mut chars = json[body_start..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, body_start + i + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next().map(|(_, c)| c)).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// Finds "<64 hex chars>  <file name>" in the release notes.
fn find_sha256(body: &str, file_name: &str) -> Option<String> {
    body.lines().filter(|l| l.contains(file_name)).find_map(|line| {
        line.split_whitespace()
            .find(|w| w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|w| w.to_ascii_uppercase())
    })
}

pub fn parse_release(json: &str) -> Option<Release> {
    let (tag, _) = json_string(json, "tag_name", 0)?;
    let page_url = json_string(json, "html_url", 0).map(|v| v.0).unwrap_or_else(|| sys::REPO_URL.to_string());
    let body = json_string(json, "body", 0).map(|v| v.0).unwrap_or_default();

    let mut pos = 0;
    let mut exe = None;
    while let Some((url, next)) = json_string(json, "browser_download_url", pos) {
        pos = next;
        if url.to_ascii_lowercase().ends_with(".exe") {
            exe = Some(url);
            break;
        }
    }
    let exe_url = exe?;
    let exe_name = exe_url.rsplit('/').next().unwrap_or("memreduct.exe").to_string();
    let sha256 = find_sha256(&body, &exe_name);
    Some(Release { tag, page_url, exe_name, exe_url, sha256 })
}

// ---------------------------------------------------------------------------
// HTTP (WinHTTP) and hashing (CNG)
// ---------------------------------------------------------------------------

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn http_get(url: &str, accept: Option<&str>) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("only https URLs are supported")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let agent = wide(&format!("memreduct-rs/{}", APP_VERSION));
    let whost = wide(host);
    let wpath = wide(path);
    let verb = wide("GET");
    unsafe {
        let session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0));
        if session.0.is_null() {
            return Err("could not start WinHTTP".into());
        }
        let connect = Handle(WinHttpConnect(session.0, whost.as_ptr(), 443, 0));
        if connect.0.is_null() {
            return Err(format!("could not connect to {}", host));
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            verb.as_ptr(),
            wpath.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err("could not create the request".into());
        }
        let headers = accept.map(|a| wide(&format!("Accept: {}\r\n", a)));
        let (hptr, hlen) = match &headers {
            Some(h) => (h.as_ptr(), u32::MAX), // -1: NUL-terminated
            None => (std::ptr::null(), 0),
        };
        if WinHttpSendRequest(request.0, hptr, hlen, std::ptr::null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0
        {
            return Err(format!("network error ({})", windows_sys::Win32::Foundation::GetLastError()));
        }
        let mut status = 0u32;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut len,
            std::ptr::null_mut(),
        );
        if status != 200 {
            return Err(format!("server answered HTTP {}", status));
        }
        let mut data = Vec::new();
        loop {
            let mut avail = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut avail) == 0 {
                return Err("download interrupted".into());
            }
            if avail == 0 {
                break;
            }
            let start = data.len();
            data.resize(start + avail as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(request.0, data[start..].as_mut_ptr() as *mut c_void, avail, &mut read) == 0 {
                return Err("download interrupted".into());
            }
            data.truncate(start + read as usize);
        }
        Ok(data)
    }
}

pub fn sha256_hex(data: &[u8]) -> Option<String> {
    let mut out = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            std::ptr::null(),
            0,
            data.as_ptr(),
            data.len() as u32,
            out.as_mut_ptr(),
            out.len() as u32,
        )
    };
    (status >= 0).then(|| out.iter().map(|b| format!("{:02X}", b)).collect())
}

// ---------------------------------------------------------------------------
// Workflow
// ---------------------------------------------------------------------------

fn fetch_latest() -> CheckResult {
    let json = match http_get(API_URL, Some("application/vnd.github+json")) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => return CheckResult::Failed(e),
    };
    match parse_release(&json) {
        Some(rel) if is_newer(&rel.tag, APP_VERSION) => CheckResult::Available(rel),
        Some(rel) => CheckResult::UpToDate(rel.tag),
        None => CheckResult::Failed("the latest release has no .exe download".into()),
    }
}

/// Starts a check in the background. `silent` checks only show a balloon
/// when an update exists.
pub fn check(hwnd: HWND, silent: bool) {
    if BUSY.with(|b| b.replace(true)) {
        if !silent {
            win::message_box(hwnd, "An update check or download is already running.", APP_NAME, MB_OK | MB_ICONINFORMATION);
        }
        return;
    }
    config::set_i64("UpdateLastCheck", unix_now());
    let target = hwnd as usize;
    std::thread::spawn(move || {
        let ptr = Box::into_raw(Box::new(fetch_latest()));
        unsafe {
            if PostMessageW(target as HWND, WM_UPDATE_CHECKED, silent as usize, ptr as isize) == 0 {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

/// Called every timer tick: daily automatic check.
pub fn maybe_auto_check(hwnd: HWND) {
    if !config::get_bool("CheckUpdates", true) || BUSY.with(|b| b.get()) {
        return;
    }
    if unix_now() - config::get_i64("UpdateLastCheck", 0) >= CHECK_INTERVAL {
        check(hwnd, true);
    }
}

pub fn on_checked(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    BUSY.with(|b| b.set(false));
    let silent = wparam != 0;
    let result = unsafe { Box::from_raw(lparam as *mut CheckResult) };
    match *result {
        CheckResult::Available(rel) => {
            if silent {
                tray::balloon(
                    hwnd,
                    "Update available",
                    &format!("{} {} is available (you have {}). Click here to install it.", APP_NAME, rel.tag, APP_VERSION),
                    false,
                    config::get_bool("IsNotificationsSound", true),
                );
                PENDING.with(|p| *p.borrow_mut() = Some(rel));
            } else {
                prompt_install(hwnd, rel);
            }
        }
        CheckResult::UpToDate(tag) => {
            if !silent {
                win::message_box(
                    hwnd,
                    &format!("You are using the latest version ({}). Latest release: {}.", APP_VERSION, tag),
                    APP_NAME,
                    MB_OK | MB_ICONINFORMATION,
                );
            }
        }
        CheckResult::Failed(e) => {
            log(LogLevel::Error, &format!("Update check failed: {}", e));
            if !silent {
                win::message_box(hwnd, &format!("Could not check for updates: {}.", e), APP_NAME, MB_OK | MB_ICONWARNING);
            }
        }
    }
}

/// The user clicked the "update available" balloon.
pub fn on_balloon_click(hwnd: HWND) {
    if let Some(rel) = PENDING.with(|p| p.borrow_mut().take()) {
        prompt_install(hwnd, rel);
    }
}

unsafe extern "system" fn link_callback(_: HWND, msg: TASKDIALOG_NOTIFICATIONS, _: WPARAM, lparam: LPARAM, _: isize) -> i32 {
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

fn prompt_install(hwnd: HWND, rel: Release) {
    let title = wide(APP_NAME);
    let main = wide(&format!("Update available: {}", rel.tag));
    let content = wide(&format!(
        "You have version {}. Download and install {} now?\n\n\
         Your settings are kept. The app restarts when the update is installed.\n\n\
         <a href=\"{}\">See what's new</a>",
        APP_VERSION, rel.tag, rel.page_url
    ));
    let accepted = unsafe {
        let mut cfg: TASKDIALOGCONFIG = std::mem::zeroed();
        cfg.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        cfg.hwndParent = hwnd;
        cfg.dwFlags = TDF_ENABLE_HYPERLINKS | TDF_ALLOW_DIALOG_CANCELLATION;
        cfg.dwCommonButtons = TDCBF_YES_BUTTON | TDCBF_NO_BUTTON;
        cfg.pszWindowTitle = title.as_ptr();
        cfg.Anonymous1.pszMainIcon = TD_INFORMATION_ICON;
        cfg.pszMainInstruction = main.as_ptr();
        cfg.pszContent = content.as_ptr();
        cfg.pfCallback = Some(link_callback);
        let mut button = 0i32;
        TaskDialogIndirect(&cfg, &mut button, std::ptr::null_mut(), std::ptr::null_mut()) >= 0 && button == IDYES
    };
    if accepted {
        download(hwnd, rel);
    }
}

fn staged_path(exe: &Path, suffix: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn download_and_verify(rel: &Release) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let data = http_get(&rel.exe_url, Some("application/octet-stream"))?;
    if data.len() < 64 * 1024 || !data.starts_with(b"MZ") {
        return Err("the downloaded file is not a valid program".into());
    }
    match &rel.sha256 {
        Some(expected) => {
            let actual = sha256_hex(&data).ok_or("could not compute the checksum")?;
            if &actual != expected {
                return Err(format!("checksum mismatch (expected {}, got {})", expected, actual));
            }
        }
        None => return Err("the release notes do not list a SHA-256 checksum for the download".into()),
    }
    let staged = staged_path(&exe, ".new");
    std::fs::write(&staged, &data).map_err(|e| format!("could not save the update next to the program: {}", e))?;
    Ok(staged)
}

fn download(hwnd: HWND, rel: Release) {
    if BUSY.with(|b| b.replace(true)) {
        return;
    }
    tray::balloon(hwnd, APP_NAME, &format!("Downloading {}...", rel.tag), false, false);
    let target = hwnd as usize;
    std::thread::spawn(move || {
        let result: Result<PathBuf, String> = download_and_verify(&rel);
        let ptr = Box::into_raw(Box::new((rel.tag.clone(), result)));
        unsafe {
            if PostMessageW(target as HWND, WM_UPDATE_DOWNLOADED, 0, ptr as isize) == 0 {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

pub fn on_downloaded(hwnd: HWND, lparam: LPARAM) {
    BUSY.with(|b| b.set(false));
    let boxed = unsafe { Box::from_raw(lparam as *mut (String, Result<PathBuf, String>)) };
    let (tag, result) = *boxed;
    match result {
        Ok(staged) => install(hwnd, &tag, &staged),
        Err(e) => {
            log(LogLevel::Error, &format!("Update download failed: {}", e));
            win::message_box(hwnd, &format!("The update could not be downloaded: {}.", e), APP_NAME, MB_OK | MB_ICONWARNING);
        }
    }
}

/// Swaps the executable and restarts. The config file is not touched.
fn install(hwnd: HWND, tag: &str, staged: &Path) {
    let fail = |msg: String| {
        log(LogLevel::Error, &format!("Update install failed: {}", msg));
        let _ = std::fs::remove_file(staged);
        win::message_box(hwnd, &format!("The update could not be installed: {}.", msg), APP_NAME, MB_OK | MB_ICONWARNING);
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return fail(e.to_string()),
    };
    let old = staged_path(&exe, ".old");
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(&exe, &old) {
        return fail(format!("could not replace the program file ({})", e));
    }
    if let Err(e) = std::fs::rename(staged, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return fail(format!("could not move the new version in place ({})", e));
    }

    config::set_string("UpdatedTo", tag);
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    crate::release_instance_mutex();
    let mut cmd = std::process::Command::new(&exe);
    if !visible {
        cmd.arg("-minimized");
    }
    match cmd.spawn() {
        Ok(_) => {
            tray::remove(hwnd);
            unsafe { DestroyWindow(hwnd) };
        }
        Err(e) => {
            crate::acquire_instance_mutex();
            config::remove("UpdatedTo");
            win::message_box(
                hwnd,
                &format!("{} was installed but could not be started ({}). Please start it again.", tag, e),
                APP_NAME,
                MB_OK | MB_ICONWARNING,
            );
        }
    }
}

/// Startup housekeeping: removes the previous executable left by an update.
pub fn cleanup_after_update() {
    let Ok(exe) = std::env::current_exe() else { return };
    let old = staged_path(&exe, ".old");
    let stale = staged_path(&exe, ".new");
    if !old.exists() && !stale.exists() {
        return;
    }
    std::thread::spawn(move || {
        // the previous process may still be shutting down
        for _ in 0..10 {
            let _ = std::fs::remove_file(&stale);
            if std::fs::remove_file(&old).is_ok() || !old.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

/// After the main window is up: tells the user the update went through.
pub fn notify_if_updated(hwnd: HWND) {
    if let Some(tag) = config::get_raw("UpdatedTo") {
        config::remove("UpdatedTo");
        tray::balloon(
            hwnd,
            APP_NAME,
            &format!("Updated to {} (version {}). Your settings were kept.", tag, APP_VERSION),
            false,
            config::get_bool("IsNotificationsSound", true),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v.3.5.2"), vec![3, 5, 2]);
        assert_eq!(parse_version("v3.6"), vec![3, 6]);
        assert_eq!(parse_version("3.5.0"), vec![3, 5]);
        assert!(is_newer("v.3.5.3", "3.5.2"));
        assert!(is_newer("v.3.6", "3.5.2"));
        assert!(is_newer("v.4.0", "3.5.2"));
        assert!(!is_newer("v.3.5.2", "3.5.2"));
        assert!(!is_newer("v.3.5", "3.5.0"));
        assert!(!is_newer("v.3.4.9", "3.5.2"));
    }

    #[test]
    fn release_json() {
        let json = r#"{"url":"x","html_url":"https://github.com/SYNLiquify/memreduct-rs/releases/tag/v.3.5.3","tag_name":"v.3.5.3",
            "assets":[{"name":"memreduct-3.5.3-bin.zip","browser_download_url":"https://github.com/SYNLiquify/memreduct-rs/releases/download/v.3.5.3/memreduct-3.5.3-bin.zip"},
            {"name":"memreduct-3.5.3.exe","browser_download_url":"https://github.com/SYNLiquify/memreduct-rs/releases/download/v.3.5.3/memreduct-3.5.3.exe"}],
            "body":"Notes & more\r\n```\r\n2C582869A6EE8194204AE7E66F8B12882E8A5CDB3F2D0E09EB48C00C5975A83B  memreduct-3.5.3.exe\r\n56277A36D227BA081E06D348AFFA514AC57CE86D757EE329FB46DC000E447020  memreduct-3.5.3-bin.zip\r\n```"}"#;
        let rel = parse_release(json).unwrap();
        assert_eq!(rel.tag, "v.3.5.3");
        assert_eq!(rel.exe_name, "memreduct-3.5.3.exe");
        assert!(rel.exe_url.ends_with("/v.3.5.3/memreduct-3.5.3.exe"));
        assert_eq!(rel.page_url, "https://github.com/SYNLiquify/memreduct-rs/releases/tag/v.3.5.3");
        assert_eq!(rel.sha256.as_deref(), Some("2C582869A6EE8194204AE7E66F8B12882E8A5CDB3F2D0E09EB48C00C5975A83B"));
    }

    /// Network test against the real repository: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn live_latest_release() {
        let json = String::from_utf8(http_get(API_URL, Some("application/vnd.github+json")).unwrap()).unwrap();
        let rel = parse_release(&json).expect("latest release parses");
        println!("{:?}", rel);
        assert!(rel.exe_url.ends_with(".exe"));
        assert!(rel.sha256.is_some(), "release notes must list the exe checksum");
        let exe = http_get(&rel.exe_url, Some("application/octet-stream")).unwrap();
        assert_eq!(sha256_hex(&exe), rel.sha256);
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }
}
