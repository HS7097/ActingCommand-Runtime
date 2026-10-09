// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375: the machine's local time, shared by the frame cleaner and the frame listing,
//! which name kept folders and frame files in local time.

/// The machine's offset from UTC at the UTC instant `unix_ms`, in milliseconds east of UTC,
/// daylight saving time included; `None` when the operating system cannot convert the instant.
#[cfg(windows)]
pub fn machine_local_offset_ms(unix_ms: u64) -> Option<i64> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };
    /// FILETIME counts 100 ns intervals from 1601-01-01; the Unix epoch is this far after it.
    const UNIX_EPOCH_FILETIME_MS: u64 = 11_644_473_600_000;
    let ticks = unix_ms
        .checked_add(UNIX_EPOCH_FILETIME_MS)?
        .checked_mul(10_000)?;
    let universal_file = FILETIME {
        dwLowDateTime: (ticks & 0xffff_ffff) as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let empty = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    let mut universal = empty;
    let mut local = empty;
    let mut local_file = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: each call reads and writes only the local values it is given, which outlive it;
    // a null time zone pointer selects the machine's current time zone.
    let converted = unsafe {
        FileTimeToSystemTime(&universal_file, &mut universal) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &universal, &mut local) != 0
            && SystemTimeToFileTime(&local, &mut local_file) != 0
    };
    if !converted {
        return None;
    }
    let local_ticks =
        (u64::from(local_file.dwHighDateTime) << 32) | u64::from(local_file.dwLowDateTime);
    i64::try_from((i128::from(local_ticks) - i128::from(ticks)) / 10_000).ok()
}

/// The Runtime runs on Windows only; elsewhere the local offset is unknown.
#[cfg(not(windows))]
pub fn machine_local_offset_ms(_unix_ms: u64) -> Option<i64> {
    None
}
