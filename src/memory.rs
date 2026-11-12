//! Memory statistics and the actual cleaning, done through the Native API
//! (`NtSetSystemInformation`), the same calls the original Mem Reduct uses.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, FlushFileBuffers,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use crate::sys;
use crate::util::{from_wide, wide};

// ---------------------------------------------------------------------------
// Native API
// ---------------------------------------------------------------------------

const SYSTEM_FILE_CACHE_INFORMATION: u32 = 21;
const SYSTEM_MEMORY_LIST_INFORMATION: u32 = 80;
const SYSTEM_FILE_CACHE_INFORMATION_EX: u32 = 81;
const SYSTEM_COMBINE_PHYSICAL_MEMORY_INFORMATION: u32 = 130;
const SYSTEM_REGISTRY_RECONCILIATION_INFORMATION: u32 = 155;

// SYSTEM_MEMORY_LIST_COMMAND
const MEMORY_EMPTY_WORKING_SETS: i32 = 2;
const MEMORY_FLUSH_MODIFIED_LIST: i32 = 3;
const MEMORY_PURGE_STANDBY_LIST: i32 = 4;
const MEMORY_PURGE_LOW_PRIORITY_STANDBY_LIST: i32 = 5;

#[repr(C)]
#[derive(Default)]
struct SystemFileCacheInformation {
    current_size: usize,
    peak_size: usize,
    page_fault_count: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    current_size_including_transition_in_pages: usize,
    peak_size_including_transition_in_pages: usize,
    transition_repurpose_count: u32,
    flags: u32,
}

#[repr(C)]
struct MemoryCombineInformationEx {
    handle: HANDLE,
    pages_combined: usize,
    flags: u32,
}

#[link(name = "ntdll", kind = "raw-dylib")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret_len: *mut u32) -> i32;
    fn NtSetSystemInformation(class: u32, info: *mut c_void, len: u32) -> i32;
}

// ---------------------------------------------------------------------------
// Regions
// ---------------------------------------------------------------------------

pub const WORKINGSET: u32 = 0x01;
pub const SYSTEMFILECACHE: u32 = 0x02;
pub const STANDBYPRIORITY0LIST: u32 = 0x04;
pub const STANDBYLIST: u32 = 0x08;
pub const MODIFIEDLIST: u32 = 0x10;
pub const COMBINEMEMORYLISTS: u32 = 0x20;
pub const REGISTRYCACHE: u32 = 0x40;
pub const MODIFIEDFILECACHE: u32 = 0x80;

pub const MASK_ALL: u32 = WORKINGSET
    | SYSTEMFILECACHE
    | STANDBYPRIORITY0LIST
    | STANDBYLIST
    | MODIFIEDLIST
    | COMBINEMEMORYLISTS
    | REGISTRYCACHE
    | MODIFIEDFILECACHE;

pub const MASK_DEFAULT: u32 = WORKINGSET
    | SYSTEMFILECACHE
    | STANDBYPRIORITY0LIST
    | REGISTRYCACHE
    | COMBINEMEMORYLISTS
    | MODIFIEDFILECACHE;

/// Regions that can stall the machine for a while when purged.
pub const MASK_FREEZES: u32 = STANDBYLIST | MODIFIEDLIST;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MinOs {
    Any,
    Win81,
    Win10,
}

pub struct Region {
    pub mask: u32,
    pub name: &'static str,
    pub min_os: MinOs,
}

/// Display order used by every menu and list.
pub const REGIONS: [Region; 8] = [
    Region { mask: WORKINGSET, name: "Working set", min_os: MinOs::Any },
    Region { mask: SYSTEMFILECACHE, name: "System file cache", min_os: MinOs::Any },
    Region { mask: MODIFIEDFILECACHE, name: "Modified file cache", min_os: MinOs::Any },
    Region { mask: STANDBYPRIORITY0LIST, name: "Standby list (without priority)", min_os: MinOs::Any },
    Region { mask: STANDBYLIST, name: "Standby list", min_os: MinOs::Any },
    Region { mask: MODIFIEDLIST, name: "Modified page list", min_os: MinOs::Any },
    Region { mask: REGISTRYCACHE, name: "Registry cache", min_os: MinOs::Win81 },
    Region { mask: COMBINEMEMORYLISTS, name: "Combine memory lists", min_os: MinOs::Win10 },
];

impl Region {
    /// Name with the "*" freeze marker and OS requirement suffix.
    pub fn label(&self) -> String {
        let mut s = self.name.to_string();
        if self.mask & MASK_FREEZES != 0 {
            s.push('*');
        }
        match self.min_os {
            MinOs::Any => {}
            MinOs::Win81 => s.push_str(" (win8.1+)"),
            MinOs::Win10 => s.push_str(" (win10+)"),
        }
        s
    }

    pub fn supported(&self) -> bool {
        match self.min_os {
            MinOs::Any => true,
            MinOs::Win81 => sys::is_win81_or_later(),
            MinOs::Win10 => sys::is_win10_or_later(),
        }
    }
}

/// Bullet list of the regions in `mask`, used by confirmations and results.
pub fn describe_mask(mask: u32) -> String {
    REGIONS
        .iter()
        .filter(|r| mask & r.mask != 0)
        .map(|r| format!("- {}", r.label()))
        .collect::<Vec<_>>()
        .join("\r\n")
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default, Debug)]
pub struct Usage {
    pub total: u64,
    pub free: u64,
    pub used: u64,
    pub percent: u32,
    pub percent_f: f64,
}

impl Usage {
    fn new(total: u64, free: u64) -> Self {
        let free = free.min(total);
        let used = total - free;
        let percent_f = if total == 0 { 0.0 } else { used as f64 * 100.0 / total as f64 };
        Usage { total, free, used, percent: percent_f.round() as u32, percent_f }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct MemInfo {
    pub physical: Usage,
    pub page_file: Usage,
    pub system_cache: Usage,
}

pub fn query() -> MemInfo {
    let mut info = MemInfo::default();

    let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    ms.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut ms) } != 0 {
        info.physical = Usage::new(ms.ullTotalPhys, ms.ullAvailPhys);
        info.page_file = Usage::new(ms.ullTotalPageFile, ms.ullAvailPageFile);
    }

    let mut sfci = SystemFileCacheInformation::default();
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_FILE_CACHE_INFORMATION,
            &mut sfci as *mut _ as *mut c_void,
            std::mem::size_of::<SystemFileCacheInformation>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status >= 0 {
        let peak = sfci.peak_size as u64;
        let current = (sfci.current_size as u64).min(peak);
        info.system_cache = Usage::new(peak, peak - current);
    }

    info
}

// ---------------------------------------------------------------------------
// Cleaning
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct CleanResult {
    pub mask: u32,
    pub freed: u64,
    pub errors: Vec<String>,
}

fn nt_set(class: u32, data: *mut c_void, len: u32, what: &str, errors: &mut Vec<String>) {
    let status = unsafe { NtSetSystemInformation(class, data, len) };
    if status < 0 {
        errors.push(format!("{} failed (NTSTATUS 0x{:08X})", what, status as u32));
    }
}

fn memory_list_command(command: i32, what: &str, errors: &mut Vec<String>) {
    let mut cmd = command;
    nt_set(
        SYSTEM_MEMORY_LIST_INFORMATION,
        &mut cmd as *mut i32 as *mut c_void,
        std::mem::size_of::<i32>() as u32,
        what,
        errors,
    );
}

/// Flushes the write cache of every mounted volume.
fn flush_volume_cache(errors: &mut Vec<String>) {
    let mut name = [0u16; 512];
    let find = unsafe { FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32) };
    if find == INVALID_HANDLE_VALUE {
        errors.push("FindFirstVolumeW failed".to_string());
        return;
    }
    loop {
        // "\\?\Volume{GUID}\" -> open the volume device itself (no trailing slash)
        let mut path = from_wide(&name);
        if path.ends_with('\\') {
            path.pop();
        }
        let wpath = wide(&path);
        let handle = unsafe {
            CreateFileW(
                wpath.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            unsafe {
                FlushFileBuffers(handle);
                CloseHandle(handle);
            }
        }
        if unsafe { FindNextVolumeW(find, name.as_mut_ptr(), name.len() as u32) } == 0 {
            break;
        }
    }
    unsafe { FindVolumeClose(find) };
}

/// Cleans every region in `mask`. Needs administrator rights and the
/// privileges enabled by [`sys::enable_cleaning_privileges`].
pub fn clean(mask: u32) -> CleanResult {
    let mut errors = Vec::new();
    let before = query().physical.used;

    if mask & WORKINGSET != 0 {
        memory_list_command(MEMORY_EMPTY_WORKING_SETS, "Empty working sets", &mut errors);
    }

    if mask & SYSTEMFILECACHE != 0 {
        let mut sfci = SystemFileCacheInformation {
            minimum_working_set: usize::MAX,
            maximum_working_set: usize::MAX,
            ..Default::default()
        };
        nt_set(
            SYSTEM_FILE_CACHE_INFORMATION_EX,
            &mut sfci as *mut _ as *mut c_void,
            std::mem::size_of::<SystemFileCacheInformation>() as u32,
            "System file cache",
            &mut errors,
        );
    }

    if mask & MODIFIEDFILECACHE != 0 {
        flush_volume_cache(&mut errors);
    }

    if mask & MODIFIEDLIST != 0 {
        memory_list_command(MEMORY_FLUSH_MODIFIED_LIST, "Flush modified list", &mut errors);
    }

    if mask & STANDBYLIST != 0 {
        memory_list_command(MEMORY_PURGE_STANDBY_LIST, "Purge standby list", &mut errors);
    }

    if mask & STANDBYPRIORITY0LIST != 0 {
        memory_list_command(
            MEMORY_PURGE_LOW_PRIORITY_STANDBY_LIST,
            "Purge low-priority standby list",
            &mut errors,
        );
    }

    if mask & REGISTRYCACHE != 0 && sys::is_win81_or_later() {
        nt_set(
            SYSTEM_REGISTRY_RECONCILIATION_INFORMATION,
            std::ptr::null_mut(),
            0,
            "Registry reconciliation",
            &mut errors,
        );
    }

    if mask & COMBINEMEMORYLISTS != 0 && sys::is_win10_or_later() {
        let mut combine = MemoryCombineInformationEx {
            handle: std::ptr::null_mut(),
            pages_combined: 0,
            flags: 0,
        };
        nt_set(
            SYSTEM_COMBINE_PHYSICAL_MEMORY_INFORMATION,
            &mut combine as *mut _ as *mut c_void,
            std::mem::size_of::<MemoryCombineInformationEx>() as u32,
            "Combine memory lists",
            &mut errors,
        );
    }

    let after = query().physical.used;

    CleanResult { mask, freed: before.saturating_sub(after), errors }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_math() {
        let u = Usage::new(1000, 250);
        assert_eq!(u.used, 750);
        assert_eq!(u.percent, 75);
        let z = Usage::new(0, 0);
        assert_eq!(z.percent, 0);
    }

    #[test]
    fn query_returns_something() {
        let info = query();
        assert!(info.physical.total > 0);
        assert!(info.physical.percent <= 100);
    }

    #[test]
    fn masks() {
        assert_eq!(MASK_ALL, 0xFF);
        assert_eq!(REGIONS.iter().fold(0, |m, r| m | r.mask), MASK_ALL);
        assert!(describe_mask(STANDBYLIST).contains("Standby list*"));
    }
}
