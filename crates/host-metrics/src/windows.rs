// SPDX-License-Identifier: AGPL-3.0-only

//! Windows implementation of the host-counter boundary.

use crate::{
    ForegroundSample, HostMetric, HostSample, HostSampler, ProcessLoadThresholds, ProcessOwnership,
    ProcessSample,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetProcessIoCounters, GetProcessTimes, GetSystemTimes, IO_COUNTERS, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
};

const CPU_COUNTER: &str = r"\Processor(*)\% Processor Time";
const DISK_QUEUE_COUNTER: &str = r"\PhysicalDisk(_Total)\Current Disk Queue Length";
const DISK_LATENCY_COUNTER: &str = r"\PhysicalDisk(_Total)\Avg. Disk sec/Transfer";
const GPU_COUNTER: &str = r"\GPU Engine(*)\Utilization Percentage";

#[derive(Clone, Copy)]
struct SystemTimes {
    idle: u64,
    kernel: u64,
    user: u64,
}

#[derive(Clone, Copy)]
struct ProcessCounters {
    cpu_100ns: u64,
    io_bytes: u64,
}

struct ProcessRecord {
    sample: ProcessSample,
    counters: ProcessCounters,
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the wrapper is created only for non-null handles returned by Win32 APIs.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct PdhQuery {
    handle: usize,
    cpu: Option<usize>,
    disk_queue: Option<usize>,
    disk_latency: Option<usize>,
    gpu: Option<usize>,
}

impl PdhQuery {
    fn open() -> Result<Self, &'static str> {
        let mut query: PDH_HQUERY = null_mut();
        // SAFETY: PDH initializes the out handle; the null data source requests live counters.
        if unsafe { PdhOpenQueryW(null(), 0, &mut query) } != 0 || query.is_null() {
            return Err("performance_pdh_open_failed");
        }
        let mut result = Self {
            handle: query as usize,
            cpu: None,
            disk_queue: None,
            disk_latency: None,
            gpu: None,
        };
        result.cpu = result.add_counter(CPU_COUNTER);
        result.disk_queue = result.add_counter(DISK_QUEUE_COUNTER);
        result.disk_latency = result.add_counter(DISK_LATENCY_COUNTER);
        result.gpu = result.add_counter(GPU_COUNTER);
        // The first collection seeds rate counters; individual unavailable counters remain explicit.
        if unsafe { PdhCollectQueryData(result.query()) } != 0 {
            return Err("performance_pdh_seed_failed");
        }
        Ok(result)
    }

    fn query(&self) -> PDH_HQUERY {
        self.handle as *mut c_void
    }

    fn add_counter(&self, path: &str) -> Option<usize> {
        let path = wide(path);
        let mut counter: PDH_HCOUNTER = null_mut();
        // SAFETY: the path is NUL-terminated and both handles remain valid for this query lifetime.
        let status = unsafe { PdhAddEnglishCounterW(self.query(), path.as_ptr(), 0, &mut counter) };
        (status == 0 && !counter.is_null()).then_some(counter as usize)
    }

    fn collect(&self) -> bool {
        // SAFETY: query is owned by this sampler and not used concurrently.
        unsafe { PdhCollectQueryData(self.query()) == 0 }
    }
}

impl Drop for PdhQuery {
    fn drop(&mut self) {
        // SAFETY: the query handle is owned exactly once by this value.
        unsafe {
            PdhCloseQuery(self.query());
        }
    }
}

pub(crate) struct WindowsHostSampler {
    previous_system: SystemTimes,
    previous_processes: BTreeMap<u32, ProcessCounters>,
    previous_observed_at_unix_ms: Option<u64>,
    pdh: PdhQuery,
}

impl WindowsHostSampler {
    pub(crate) fn new() -> Result<Self, &'static str> {
        Ok(Self {
            previous_system: system_times()?,
            previous_processes: BTreeMap::new(),
            previous_observed_at_unix_ms: None,
            pdh: PdhQuery::open()?,
        })
    }
}

impl HostSampler for WindowsHostSampler {
    fn sample(
        &mut self,
        observed_at_unix_ms: u64,
        owned_processes: &BTreeMap<u32, String>,
        top_process_count: usize,
        thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, &'static str> {
        if self
            .previous_observed_at_unix_ms
            .is_some_and(|previous| previous >= observed_at_unix_ms)
        {
            return Err("performance_sample_time_invalid");
        }
        let current_system = system_times()?;
        let total_delta = current_system
            .kernel
            .saturating_add(current_system.user)
            .saturating_sub(
                self.previous_system
                    .kernel
                    .saturating_add(self.previous_system.user),
            );
        let idle_delta = current_system
            .idle
            .saturating_sub(self.previous_system.idle);
        if total_delta == 0 || idle_delta > total_delta {
            return Err("performance_system_counter_invalid");
        }
        let cpu_total_basis_points = ratio_basis_points(total_delta - idle_delta, total_delta);
        let process_rates_available = self.previous_observed_at_unix_ms.is_some();
        let elapsed_ms = match self.previous_observed_at_unix_ms {
            Some(previous) => observed_at_unix_ms - previous,
            None => 0,
        };
        let process_records = process_samples(
            total_delta,
            elapsed_ms,
            &self.previous_processes,
            owned_processes,
        )?;
        let mut next_processes = BTreeMap::new();
        let mut all_processes = BTreeMap::new();
        for record in process_records {
            next_processes.insert(record.sample.pid, record.counters);
            all_processes.insert(record.sample.pid, record.sample);
        }
        let mut owned = all_processes
            .values()
            .filter(|process| process.ownership != ProcessOwnership::ThirdParty)
            .cloned()
            .collect::<Vec<_>>();
        owned.sort_by_key(|process| process.pid);
        let mut third_party_high_load = all_processes
            .values()
            .filter(|process| {
                process.ownership == ProcessOwnership::ThirdParty
                    && (process.cpu_basis_points >= thresholds.cpu_basis_points
                        || process.io_bytes_per_second >= thresholds.io_bytes_per_second)
            })
            .cloned()
            .collect::<Vec<_>>();
        third_party_high_load.sort_by(|left, right| {
            right
                .cpu_basis_points
                .cmp(&left.cpu_basis_points)
                .then_with(|| right.io_bytes_per_second.cmp(&left.io_bytes_per_second))
                .then_with(|| left.pid.cmp(&right.pid))
        });
        third_party_high_load.truncate(top_process_count);

        let pdh_collected = self.pdh.collect();
        let mut unavailable = BTreeSet::new();
        let cpu_per_core_basis_points = if pdh_collected {
            self.pdh
                .cpu
                .and_then(formatted_array)
                .map(|mut values| {
                    values.retain(|(name, _)| name != "_Total");
                    values.sort_by(|left, right| left.0.cmp(&right.0));
                    values
                        .into_iter()
                        .map(|(_, value)| percent_to_basis_points(value))
                        .collect::<Vec<_>>()
                })
                .filter(|values| !values.is_empty())
                .unwrap_or_else(|| {
                    unavailable.insert(HostMetric::CpuPerCore);
                    Vec::new()
                })
        } else {
            unavailable.insert(HostMetric::CpuPerCore);
            Vec::new()
        };
        let disk_queue_depth_milli = if pdh_collected {
            self.pdh
                .disk_queue
                .and_then(formatted_value)
                .map(nonnegative_to_u32_milli)
        } else {
            None
        };
        if disk_queue_depth_milli.is_none() {
            unavailable.insert(HostMetric::DiskQueueDepth);
        }
        let disk_latency_micros = if pdh_collected {
            self.pdh
                .disk_latency
                .and_then(formatted_value)
                .map(seconds_to_micros)
        } else {
            None
        };
        if disk_latency_micros.is_none() {
            unavailable.insert(HostMetric::DiskLatency);
        }
        let gpu_basis_points = if pdh_collected {
            self.pdh
                .gpu
                .and_then(formatted_array)
                .and_then(|values| {
                    (!values.is_empty()).then(|| {
                        values
                            .into_iter()
                            .map(|(_, value)| value.max(0.0))
                            .sum::<f64>()
                    })
                })
                .map(percent_to_basis_points)
        } else {
            None
        };
        if gpu_basis_points.is_none() {
            unavailable.insert(HostMetric::Gpu);
        }
        let foreground = foreground_summary(&all_processes);
        if foreground.is_none() {
            unavailable.insert(HostMetric::ForegroundProcess);
        }
        if !process_rates_available {
            unavailable.extend([HostMetric::ProcessCpu, HostMetric::ProcessIo]);
        }
        if owned_processes
            .keys()
            .any(|pid| !all_processes.contains_key(pid))
        {
            unavailable.extend([
                HostMetric::ProcessCpu,
                HostMetric::ProcessRam,
                HostMetric::ProcessIo,
            ]);
        }
        if owned.is_empty() {
            unavailable.insert(HostMetric::ProcessRam);
            unavailable.extend([HostMetric::ProcessCpu, HostMetric::ProcessIo]);
        }
        let ram_used_basis_points = memory_used_basis_points()?;
        self.previous_system = current_system;
        self.previous_processes = next_processes;
        self.previous_observed_at_unix_ms = Some(observed_at_unix_ms);
        Ok(HostSample {
            observed_at_unix_ms,
            cpu_total_basis_points,
            cpu_per_core_basis_points,
            ram_used_basis_points,
            disk_queue_depth_milli,
            disk_latency_micros,
            gpu_basis_points,
            foreground,
            owned_processes: owned,
            third_party_high_load,
            unavailable_metrics: unavailable.into_iter().collect(),
        })
    }
}

fn system_times() -> Result<SystemTimes, &'static str> {
    // SAFETY: all FILETIME out pointers are valid for the duration of the call.
    unsafe {
        let mut idle = zeroed();
        let mut kernel = zeroed();
        let mut user = zeroed();
        if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
            return Err("performance_system_times_failed");
        }
        Ok(SystemTimes {
            idle: filetime(idle),
            kernel: filetime(kernel),
            user: filetime(user),
        })
    }
}

fn memory_used_basis_points() -> Result<u16, &'static str> {
    // SAFETY: MEMORYSTATUSEX is initialized with its documented byte size.
    unsafe {
        let mut status: MEMORYSTATUSEX = zeroed();
        status.dwLength = u32::try_from(size_of::<MEMORYSTATUSEX>())
            .map_err(|_| "performance_memory_structure_invalid")?;
        if GlobalMemoryStatusEx(&mut status) == 0 || status.ullTotalPhys == 0 {
            return Err("performance_memory_status_failed");
        }
        Ok(ratio_basis_points(
            status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
            status.ullTotalPhys,
        ))
    }
}

fn process_samples(
    total_system_delta: u64,
    elapsed_ms: u64,
    previous: &BTreeMap<u32, ProcessCounters>,
    owned: &BTreeMap<u32, String>,
) -> Result<Vec<ProcessRecord>, &'static str> {
    // SAFETY: Toolhelp owns a stable snapshot; PROCESSENTRY32W has the required size.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err("performance_process_snapshot_failed");
        }
        let _snapshot = OwnedHandle(snapshot);
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = u32::try_from(size_of::<PROCESSENTRY32W>())
            .map_err(|_| "performance_process_structure_invalid")?;
        if Process32FirstW(snapshot, &mut entry) == 0 {
            return Err("performance_process_enumeration_failed");
        }
        let mut records = Vec::new();
        loop {
            if let Some(record) =
                process_record(&entry, total_system_delta, elapsed_ms, previous, owned)
            {
                records.push(record);
            }
            if Process32NextW(snapshot, &mut entry) == 0 {
                break;
            }
        }
        Ok(records)
    }
}

unsafe fn process_record(
    entry: &PROCESSENTRY32W,
    total_system_delta: u64,
    elapsed_ms: u64,
    previous: &BTreeMap<u32, ProcessCounters>,
    owned: &BTreeMap<u32, String>,
) -> Option<ProcessRecord> {
    let pid = entry.th32ProcessID;
    if pid == 0 {
        return None;
    }
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let _handle = OwnedHandle(handle);
    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    memory.cb = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
    let mut io: IO_COUNTERS = unsafe { zeroed() };
    if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0
        || unsafe { K32GetProcessMemoryInfo(handle, &mut memory, memory.cb) } == 0
        || unsafe { GetProcessIoCounters(handle, &mut io) } == 0
    {
        return None;
    }
    let counters = ProcessCounters {
        cpu_100ns: filetime(kernel).saturating_add(filetime(user)),
        io_bytes: io
            .ReadTransferCount
            .saturating_add(io.WriteTransferCount)
            .saturating_add(io.OtherTransferCount),
    };
    let previous = previous.get(&pid);
    let cpu_basis_points = previous
        .map(|value| {
            ratio_basis_points(
                counters.cpu_100ns.saturating_sub(value.cpu_100ns),
                total_system_delta,
            )
        })
        .unwrap_or(0);
    let io_bytes_per_second = previous
        .filter(|_| elapsed_ms > 0)
        .map(|value| {
            counters
                .io_bytes
                .saturating_sub(value.io_bytes)
                .saturating_mul(1_000)
                / elapsed_ms
        })
        .unwrap_or(0);
    let process_name = owned
        .get(&pid)
        .cloned()
        .unwrap_or_else(|| process_name(&entry.szExeFile));
    let ownership = if pid == std::process::id() {
        ProcessOwnership::Runtime
    } else if owned.contains_key(&pid) {
        ProcessOwnership::Owned
    } else {
        ProcessOwnership::ThirdParty
    };
    Some(ProcessRecord {
        sample: ProcessSample {
            pid,
            process_name,
            ownership,
            cpu_basis_points,
            working_set_bytes: memory.WorkingSetSize as u64,
            io_bytes_per_second,
        },
        counters,
    })
}

fn foreground_summary(processes: &BTreeMap<u32, ProcessSample>) -> Option<ForegroundSample> {
    // SAFETY: HWND is read-only and the PID/rect out pointers are valid.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return None;
        }
        let mut pid = 0;
        if GetWindowThreadProcessId(window, &mut pid) == 0 {
            return None;
        }
        let process = processes.get(&pid)?;
        Some(ForegroundSample {
            process: ProcessSample {
                pid: process.pid,
                process_name: process.process_name.clone(),
                ownership: process.ownership,
                cpu_basis_points: process.cpu_basis_points,
                working_set_bytes: process.working_set_bytes,
                io_bytes_per_second: process.io_bytes_per_second,
            },
            fullscreen: is_fullscreen(window)?,
        })
    }
}

unsafe fn is_fullscreen(window: *mut c_void) -> Option<bool> {
    let mut window_rect: RECT = unsafe { zeroed() };
    if unsafe { GetWindowRect(window, &mut window_rect) } == 0 {
        return None;
    }
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info: MONITORINFO = unsafe { zeroed() };
    info.cbSize = match u32::try_from(size_of::<MONITORINFO>()) {
        Ok(value) => value,
        Err(_) => return None,
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    Some(
        window_rect.left <= info.rcMonitor.left
            && window_rect.top <= info.rcMonitor.top
            && window_rect.right >= info.rcMonitor.right
            && window_rect.bottom >= info.rcMonitor.bottom,
    )
}

fn formatted_value(counter: usize) -> Option<f64> {
    // SAFETY: the counter handle remains owned by the query for this call.
    unsafe {
        let mut value: PDH_FMT_COUNTERVALUE = zeroed();
        if PdhGetFormattedCounterValue(
            counter as PDH_HCOUNTER,
            PDH_FMT_DOUBLE,
            null_mut(),
            &mut value,
        ) != 0
            || !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA)
        {
            return None;
        }
        let value = value.Anonymous.doubleValue;
        value.is_finite().then_some(value)
    }
}

fn formatted_array(counter: usize) -> Option<Vec<(String, f64)>> {
    // SAFETY: PDH reports the exact buffer size; the aligned u64 allocation remains alive while
    // item pointers and their embedded names are read.
    unsafe {
        let mut bytes = 0;
        let mut count = 0;
        let status = PdhGetFormattedCounterArrayW(
            counter as PDH_HCOUNTER,
            PDH_FMT_DOUBLE,
            &mut bytes,
            &mut count,
            null_mut(),
        );
        if status != PDH_MORE_DATA || bytes == 0 || count == 0 {
            return None;
        }
        let words = usize::try_from(bytes).ok()?.div_ceil(size_of::<u64>());
        let mut buffer = vec![0u64; words];
        let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        if PdhGetFormattedCounterArrayW(
            counter as PDH_HCOUNTER,
            PDH_FMT_DOUBLE,
            &mut bytes,
            &mut count,
            items,
        ) != 0
        {
            return None;
        }
        let mut values = Vec::new();
        for item in std::slice::from_raw_parts(items, usize::try_from(count).ok()?) {
            if !matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            ) {
                continue;
            }
            let value = item.FmtValue.Anonymous.doubleValue;
            if value.is_finite() && !item.szName.is_null() {
                values.push((wide_string(item.szName), value));
            }
        }
        Some(values)
    }
}

fn ratio_basis_points(numerator: u64, denominator: u64) -> u16 {
    if denominator == 0 {
        return 0;
    }
    let value =
        (u128::from(numerator).saturating_mul(10_000) / u128::from(denominator)).min(10_000);
    value as u16
}

fn percent_to_basis_points(value: f64) -> u16 {
    value.clamp(0.0, 100.0).mul_add(100.0, 0.0).round() as u16
}

fn nonnegative_to_u32_milli(value: f64) -> u32 {
    value
        .clamp(0.0, f64::from(u32::MAX) / 1_000.0)
        .mul_add(1_000.0, 0.0)
        .round() as u32
}

fn seconds_to_micros(value: f64) -> u64 {
    value
        .clamp(0.0, u64::MAX as f64 / 1_000_000.0)
        .mul_add(1_000_000.0, 0.0)
        .round() as u64
}

fn filetime(value: FILETIME) -> u64 {
    u64::from(value.dwLowDateTime) | (u64::from(value.dwHighDateTime) << 32)
}

fn process_name(buffer: &[u16]) -> String {
    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn wide_string(pointer: *const u16) -> String {
    let mut length = 0usize;
    while unsafe { *pointer.add(length) } != 0 && length < 1_024 {
        length += 1;
    }
    // SAFETY: PDH names are NUL-terminated inside the live query result buffer.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_basis_points_is_bounded_and_deterministic() {
        assert_eq!(ratio_basis_points(0, 0), 0);
        assert_eq!(ratio_basis_points(1, 4), 2_500);
        assert_eq!(ratio_basis_points(3, 2), 10_000);
    }

    #[test]
    fn counter_unit_conversions_clamp_invalid_ranges() {
        assert_eq!(percent_to_basis_points(-1.0), 0);
        assert_eq!(percent_to_basis_points(42.5), 4_250);
        assert_eq!(percent_to_basis_points(101.0), 10_000);
        assert_eq!(nonnegative_to_u32_milli(-1.0), 0);
        assert_eq!(nonnegative_to_u32_milli(1.25), 1_250);
        assert_eq!(seconds_to_micros(-1.0), 0);
        assert_eq!(seconds_to_micros(0.025), 25_000);
    }
}
