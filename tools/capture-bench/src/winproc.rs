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
}

pub use imp::{Job, own_creation_unix_ms, own_priority, qpc_micros, set_own_priority};
