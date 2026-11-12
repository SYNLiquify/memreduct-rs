// Build script: produces a compiled resource file (.res) in pure Rust and
// hands it to the MSVC linker together with the application manifest.
//
// Resources:
//   ICON 1          res/memreduct.ico (every size in the file)
//   VERSIONINFO 1   file/product version + description
//
// No rc.exe or extra crates are needed: link.exe accepts .res files directly.

use std::path::{Path, PathBuf};

fn u16le(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn u32le(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn pad4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

fn utf16z(v: &mut Vec<u8>, s: &str) {
    for c in s.encode_utf16() {
        u16le(v, c);
    }
    u16le(v, 0);
}

/// One resource entry with ordinal type and name.
fn res_entry(out: &mut Vec<u8>, res_type: u16, name: u16, flags: u16, data: &[u8]) {
    u32le(out, data.len() as u32); // DataSize
    u32le(out, 32); // HeaderSize
    u16le(out, 0xFFFF);
    u16le(out, res_type);
    u16le(out, 0xFFFF);
    u16le(out, name);
    u32le(out, 0); // DataVersion
    u16le(out, flags); // MemoryFlags
    u16le(out, 0x0409); // LanguageId (en-US)
    u32le(out, 0); // Version
    u32le(out, 0); // Characteristics
    out.extend_from_slice(data);
    pad4(out);
}

struct IcoImage {
    width: u8,
    height: u8,
    colors: u8,
    planes: u16,
    bit_count: u16,
    data: Vec<u8>,
}

fn parse_ico(bytes: &[u8]) -> Vec<IcoImage> {
    let rd16 = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let rd32 = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    assert!(bytes.len() >= 6 && rd16(0) == 0 && rd16(2) == 1, "not an .ico file");
    let count = rd16(4) as usize;
    (0..count)
        .map(|i| {
            let e = 6 + i * 16;
            let size = rd32(e + 8) as usize;
            let offset = rd32(e + 12) as usize;
            IcoImage {
                width: bytes[e],
                height: bytes[e + 1],
                colors: bytes[e + 2],
                planes: rd16(e + 4),
                bit_count: rd16(e + 6),
                data: bytes[offset..offset + size].to_vec(),
            }
        })
        .collect()
}

/// Generic VS_VERSIONINFO node.
fn ver_node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    utf16z(&mut b, key);
    pad4(&mut b);
    b.extend_from_slice(value);
    for child in children {
        pad4(&mut b);
        b.extend_from_slice(child);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&value_len.to_le_bytes());
    b[4..6].copy_from_slice(&(text as u16).to_le_bytes());
    b
}

fn ver_string(key: &str, value: &str) -> Vec<u8> {
    let mut v = Vec::new();
    utf16z(&mut v, value);
    ver_node(key, &v, (value.encode_utf16().count() + 1) as u16, true, &[])
}

fn version_info() -> Vec<u8> {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let parts: Vec<u16> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let (major, minor, patch) = (parts[0], *parts.get(1).unwrap_or(&0), *parts.get(2).unwrap_or(&0));
    let ms = ((major as u32) << 16) | minor as u32;
    let ls = (patch as u32) << 16;

    let mut fixed = Vec::new();
    for v in [
        0xFEEF04BD, // signature
        0x0001_0000, // struct version
        ms, ls,      // file version
        ms, ls,      // product version
        0x3F,        // flags mask
        0,           // flags
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0, 0, 0,     // subtype, date
    ] {
        u32le(&mut fixed, v);
    }

    let strings = [
        ("CompanyName", "Liquify"),
        ("FileDescription", "Mem Reduct (Rust)"),
        ("FileVersion", version.as_str()),
        ("InternalName", "memreduct"),
        ("LegalCopyright", "(c) 2026 Liquify. Based on Mem Reduct (c) 2011-2026 Henry++. GPL-3.0-or-later."),
        ("OriginalFilename", "memreduct.exe"),
        ("ProductName", "Mem Reduct (Rust)"),
        ("ProductVersion", version.as_str()),
    ];
    let table: Vec<Vec<u8>> = strings.iter().map(|(k, v)| ver_string(k, v)).collect();
    let string_table = ver_node("040904B0", &[], 0, true, &table);
    let string_file_info = ver_node("StringFileInfo", &[], 0, true, &[string_table]);

    let mut translation = Vec::new();
    u16le(&mut translation, 0x0409);
    u16le(&mut translation, 0x04B0);
    let var = ver_node("Translation", &translation, 4, false, &[]);
    let var_file_info = ver_node("VarFileInfo", &[], 0, true, &[var]);

    ver_node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_file_info, var_file_info])
}

fn build_res(icon_path: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    // leading empty entry marks a 32-bit .res file
    res_entry(&mut out, 0, 0, 0, &[]);

    let ico = std::fs::read(icon_path).expect("read icon");
    let images = parse_ico(&ico);

    let mut group = Vec::new();
    u16le(&mut group, 0);
    u16le(&mut group, 1);
    u16le(&mut group, images.len() as u16);
    for (i, img) in images.iter().enumerate() {
        let id = (i + 1) as u16;
        res_entry(&mut out, 3 /* RT_ICON */, id, 0x1010, &img.data);
        group.push(img.width);
        group.push(img.height);
        group.push(img.colors);
        group.push(0);
        u16le(&mut group, img.planes);
        u16le(&mut group, img.bit_count);
        u32le(&mut group, img.data.len() as u32);
        u16le(&mut group, id);
    }
    res_entry(&mut out, 14 /* RT_GROUP_ICON */, 1, 0x1030, &group);
    res_entry(&mut out, 16 /* RT_VERSION */, 1, 0x0030, &version_info());
    out
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let manifest = root.join("res").join("memreduct.manifest");
    let icon = root.join("res").join("memreduct.ico");

    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() != "msvc" {
        return;
    }

    let res_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("memreduct.res");
    std::fs::write(&res_path, build_res(&icon)).expect("write .res");

    println!("cargo:rustc-link-arg-bins={}", res_path.display());
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
}
