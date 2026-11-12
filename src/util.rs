//! Small helpers: UTF-16 strings, byte formatting, colors, logging.

use std::io::Write;

/// Builds a NUL-terminated UTF-16 string.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a UTF-16 buffer up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Copies `s` into a fixed UTF-16 buffer, truncating and NUL-terminating.
pub fn copy_to_buf(dst: &mut [u16], s: &str) {
    if dst.is_empty() {
        return;
    }
    let max = dst.len() - 1;
    let mut n = 0;
    for c in s.encode_utf16() {
        if n >= max {
            break;
        }
        dst[n] = c;
        n += 1;
    }
    dst[n] = 0;
}

/// "1.5 GB", "512 MB", "0 B" ...
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} B", bytes);
    }
    let mut s = format!("{:.2}", value);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    format!("{} {}", s, UNITS[unit])
}

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

pub fn color_r(c: u32) -> u8 {
    (c & 0xFF) as u8
}
pub fn color_g(c: u32) -> u8 {
    ((c >> 8) & 0xFF) as u8
}
pub fn color_b(c: u32) -> u8 {
    ((c >> 16) & 0xFF) as u8
}

/// Linear blend between two COLORREFs; `t` = 0 gives `a`, 1 gives `b`.
pub fn blend(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    rgb(
        mix(color_r(a), color_r(b)),
        mix(color_g(a), color_g(b)),
        mix(color_b(a), color_b(b)),
    )
}

/// Relative luminance in 0..1.
pub fn luminance(c: u32) -> f32 {
    (0.2126 * color_r(c) as f32 + 0.7152 * color_g(c) as f32 + 0.0722 * color_b(c) as f32) / 255.0
}

pub fn is_dark(c: u32) -> bool {
    luminance(c) < 0.5
}

/// Black or white, whichever reads better on `bg`.
pub fn contrast_text(bg: u32) -> u32 {
    if is_dark(bg) {
        rgb(0xFF, 0xFF, 0xFF)
    } else {
        rgb(0x00, 0x00, 0x00)
    }
}

pub fn color_to_hex(c: u32) -> String {
    format!("#{:02X}{:02X}{:02X}", color_r(c), color_g(c), color_b(c))
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn local_timestamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut st = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

#[derive(Clone, Copy)]
pub enum LogLevel {
    Info,
    Error,
}

/// Appends a line to `memreduct.log` next to the config file.
pub fn log(level: LogLevel, message: &str) {
    let path = crate::config::dir().join("memreduct.log");
    let tag = match level {
        LogLevel::Info => "INFO",
        LogLevel::Error => "ERROR",
    };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{}] {}: {}", local_timestamp(), tag, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(8 * 1024 * 1024 * 1024), "8 GB");
        assert_eq!(format_bytes(1_288_490_189), "1.2 GB");
    }

    #[test]
    fn colors() {
        let c = rgb(0x12, 0x34, 0x56);
        assert_eq!((color_r(c), color_g(c), color_b(c)), (0x12, 0x34, 0x56));
        assert_eq!(color_to_hex(c), "#123456");
        assert_eq!(blend(rgb(0, 0, 0), rgb(255, 255, 255), 0.5), rgb(128, 128, 128));
        assert!(is_dark(rgb(0x20, 0x20, 0x20)));
        assert!(!is_dark(rgb(0xF0, 0xF0, 0xF0)));
    }

    #[test]
    fn wide_roundtrip() {
        let w = wide("héllo");
        assert_eq!(*w.last().unwrap(), 0);
        assert_eq!(from_wide(&w), "héllo");
        let mut buf = [0u16; 4];
        copy_to_buf(&mut buf, "abcdef");
        assert_eq!(from_wide(&buf), "abc");
    }
}
