// SPDX-License-Identifier: AGPL-3.0-only

//! The Windows samplers (design §1.5 item 4): the process priority, `GetProcessTimes`,
//! `EnumProcessModules` with each module's file version, the GPU performance counters (PDH, as
//! `crates/host-metrics/src/windows.rs` reads them) and the DXGI adapter list (a minimal
//! hand-written vtable: `windows-sys` has no COM interfaces). Memory comes from
//! `actingcommand-host-metrics` (`GlobalMemoryStatusEx`, `GetProcessMemoryInfo`).

use crate::error::{BenchCode, BenchError};
use crate::report::{DxgiAdapter, ModuleRecord};
use crate::safety::MemoryReader;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{FILETIME, GetLastError, HMODULE};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows_sys::Win32::System::ProcessStatus::{K32EnumProcessModules, K32GetModuleFileNameExW};
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetCurrentProcessId, GetPriorityClass,
    GetProcessTimes, SetPriorityClass, TerminateProcess,
};

/// `BELOW_NORMAL_PRIORITY_CLASS`.
pub const BELOW_NORMAL_CLASS: u32 = BELOW_NORMAL_PRIORITY_CLASS;

fn last_error() -> u32 {
    // SAFETY: reads the calling thread's last-error value.
    unsafe { GetLastError() }
}

fn sampler(message: impl Into<String>) -> BenchError {
    BenchError::new(BenchCode::SamplerFailed, message)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn current_process_id() -> u32 {
    // SAFETY: no arguments; returns this process's id.
    unsafe { GetCurrentProcessId() }
}

/// Sets this process to `BELOW_NORMAL_PRIORITY_CLASS` and reads the class back; a refused set or
/// a different read-back is `bench_priority_unverified`.
pub fn set_below_normal_priority() -> Result<u32, BenchError> {
    // SAFETY: the pseudo handle of the current process needs no closing.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: a valid process handle and a documented priority class.
    if unsafe { SetPriorityClass(process, BELOW_NORMAL_PRIORITY_CLASS) } == 0 {
        return Err(BenchError::new(
            BenchCode::PriorityUnverified,
            format!(
                "SetPriorityClass(BELOW_NORMAL_PRIORITY_CLASS) failed with error {}",
                last_error()
            ),
        ));
    }
    // SAFETY: a valid process handle.
    let class = unsafe { GetPriorityClass(process) };
    if class == 0 {
        return Err(BenchError::new(
            BenchCode::PriorityUnverified,
            format!("GetPriorityClass failed with error {}", last_error()),
        ));
    }
    if class != BELOW_NORMAL_PRIORITY_CLASS {
        return Err(BenchError::new(
            BenchCode::PriorityUnverified,
            format!(
                "the priority class reads back 0x{class:x}, not BELOW_NORMAL_PRIORITY_CLASS 0x{BELOW_NORMAL_PRIORITY_CLASS:x}"
            ),
        ));
    }
    Ok(class)
}

/// This process's user plus kernel time, in microseconds (`GetProcessTimes`).
pub fn process_cpu_time_us() -> Result<u64, BenchError> {
    // SAFETY: FILETIME is plain data.
    let (mut creation, mut exit, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) =
        unsafe { (zeroed(), zeroed(), zeroed(), zeroed()) };
    // SAFETY: the pseudo handle and four valid out pointers.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return Err(sampler(format!(
            "GetProcessTimes failed with error {}",
            last_error()
        )));
    }
    Ok((filetime(kernel) + filetime(user)) / 10)
}

fn filetime(value: FILETIME) -> u64 {
    u64::from(value.dwLowDateTime) | (u64::from(value.dwHighDateTime) << 32)
}

/// Ends the process at once with `status`, without running any library's shutdown (a safety
/// stop must not wait for an inference to finish).
pub fn terminate_now(status: i32) -> ! {
    // SAFETY: the pseudo handle of the current process; the call does not return on success.
    unsafe {
        TerminateProcess(GetCurrentProcess(), status as u32);
    }
    std::process::exit(status)
}

/// The host's available memory and this process's private bytes.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsMemoryReader;

impl MemoryReader for OsMemoryReader {
    fn host_available_bytes(&mut self) -> Result<u64, BenchError> {
        actingcommand_host_metrics::sample_physical_memory()
            .map(|sample| sample.available_bytes)
            .map_err(|code| sampler(format!("the host memory status failed: {code}")))
    }

    fn own_private_bytes(&mut self) -> Result<u64, BenchError> {
        actingcommand_host_metrics::sample_own_process_memory()
            .map(|sample| sample.private_bytes)
            .map_err(|code| sampler(format!("GetProcessMemoryInfo failed: {code}")))
    }
}

/// Every module loaded in this process, with its file version when it has one.
pub fn loaded_modules() -> Result<Vec<ModuleRecord>, BenchError> {
    // SAFETY: the pseudo handle of the current process.
    let process = unsafe { GetCurrentProcess() };
    let mut handles: Vec<HMODULE> = vec![null_mut(); 512];
    loop {
        let capacity = u32::try_from(handles.len() * size_of::<HMODULE>())
            .map_err(|_| sampler("the module list is too large"))?;
        let mut needed = 0u32;
        // SAFETY: the buffer holds `capacity` bytes of module handles.
        if unsafe { K32EnumProcessModules(process, handles.as_mut_ptr(), capacity, &mut needed) }
            == 0
        {
            return Err(sampler(format!(
                "EnumProcessModules failed with error {}",
                last_error()
            )));
        }
        let count = needed as usize / size_of::<HMODULE>();
        if needed <= capacity {
            handles.truncate(count);
            break;
        }
        handles = vec![null_mut(); count + 64];
    }
    let mut modules = Vec::with_capacity(handles.len());
    let mut buffer = vec![0u16; 32_768];
    for handle in handles {
        // SAFETY: a module handle of this process and a buffer of the stated length.
        let length = unsafe {
            K32GetModuleFileNameExW(process, handle, buffer.as_mut_ptr(), buffer.len() as u32)
        };
        if length == 0 {
            return Err(sampler(format!(
                "GetModuleFileNameEx failed with error {}",
                last_error()
            )));
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        let file_version = file_version(Path::new(&path));
        modules.push(ModuleRecord { path, file_version });
    }
    modules.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(modules)
}

/// The fixed file version `a.b.c.d` of a file, when it has a version resource.
pub fn file_version(path: &Path) -> Option<String> {
    let name = wide_path(path);
    let mut handle = 0u32;
    // SAFETY: a NUL-terminated path.
    let size = unsafe { GetFileVersionInfoSizeW(name.as_ptr(), &mut handle) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: the buffer holds `size` bytes.
    if unsafe { GetFileVersionInfoW(name.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let root = wide("\\");
    let mut info: *mut c_void = null_mut();
    let mut length = 0u32;
    // SAFETY: the version block read above and a NUL-terminated sub-block name.
    if unsafe { VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut info, &mut length) } == 0
        || info.is_null()
        || (length as usize) < size_of::<VS_FIXEDFILEINFO>()
    {
        return None;
    }
    // SAFETY: `info` points at a VS_FIXEDFILEINFO inside `data`, which is still alive; it is read
    // unaligned because the block is a byte buffer.
    let fixed: VS_FIXEDFILEINFO = unsafe { std::ptr::read_unaligned(info.cast()) };
    Some(format!(
        "{}.{}.{}.{}",
        fixed.dwFileVersionMS >> 16,
        fixed.dwFileVersionMS & 0xffff,
        fixed.dwFileVersionLS >> 16,
        fixed.dwFileVersionLS & 0xffff
    ))
}

// The GPU performance counters.

const GPU_DEDICATED_COUNTER: &str = r"\GPU Process Memory(*)\Dedicated Usage";
const GPU_SHARED_COUNTER: &str = r"\GPU Process Memory(*)\Shared Usage";
const GPU_ENGINE_COUNTER: &str = r"\GPU Engine(*)\Utilization Percentage";

/// One sample of one GPU (by LUID).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GpuSample {
    pub own_dedicated_bytes: u64,
    pub own_shared_bytes: u64,
    /// The busiest engine of this GPU counting only this process.
    pub own_busiest_engine_percent: f64,
    /// The busiest engine of this GPU counting every process.
    pub host_busiest_engine_percent: f64,
}

/// The PDH query of the three GPU counters; vendor neutral (CUDA and D3D12 both count).
pub struct GpuCounters {
    query: PDH_HQUERY,
    dedicated: PDH_HCOUNTER,
    shared: PDH_HCOUNTER,
    engine: PDH_HCOUNTER,
}

impl GpuCounters {
    pub fn open() -> Result<Self, BenchError> {
        let mut query: PDH_HQUERY = null_mut();
        // SAFETY: PDH fills the out handle; the null data source asks for live counters.
        let status = unsafe { PdhOpenQueryW(null(), 0, &mut query) };
        if status != 0 || query.is_null() {
            return Err(sampler(format!(
                "PdhOpenQuery failed with status 0x{status:08x}"
            )));
        }
        let mut counters = Self {
            query,
            dedicated: null_mut(),
            shared: null_mut(),
            engine: null_mut(),
        };
        counters.dedicated = counters.add(GPU_DEDICATED_COUNTER)?;
        counters.shared = counters.add(GPU_SHARED_COUNTER)?;
        counters.engine = counters.add(GPU_ENGINE_COUNTER)?;
        // The first collection seeds the rate counter.
        if !counters.collect() {
            return Err(sampler("the first PdhCollectQueryData failed"));
        }
        Ok(counters)
    }

    fn add(&self, path: &str) -> Result<PDH_HCOUNTER, BenchError> {
        let name = wide(path);
        let mut counter: PDH_HCOUNTER = null_mut();
        // SAFETY: a NUL-terminated path and the query owned by `self`.
        let status = unsafe { PdhAddEnglishCounterW(self.query, name.as_ptr(), 0, &mut counter) };
        if status != 0 || counter.is_null() {
            return Err(sampler(format!(
                "PdhAddEnglishCounter {path} failed with status 0x{status:08x}"
            )));
        }
        Ok(counter)
    }

    pub fn collect(&self) -> bool {
        // SAFETY: the query owned by `self`, used from one thread.
        unsafe { PdhCollectQueryData(self.query) == 0 }
    }

    /// The last collection's values for process `pid`, per GPU LUID.
    pub fn sample(&self, pid: u32) -> BTreeMap<String, GpuSample> {
        let own = format!("pid_{pid}_");
        let mut gpus = BTreeMap::<String, GpuSample>::new();
        for (instance, value) in formatted_array(self.dedicated) {
            if instance.starts_with(&own)
                && let Some(luid) = luid_of(&instance)
            {
                gpus.entry(luid).or_default().own_dedicated_bytes += value.max(0.0) as u64;
            }
        }
        for (instance, value) in formatted_array(self.shared) {
            if instance.starts_with(&own)
                && let Some(luid) = luid_of(&instance)
            {
                gpus.entry(luid).or_default().own_shared_bytes += value.max(0.0) as u64;
            }
        }
        // Per engine (the instance name without its pid), summed over processes.
        let mut host_engines = BTreeMap::<String, f64>::new();
        let mut own_engines = BTreeMap::<String, f64>::new();
        for (instance, value) in formatted_array(self.engine) {
            let Some(engine) = engine_key(&instance) else {
                continue;
            };
            *host_engines.entry(engine.to_owned()).or_default() += value;
            if instance.starts_with(&own) {
                *own_engines.entry(engine.to_owned()).or_default() += value;
            }
        }
        for (engines, own_side) in [(&host_engines, false), (&own_engines, true)] {
            for (engine, value) in engines {
                let Some(luid) = luid_of_engine(engine) else {
                    continue;
                };
                let sample = gpus.entry(luid).or_default();
                let slot = if own_side {
                    &mut sample.own_busiest_engine_percent
                } else {
                    &mut sample.host_busiest_engine_percent
                };
                *slot = slot.max(*value);
            }
        }
        gpus
    }
}

impl Drop for GpuCounters {
    fn drop(&mut self) {
        // SAFETY: the query is owned exactly once by this value.
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

/// `pid_<n>_luid_0x..._0x..._phys_0[_eng_<k>_engtype_<t>]` → the part after `pid_<n>_`.
fn engine_key(instance: &str) -> Option<&str> {
    let rest = instance.strip_prefix("pid_")?;
    let (pid, tail) = rest.split_once('_')?;
    pid.parse::<u32>().ok()?;
    Some(tail)
}

/// The LUID of an instance name, as `0x<high>_0x<low>`.
fn luid_of(instance: &str) -> Option<String> {
    luid_of_engine(engine_key(instance)?)
}

fn luid_of_engine(engine: &str) -> Option<String> {
    let rest = engine.strip_prefix("luid_")?;
    let mut parts = rest.splitn(3, '_');
    let high = parts.next()?;
    let low = parts.next()?;
    Some(format!("{high}_{low}"))
}

fn formatted_array(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
    // SAFETY: PDH reports the exact buffer size; the u64 buffer keeps the items aligned and stays
    // alive while their embedded names are read.
    unsafe {
        let mut bytes = 0u32;
        let mut count = 0u32;
        let status = PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut bytes,
            &mut count,
            null_mut(),
        );
        if status != PDH_MORE_DATA || bytes == 0 || count == 0 {
            return Vec::new();
        }
        let words = (bytes as usize).div_ceil(size_of::<u64>());
        let mut buffer = vec![0u64; words];
        let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, items) != 0
        {
            return Vec::new();
        }
        let mut values = Vec::with_capacity(count as usize);
        for item in std::slice::from_raw_parts(items, count as usize) {
            if !matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            ) || item.szName.is_null()
            {
                continue;
            }
            let value = item.FmtValue.Anonymous.doubleValue;
            if value.is_finite() {
                values.push((wide_string(item.szName), value));
            }
        }
        values
    }
}

/// # Safety
/// `pointer` is a NUL-terminated UTF-16 string.
unsafe fn wide_string(pointer: *const u16) -> String {
    let mut length = 0usize;
    // SAFETY: the caller guarantees the terminator; the scan is bounded.
    while length < 1_024 && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units were just read.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) })
}

// DXGI, through a minimal vtable.

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl Guid {
    fn text(&self) -> String {
        let d = &self.data4;
        format!(
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

/// `IID_IDXGIFactory1` {770aae78-f26f-4dba-a829-253c83d1b387}.
const IID_IDXGI_FACTORY1: Guid = Guid {
    data1: 0x770a_ae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};

#[repr(C)]
struct Luid {
    low_part: u32,
    high_part: i32,
}

/// `DXGI_ADAPTER_DESC1`.
#[repr(C)]
struct DxgiAdapterDesc1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: Luid,
    flags: u32,
}

const DXGI_ERROR_NOT_FOUND: i32 = 0x887A_0002_u32 as i32;
const DXGI_ADAPTER_FLAG_REMOTE: u32 = 1;
const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;
/// The adapters the list reads at most.
const MAX_ADAPTERS: u32 = 64;

type CreateDxgiFactory1 = unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;
type EnumAdapters1 = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
type GetDesc1 = unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32;

/// IUnknown 0-2, IDXGIObject 3-6, IDXGIFactory 7-11, IDXGIFactory1 12 `EnumAdapters1`.
const FACTORY1_ENUM_ADAPTERS1: usize = 12;
/// IUnknown 0-2, IDXGIObject 3-6, IDXGIAdapter 7-9, IDXGIAdapter1 10 `GetDesc1`.
const ADAPTER1_GET_DESC1: usize = 10;
const UNKNOWN_RELEASE: usize = 2;

/// One COM object this module owns a reference to.
struct ComObject(*mut c_void);

impl ComObject {
    /// # Safety
    /// `index` is a slot of the object's vtable whose function has type `F`.
    unsafe fn method<F: Copy>(&self, index: usize) -> F {
        // SAFETY: a COM object starts with its vtable pointer; the caller names a valid slot.
        unsafe {
            let vtable = *(self.0 as *const *const usize);
            let entry = *vtable.add(index);
            std::mem::transmute_copy::<usize, F>(&entry)
        }
    }
}

impl Drop for ComObject {
    fn drop(&mut self) {
        // SAFETY: slot 2 of every COM vtable is IUnknown::Release; this value owns one reference.
        unsafe {
            let release: Release = self.method(UNKNOWN_RELEASE);
            release(self.0);
        }
    }
}

/// The DXGI adapters in `IDXGIFactory1::EnumAdapters1` order.
pub fn dxgi_adapters() -> Result<Vec<DxgiAdapter>, BenchError> {
    let library = wide("dxgi.dll");
    // SAFETY: a NUL-terminated name, loaded from System32 only. The module stays loaded.
    let module =
        unsafe { LoadLibraryExW(library.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
    if module.is_null() {
        return Err(sampler(format!(
            "dxgi.dll did not load from System32: error {}",
            last_error()
        )));
    }
    // SAFETY: a loaded module and a NUL-terminated export name.
    let Some(export) = (unsafe { GetProcAddress(module, c"CreateDXGIFactory1".as_ptr().cast()) })
    else {
        return Err(sampler(format!(
            "dxgi.dll has no CreateDXGIFactory1: error {}",
            last_error()
        )));
    };
    // SAFETY: the documented signature of CreateDXGIFactory1.
    let create = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateDxgiFactory1>(export)
    };
    let mut factory: *mut c_void = null_mut();
    // SAFETY: a valid IID and out pointer.
    let result = unsafe { create(&IID_IDXGI_FACTORY1, &mut factory) };
    if result < 0 || factory.is_null() {
        return Err(sampler(format!(
            "CreateDXGIFactory1({{{}}}) failed with HRESULT 0x{:08x}",
            IID_IDXGI_FACTORY1.text(),
            result as u32
        )));
    }
    let factory = ComObject(factory);
    let mut adapters = Vec::new();
    for index in 0..MAX_ADAPTERS {
        let mut adapter: *mut c_void = null_mut();
        // SAFETY: slot 12 of IDXGIFactory1 is EnumAdapters1.
        let result = unsafe {
            let enum_adapters: EnumAdapters1 = factory.method(FACTORY1_ENUM_ADAPTERS1);
            enum_adapters(factory.0, index, &mut adapter)
        };
        if result == DXGI_ERROR_NOT_FOUND {
            break;
        }
        if result < 0 || adapter.is_null() {
            return Err(sampler(format!(
                "IDXGIFactory1::EnumAdapters1({index}) failed with HRESULT 0x{:08x}",
                result as u32
            )));
        }
        let adapter = ComObject(adapter);
        // SAFETY: plain data, filled by GetDesc1.
        let mut desc: DxgiAdapterDesc1 = unsafe { zeroed() };
        // SAFETY: slot 10 of IDXGIAdapter1 is GetDesc1.
        let result = unsafe {
            let get_desc: GetDesc1 = adapter.method(ADAPTER1_GET_DESC1);
            get_desc(adapter.0, &mut desc)
        };
        if result < 0 {
            return Err(sampler(format!(
                "IDXGIAdapter1::GetDesc1 of adapter {index} failed with HRESULT 0x{:08x}",
                result as u32
            )));
        }
        let length = desc
            .description
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(desc.description.len());
        adapters.push(DxgiAdapter {
            index,
            description: String::from_utf16_lossy(&desc.description[..length]),
            vendor_id: format!("0x{:04x}", desc.vendor_id),
            device_id: format!("0x{:04x}", desc.device_id),
            sub_sys_id: format!("0x{:08x}", desc.sub_sys_id),
            revision: desc.revision,
            dedicated_video_memory_bytes: desc.dedicated_video_memory as u64,
            dedicated_system_memory_bytes: desc.dedicated_system_memory as u64,
            shared_system_memory_bytes: desc.shared_system_memory as u64,
            luid: format!(
                "0x{:08X}_0x{:08X}",
                desc.adapter_luid.high_part as u32, desc.adapter_luid.low_part
            ),
            flags: desc.flags,
            software: desc.flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0,
            remote: desc.flags & DXGI_ADAPTER_FLAG_REMOTE != 0,
        });
    }
    Ok(adapters)
}
