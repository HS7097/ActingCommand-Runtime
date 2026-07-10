// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome};
use actingcommand_lab::MemorySample;

#[cfg(windows)]
pub(super) fn sample_system_memory() -> CliOutcome<MemorySample> {
    use std::mem::{MaybeUninit, size_of};

    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MaybeUninit::<MemoryStatusEx>::zeroed();
    let status_ptr = status.as_mut_ptr();
    unsafe {
        (*status_ptr).dw_length = size_of::<MemoryStatusEx>() as u32;
        if GlobalMemoryStatusEx(status_ptr) == 0 {
            return Err(CliError::device("GlobalMemoryStatusEx failed"));
        }
        let status = status.assume_init();
        Ok(MemorySample {
            total_bytes: status.ull_total_phys,
            available_bytes: status.ull_avail_phys,
        })
    }
}

#[cfg(not(windows))]
pub(super) fn sample_system_memory() -> CliOutcome<MemorySample> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .map_err(|err| CliError::device(format!("failed to read /proc/meminfo: {err}")))?;
    let mut total = None;
    let mut available = None;
    for line in meminfo.lines() {
        if let Some(value) = parse_meminfo_kib(line, "MemTotal:") {
            total = Some(value);
        } else if let Some(value) = parse_meminfo_kib(line, "MemAvailable:") {
            available = Some(value);
        }
    }
    Ok(MemorySample {
        total_bytes: total
            .ok_or_else(|| CliError::device("MemTotal missing from /proc/meminfo"))?,
        available_bytes: available
            .ok_or_else(|| CliError::device("MemAvailable missing from /proc/meminfo"))?,
    })
}

#[cfg(not(windows))]
fn parse_meminfo_kib(line: &str, prefix: &str) -> Option<u64> {
    let rest = line.strip_prefix(prefix)?;
    let kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(kib.saturating_mul(1024))
}
