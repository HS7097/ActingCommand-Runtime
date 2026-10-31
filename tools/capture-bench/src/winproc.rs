// SPDX-License-Identifier: AGPL-3.0-only

//! The bench's few Windows process calls: its job object (§3.11.5: the viewers and `typeperf`
//! end with the bench), its priority class (§3.10 item 5), its creation time (the forced-stop
//! identity) and the raw performance counter (the cross-process time base, §5.1).

use crate::plan::PriorityClass;

#[cfg(windows)]
mod imp {
    use super::PriorityClass;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    use windows_sys::Win32::System::Threading::{
        ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess,
        GetPriorityClass, GetProcessTimes, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        SetPriorityClass,
    };

    /// The job object that holds the bench's children; closing its handle ends them.
    pub struct Job {
        handle: HANDLE,
    }

    // The handle is a kernel object handle owned by this value alone.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn create() -> Result<Self, String> {
            // SAFETY: no security attributes and no name; the call returns a new handle or null.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(format!(
                    "CreateJobObjectW failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // SAFETY: a plain-data structure; all-zero is its empty value.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the handle is the job just created; the pointer and size describe `limits`.
            let set = unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if set == 0 {
                let error = std::io::Error::last_os_error();
                // SAFETY: the handle is ours and closed once.
                unsafe { CloseHandle(handle) };
                return Err(format!("SetInformationJobObject failed: {error}"));
            }
            Ok(Self { handle })
        }

        pub fn assign(&self, child: &std::process::Child) -> Result<(), String> {
            let process: HANDLE = child.as_raw_handle();
            // SAFETY: both handles are valid for the duration of the call.
            let assigned = unsafe { AssignProcessToJobObject(self.handle, process) };
            if assigned == 0 {
                return Err(format!(
                    "AssignProcessToJobObject failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and closed once; closing it ends the job's processes.
            unsafe { CloseHandle(self.handle) };
        }
    }

    fn class_value(class: PriorityClass) -> u32 {
        match class {
            PriorityClass::Idle => IDLE_PRIORITY_CLASS,
            PriorityClass::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            PriorityClass::Normal => NORMAL_PRIORITY_CLASS,
            PriorityClass::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        }
    }

    pub fn set_own_priority(class: PriorityClass) -> Result<(), String> {
        // SAFETY: the pseudo handle of the current process.
        let set = unsafe { SetPriorityClass(GetCurrentProcess(), class_value(class)) };
        if set == 0 {
            return Err(format!(
                "SetPriorityClass failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    pub fn own_priority() -> Result<PriorityClass, String> {
        // SAFETY: the pseudo handle of the current process.
        let value = unsafe { GetPriorityClass(GetCurrentProcess()) };
        if value == 0 {
            return Err(format!(
                "GetPriorityClass failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        [
            PriorityClass::Idle,
            PriorityClass::BelowNormal,
            PriorityClass::Normal,
            PriorityClass::AboveNormal,
        ]
        .into_iter()
        .find(|class| class_value(*class) == value)
        .ok_or_else(|| format!("GetPriorityClass returned the unlisted class {value:#x}"))
    }

    pub fn own_creation_unix_ms() -> Option<u64> {
        let empty = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (empty, empty, empty, empty);
        // SAFETY: the pseudo handle of the current process and four valid out pointers.
        let read = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        if read == 0 {
            return None;
        }
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        // 100 ns ticks since 1601-01-01 to Unix milliseconds.
        ticks
            .checked_sub(116_444_736_000_000_000)
            .map(|since_unix| since_unix / 10_000)
    }

    pub fn qpc_micros() -> Option<u64> {
        let mut counter = 0i64;
        let mut frequency = 0i64;
        // SAFETY: two valid out pointers.
        let ok = unsafe {
            QueryPerformanceCounter(&mut counter) != 0
                && QueryPerformanceFrequency(&mut frequency) != 0
        };
        if !ok || frequency <= 0 || counter < 0 {
            return None;
        }
        let micros = (counter as u128) * 1_000_000 / (frequency as u128);
        u64::try_from(micros).ok()
    }

    /// #402 C2b (plan §2.2, §5.5): the processes whose image name contains `VMMHeadless` and
    /// their handle counts, read with a process snapshot and `GetProcessHandleCount` (read-only
    /// queries; the processes are not otherwise touched).
    pub fn vmm_handle_counts() -> Result<Vec<super::VmmProcess>, String> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        use windows_sys::Win32::System::Threading::{
            GetProcessHandleCount, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: a snapshot of all processes; the handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return Err(format!(
                "CreateToolhelp32Snapshot failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: a plain-data structure; all-zero is its empty value.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Vec::new();
        // SAFETY: the snapshot handle and a sized entry.
        let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while more {
            let length = entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len());
            let image = String::from_utf16_lossy(&entry.szExeFile[..length]);
            if image.to_ascii_lowercase().contains("vmmheadless") {
                let pid = entry.th32ProcessID;
                // SAFETY: a query-only open of the listed pid; closed below.
                let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
                let handle_count = if process.is_null() {
                    None
                } else {
                    let mut count = 0u32;
                    // SAFETY: an open process handle and a valid out pointer.
                    let read = unsafe { GetProcessHandleCount(process, &mut count) } != 0;
                    // SAFETY: the handle opened above.
                    unsafe { CloseHandle(process) };
                    read.then_some(count)
                };
                found.push(super::VmmProcess {
                    pid,
                    parent_pid: entry.th32ParentProcessID,
                    image,
                    handle_count,
                });
            }
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        // SAFETY: the snapshot handle.
        unsafe { CloseHandle(snapshot) };
        Ok(found)
    }

    /// #402 C2b (plan §3.2 item 4, §5.1): the minimal viewer window: each frame is drawn with
    /// `SetDIBitsToDevice`, then `GdiFlush`; the time after it is `T_gdi_ret`, a submit-side
    /// proxy (`A_submit`), not presentation: GDI may batch, and DWM composes later.
    pub struct GdiWindow {
        hwnd: windows_sys::Win32::Foundation::HWND,
        bgra: Vec<u8>,
    }

    unsafe extern "system" fn window_proc(
        hwnd: windows_sys::Win32::Foundation::HWND,
        message: u32,
        wparam: windows_sys::Win32::Foundation::WPARAM,
        lparam: windows_sys::Win32::Foundation::LPARAM,
    ) -> windows_sys::Win32::Foundation::LRESULT {
        // SAFETY: the default handling of the window's own messages.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DefWindowProcW(
                hwnd, message, wparam, lparam,
            )
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    impl GdiWindow {
        pub fn create(width: u32, height: u32) -> Result<Self, String> {
            use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                CW_USEDEFAULT, CreateWindowExW, RegisterClassW, WNDCLASSW, WS_OVERLAPPEDWINDOW,
                WS_VISIBLE,
            };
            let class = wide("ac-capture-bench-view");
            let title = wide("ac-capture-bench view (A_submit proxy)");
            // SAFETY: the module handle of this executable.
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            // SAFETY: a plain-data structure; all-zero is its empty value.
            let mut class_info: WNDCLASSW = unsafe { std::mem::zeroed() };
            class_info.lpfnWndProc = Some(window_proc);
            class_info.hInstance = instance;
            class_info.lpszClassName = class.as_ptr();
            // SAFETY: the class structure above; a second registration fails harmlessly.
            unsafe { RegisterClassW(&class_info) };
            // SAFETY: the class registered above and a plain overlapped window.
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    i32::try_from(width).unwrap_or(1280).saturating_add(16),
                    i32::try_from(height).unwrap_or(720).saturating_add(39),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                )
            };
            if hwnd.is_null() {
                return Err(format!(
                    "CreateWindowExW failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self {
                hwnd,
                bgra: Vec::new(),
            })
        }

        /// Pumps the window's messages, draws one frame (RGBA8 or RGB8) and flushes GDI.
        pub fn present(
            &mut self,
            pixels: &[u8],
            width: u32,
            height: u32,
            bytes_per_pixel: usize,
        ) -> Result<(), String> {
            use windows_sys::Win32::Graphics::Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GdiFlush, GetDC, ReleaseDC,
                SetDIBitsToDevice,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
            };
            // SAFETY: a plain-data structure; all-zero is its empty value.
            let mut message: MSG = unsafe { std::mem::zeroed() };
            // SAFETY: the window's own message queue.
            while unsafe { PeekMessageW(&mut message, self.hwnd, 0, 0, PM_REMOVE) } != 0 {
                // SAFETY: the message just taken.
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            let count = width as usize * height as usize;
            if pixels.len() < count * bytes_per_pixel || !matches!(bytes_per_pixel, 3 | 4) {
                return Err("the frame does not match its size".to_owned());
            }
            self.bgra.resize(count * 4, 0);
            for index in 0..count {
                let source = index * bytes_per_pixel;
                let target = index * 4;
                self.bgra[target] = pixels[source + 2];
                self.bgra[target + 1] = pixels[source + 1];
                self.bgra[target + 2] = pixels[source];
                self.bgra[target + 3] = 255;
            }
            // SAFETY: a plain-data structure; all-zero is its empty value.
            let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = i32::try_from(width).map_err(|_| "width".to_owned())?;
            // A negative height is a top-down bitmap.
            info.bmiHeader.biHeight = -i32::try_from(height).map_err(|_| "height".to_owned())?;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB;
            // SAFETY: the window's device context, released below.
            let dc = unsafe { GetDC(self.hwnd) };
            if dc.is_null() {
                return Err("GetDC failed".to_owned());
            }
            // SAFETY: the context, a top-down 32-bit bitmap of `width` × `height` in `bgra`.
            let lines = unsafe {
                SetDIBitsToDevice(
                    dc,
                    0,
                    0,
                    width,
                    height,
                    0,
                    0,
                    0,
                    height,
                    self.bgra.as_ptr().cast(),
                    &info,
                    DIB_RGB_COLORS,
                )
            };
            // SAFETY: flushes this thread's GDI batch.
            unsafe { GdiFlush() };
            // SAFETY: the context taken above.
            unsafe { ReleaseDC(self.hwnd, dc) };
            if lines == 0 {
                return Err("SetDIBitsToDevice drew nothing".to_owned());
            }
            Ok(())
        }
    }

    impl Drop for GdiWindow {
        fn drop(&mut self) {
            // SAFETY: the window this value created.
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::PriorityClass;

    pub struct Job;

    impl Job {
        pub fn create() -> Result<Self, String> {
            Err("job objects need Windows".to_owned())
        }

        pub fn assign(&self, _child: &std::process::Child) -> Result<(), String> {
            Err("job objects need Windows".to_owned())
        }
    }

    pub fn set_own_priority(_class: PriorityClass) -> Result<(), String> {
        Err("priority classes need Windows".to_owned())
    }

    pub fn own_priority() -> Result<PriorityClass, String> {
        Err("priority classes need Windows".to_owned())
    }

    pub fn own_creation_unix_ms() -> Option<u64> {
        None
    }

    pub fn qpc_micros() -> Option<u64> {
        None
    }

    pub fn vmm_handle_counts() -> Result<Vec<super::VmmProcess>, String> {
        Err("process snapshots need Windows".to_owned())
    }

    pub struct GdiWindow;

    impl GdiWindow {
        pub fn create(_width: u32, _height: u32) -> Result<Self, String> {
            Err("the GDI window needs Windows".to_owned())
        }

        pub fn present(
            &mut self,
            _pixels: &[u8],
            _width: u32,
            _height: u32,
            _bytes_per_pixel: usize,
        ) -> Result<(), String> {
            Err("the GDI window needs Windows".to_owned())
        }
    }
}

/// #402 C2b: one `VMMHeadless` process and its handle count (`None` when it could not be read).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VmmProcess {
    pub pid: u32,
    pub parent_pid: u32,
    pub image: String,
    pub handle_count: Option<u32>,
}

pub use imp::{
    GdiWindow, Job, own_creation_unix_ms, own_priority, qpc_micros, set_own_priority,
    vmm_handle_counts,
};
