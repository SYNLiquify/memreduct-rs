//! INI-backed settings store.
//!
//! Portable mode: if `memreduct.ini` sits next to the executable it is used.
//! Otherwise settings live in `%APPDATA%\memreduct-rs\memreduct.ini`.
//! Key names match the original Mem Reduct so an old config can be copied over.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

const SECTION: &str = "memreduct";
const FILE_NAME: &str = "memreduct.ini";

struct Store {
    path: PathBuf,
    values: BTreeMap<String, String>,
    /// Lines from other sections are kept verbatim so we never destroy them.
    foreign: Vec<String>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn resolve_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let portable = dir.join(FILE_NAME);
            if portable.exists() {
                return portable;
            }
        }
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("memreduct-rs").join(FILE_NAME)
}

fn parse(text: &str) -> (BTreeMap<String, String>, Vec<String>) {
    let mut values = BTreeMap::new();
    let mut foreign = Vec::new();
    let mut in_ours = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_ours = line[1..line.len() - 1].trim().eq_ignore_ascii_case(SECTION);
            if !in_ours {
                foreign.push(raw.to_string());
            }
            continue;
        }
        if in_ours {
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                values.insert(k.trim().to_string(), v.trim().to_string());
            }
        } else {
            foreign.push(raw.to_string());
        }
    }
    (values, foreign)
}

fn serialize(values: &BTreeMap<String, String>, foreign: &[String]) -> String {
    let mut out = format!("[{}]\r\n", SECTION);
    for (k, v) in values {
        out.push_str(k);
        out.push('=');
        out.push_str(v);
        out.push_str("\r\n");
    }
    let rest: Vec<&String> = foreign.iter().filter(|l| !l.trim().is_empty()).collect();
    if !rest.is_empty() {
        out.push_str("\r\n");
        for l in rest {
            out.push_str(l);
            out.push_str("\r\n");
        }
    }
    out
}

pub fn init() {
    let path = resolve_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let (values, foreign) = parse(&text);
    *STORE.lock().unwrap() = Some(Store { path, values, foreign });
}

fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut guard = STORE.lock().unwrap();
    if guard.is_none() {
        drop(guard);
        init();
        guard = STORE.lock().unwrap();
    }
    f(guard.as_mut().unwrap())
}

fn save(store: &Store) {
    if let Some(dir) = store.path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&store.path, serialize(&store.values, &store.foreign));
}

/// Directory holding the config (and the log file).
pub fn dir() -> PathBuf {
    with(|s| s.path.parent().map(|p| p.to_path_buf()).unwrap_or_default())
}

pub fn path() -> PathBuf {
    with(|s| s.path.clone())
}

pub fn get_raw(key: &str) -> Option<String> {
    with(|s| s.values.get(key).cloned())
}

pub fn set_raw(key: &str, value: &str) {
    with(|s| {
        if s.values.get(key).map(|v| v == value).unwrap_or(false) {
            return;
        }
        s.values.insert(key.to_string(), value.to_string());
        save(s);
    })
}

pub fn remove(key: &str) {
    with(|s| {
        if s.values.remove(key).is_some() {
            save(s);
        }
    })
}

/// Drops every setting and rewrites the file.
pub fn reset() {
    with(|s| {
        s.values.clear();
        save(s);
    })
}

pub fn get_bool(key: &str, default: bool) -> bool {
    match get_raw(key) {
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => true,
            "false" | "0" | "no" | "off" => false,
            _ => default,
        },
        None => default,
    }
}

pub fn set_bool(key: &str, value: bool) {
    set_raw(key, if value { "true" } else { "false" });
}

/// Flips a boolean and returns the new value.
pub fn invert_bool(key: &str, default: bool) -> bool {
    let v = !get_bool(key, default);
    set_bool(key, v);
    v
}

pub fn get_u32(key: &str, default: u32) -> u32 {
    get_raw(key)
        .and_then(|v| {
            let v = v.trim();
            if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                v.parse::<i64>().ok().map(|n| n as u32)
            }
        })
        .unwrap_or(default)
}

pub fn set_u32(key: &str, value: u32) {
    set_raw(key, &value.to_string());
}

pub fn get_i64(key: &str, default: i64) -> i64 {
    get_raw(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

pub fn set_i64(key: &str, value: i64) {
    set_raw(key, &value.to_string());
}

pub fn get_string(key: &str, default: &str) -> String {
    get_raw(key).unwrap_or_else(|| default.to_string())
}

pub fn set_string(key: &str, value: &str) {
    set_raw(key, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_foreign_sections() {
        let text = "[other]\r\nx=1\r\n[memreduct]\r\nA=true\r\n; comment\r\nB = 42\r\n";
        let (values, foreign) = parse(text);
        assert_eq!(values.get("A").unwrap(), "true");
        assert_eq!(values.get("B").unwrap(), "42");
        assert_eq!(foreign, vec!["[other]".to_string(), "x=1".to_string()]);
        let out = serialize(&values, &foreign);
        let (v2, f2) = parse(&out);
        assert_eq!(v2, values);
        assert_eq!(f2, foreign);
    }
}
