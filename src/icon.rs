//! Draws the tray icon (memory load percentage) with real per-pixel alpha.
//!
//! GDI draws text and shapes in white on black into scratch DIBs; those
//! become coverage masks that are composited into a premultiplied BGRA icon.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO, SM_CXSMICON};

use crate::config;
use crate::util::{color_b, color_g, color_r, copy_to_buf, rgb};

pub const COLOR_TEXT: u32 = rgb(0xFF, 0xFF, 0xFF);
pub const COLOR_BG: u32 = rgb(0x00, 0x80, 0x40);
pub const COLOR_WARNING: u32 = rgb(0xFF, 0x80, 0x40);
pub const COLOR_DANGER: u32 = rgb(0xEC, 0x1C, 0x24);

pub const DEFAULT_WARNING_LEVEL: u32 = 70;
pub const DEFAULT_DANGER_LEVEL: u32 = 90;

pub const DEFAULT_FONT_FACE: &str = "Lucida Console";
pub const DEFAULT_FONT_SIZE: i32 = 8;

pub fn warning_level() -> u32 {
    config::get_u32("TrayLevelWarning", DEFAULT_WARNING_LEVEL).min(100)
}

pub fn danger_level() -> u32 {
    config::get_u32("TrayLevelDanger", DEFAULT_DANGER_LEVEL).min(100)
}

pub fn warning_color() -> u32 {
    config::get_u32("TrayColorWarning", COLOR_WARNING)
}

pub fn danger_color() -> u32 {
    config::get_u32("TrayColorDanger", COLOR_DANGER)
}

/// Font spec stored as "Face;size_pt;weight".
#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub face: String,
    pub size: i32,
    pub weight: i32,
}

impl FontSpec {
    pub fn load() -> Self {
        let raw = config::get_string("TrayFont", "");
        let mut parts = raw.split(';');
        let face = parts.next().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_FONT_FACE);
        let size = parts.next().and_then(|s| s.trim().parse().ok()).filter(|&s| s > 0).unwrap_or(DEFAULT_FONT_SIZE);
        let weight = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(400);
        FontSpec { face: face.to_string(), size, weight }
    }

    pub fn save(&self) {
        config::set_string("TrayFont", &format!("{};{};{}", self.face, self.size, self.weight));
    }

    pub fn to_logfont(&self, dpi: u32) -> LOGFONTW {
        let mut lf: LOGFONTW = unsafe { std::mem::zeroed() };
        copy_to_buf(&mut lf.lfFaceName, &self.face);
        lf.lfHeight = -((self.size * dpi as i32 + 36) / 72);
        lf.lfWeight = self.weight;
        lf.lfCharSet = DEFAULT_CHARSET;
        lf.lfQuality = if config::get_bool("TrayUseAntialiasing", false) {
            ANTIALIASED_QUALITY
        } else {
            NONANTIALIASED_QUALITY
        };
        lf
    }

    pub fn describe(&self) -> String {
        format!("{}, {}pt, {}", self.face, self.size, self.weight)
    }
}

pub struct Style {
    pub text: u32,
    pub bg: u32,
    pub transparent: bool,
    pub border: bool,
    pub round: bool,
}

impl Style {
    /// Reads the configured style and applies warning/danger coloring.
    pub fn for_percent(percent: u32) -> Self {
        let mut s = Style {
            text: config::get_u32("TrayColorText", COLOR_TEXT),
            bg: config::get_u32("TrayColorBg", COLOR_BG),
            transparent: config::get_bool("TrayUseTransparency", false),
            border: config::get_bool("TrayShowBorder", false),
            round: config::get_bool("TrayRoundCorners", false),
        };
        let danger = percent >= danger_level();
        if danger || percent >= warning_level() {
            let level_color = if danger { danger_color() } else { warning_color() };
            if config::get_bool("TrayChangeBg", true) {
                s.bg = level_color;
                s.transparent = false;
            } else {
                s.text = level_color;
            }
        }
        s
    }
}

pub struct Renderer {
    size: i32,
    font: HFONT,
}

impl Renderer {
    pub fn new(dpi: u32) -> Self {
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.max(16);
        let lf = FontSpec::load().to_logfont(dpi);
        let font = unsafe { CreateFontIndirectW(&lf) };
        Renderer { size, font }
    }

    /// Renders a single draw pass and returns its 8-bit coverage.
    unsafe fn coverage(&self, draw: impl FnOnce(HDC, &RECT)) -> Vec<u8> {
        let s = self.size;
        let n = (s * s) as usize;
        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmi = dib_info(s);
        let bmp = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(hdc);
            return vec![0; n];
        }
        let old = SelectObject(hdc, bmp);
        let rect = RECT { left: 0, top: 0, right: s, bottom: s };
        draw(hdc, &rect);
        GdiFlush();
        let px = std::slice::from_raw_parts(bits as *const u8, n * 4);
        let cov = (0..n).map(|i| px[i * 4].max(px[i * 4 + 1]).max(px[i * 4 + 2])).collect();
        SelectObject(hdc, old);
        DeleteObject(bmp);
        DeleteDC(hdc);
        cov
    }

    unsafe fn shape(hdc: HDC, r: &RECT, round: bool) {
        if round {
            let radius = (r.right / 2).max(4);
            RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
        } else {
            Rectangle(hdc, r.left, r.top, r.right, r.bottom);
        }
    }

    pub fn render(&self, text: &str, style: &Style) -> HICON {
        let pixels = self.render_pixels(text, style);
        unsafe { self.make_icon(&pixels) }
    }

    /// Premultiplied BGRA pixels, `size * size`, top-down.
    pub fn render_pixels(&self, text: &str, style: &Style) -> Vec<u8> {
        unsafe {
            let fill = if style.transparent {
                vec![0u8; (self.size * self.size) as usize]
            } else {
                self.coverage(|hdc, r| {
                    SelectObject(hdc, GetStockObject(WHITE_BRUSH));
                    SelectObject(hdc, GetStockObject(WHITE_PEN));
                    Self::shape(hdc, r, style.round);
                })
            };
            let border = if style.border {
                self.coverage(|hdc, r| {
                    SelectObject(hdc, GetStockObject(NULL_BRUSH));
                    SelectObject(hdc, GetStockObject(WHITE_PEN));
                    Self::shape(hdc, r, style.round);
                })
            } else {
                vec![0u8; (self.size * self.size) as usize]
            };
            let wtext: Vec<u16> = text.encode_utf16().collect();
            let font = self.font;
            let glyphs = self.coverage(|hdc, r| {
                let mut rc = *r;
                SelectObject(hdc, font);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, rgb(0xFF, 0xFF, 0xFF));
                let mut buf = wtext.clone();
                DrawTextW(
                    hdc,
                    buf.as_mut_ptr(),
                    buf.len() as i32,
                    &mut rc,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_NOCLIP,
                );
            });

            composite(style, &fill, &border, &glyphs)
        }
    }

    #[allow(dead_code)]
    pub fn size(&self) -> i32 {
        self.size
    }

    unsafe fn make_icon(&self, bgra: &[u8]) -> HICON {
        let s = self.size;
        let hdc = GetDC(std::ptr::null_mut());
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmi = dib_info(s);
        let color = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), hdc);
        if color.is_null() || bits.is_null() {
            return std::ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());

        // all-zero AND mask: alpha channel decides transparency
        let stride = (((s + 15) / 16) * 2) as usize;
        let zeros = vec![0u8; stride * s as usize];
        let mask = CreateBitmap(s, s, 1, 1, zeros.as_ptr() as *const c_void);

        let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii);
        DeleteObject(mask);
        DeleteObject(color);
        icon
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        if !self.font.is_null() {
            unsafe { DeleteObject(self.font) };
        }
    }
}

fn dib_info(size: i32) -> BITMAPINFO {
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = size;
    bmi.bmiHeader.biHeight = -size; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    bmi
}

/// Background, then border, then text, each "over" the previous layer.
/// Returns premultiplied BGRA.
fn composite(style: &Style, fill: &[u8], border: &[u8], text: &[u8]) -> Vec<u8> {
    let to_f = |c: u32| [color_r(c) as f32, color_g(c) as f32, color_b(c) as f32];
    let bg = to_f(style.bg);
    let fg = to_f(style.text);
    let mut out = vec![0u8; fill.len() * 4];

    for i in 0..fill.len() {
        let mut c = bg;
        let mut a = fill[i] as f32 / 255.0;

        for (layer, alpha) in [(fg, border[i] as f32 / 255.0), (fg, text[i] as f32 / 255.0)] {
            if alpha <= 0.0 {
                continue;
            }
            let out_a = alpha + a * (1.0 - alpha);
            for k in 0..3 {
                c[k] = (layer[k] * alpha + c[k] * a * (1.0 - alpha)) / out_a;
            }
            a = out_a;
        }

        let pa = (a * 255.0).round().clamp(0.0, 255.0);
        out[i * 4] = (c[2] * a).round().clamp(0.0, 255.0) as u8; // B
        out[i * 4 + 1] = (c[1] * a).round().clamp(0.0, 255.0) as u8; // G
        out[i * 4 + 2] = (c[0] * a).round().clamp(0.0, 255.0) as u8; // R
        out[i * 4 + 3] = pa as u8;
    }
    out
}

pub fn destroy(icon: HICON) {
    if !icon.is_null() {
        unsafe { DestroyIcon(icon) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders real icons through GDI. Set MR_ICON_DUMP=<dir> to also write
    /// the raw BGRA buffers for inspection.
    #[test]
    fn render_real_icons() {
        let r = Renderer::new(96);
        let s = r.size() as usize;
        let cases = [
            ("solid", Style { text: COLOR_TEXT, bg: COLOR_BG, transparent: false, border: false, round: false }),
            ("round_border", Style { text: COLOR_TEXT, bg: COLOR_DANGER, transparent: false, border: true, round: true }),
            ("transparent", Style { text: COLOR_WARNING, bg: COLOR_BG, transparent: true, border: false, round: false }),
        ];
        for (name, style) in cases {
            let px = r.render_pixels("57", &style);
            assert_eq!(px.len(), s * s * 4);
            let alpha = |x: usize, y: usize| px[(y * s + x) * 4 + 3];
            let opaque_text = (0..s * s).filter(|&i| px[i * 4 + 3] == 255 && px[i * 4 + 2] == color_r(style.text)).count();
            assert!(opaque_text > 4, "{name}: no text pixels");
            match name {
                "solid" => {
                    assert_eq!(alpha(0, 0), 255);
                    assert_eq!(&px[0..3], &[color_b(COLOR_BG), color_g(COLOR_BG), color_r(COLOR_BG)]);
                }
                "round_border" => assert_eq!(alpha(0, 0), 0, "corner should be cut"),
                _ => assert_eq!(alpha(0, 0), 0, "background should be transparent"),
            }
            if let Ok(dir) = std::env::var("MR_ICON_DUMP") {
                let path = std::path::Path::new(&dir).join(format!("{name}_{s}.bgra"));
                std::fs::write(path, &px).unwrap();
            }
            let icon = r.render("57", &style);
            assert!(!icon.is_null());
            destroy(icon);
        }
    }

    #[test]
    fn composite_layers() {
        let style = Style { text: rgb(255, 255, 255), bg: rgb(0, 128, 64), transparent: false, border: false, round: false };
        // pixel 0: background only; pixel 1: text on background; pixel 2: nothing
        let out = composite(&style, &[255, 255, 0], &[0, 0, 0], &[0, 255, 0]);
        assert_eq!(&out[0..4], &[64, 128, 0, 255]);
        assert_eq!(&out[4..8], &[255, 255, 255, 255]);
        assert_eq!(&out[8..12], &[0, 0, 0, 0]);
    }
}
