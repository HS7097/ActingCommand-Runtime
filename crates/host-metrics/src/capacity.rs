// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityTarget {
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityUnavailable {
    pub operation: &'static str,
    pub raw_os_error: Option<i32>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacitySample {
    /// Indices into the caller's bounded target list; no private paths enter the fact.
    pub targets: Vec<usize>,
    pub volume_id: Option<String>,
    pub available_bytes: Result<u64, CapacityUnavailable>,
}

/// One resolve per target and at most one capacity query per distinct volume, without retries.
pub fn sample_capacity(targets: &[CapacityTarget]) -> Vec<CapacitySample> {
    let mut samples = Vec::<CapacitySample>::new();
    let mut volumes = BTreeMap::<String, usize>::new();
    for (index, target) in targets.iter().enumerate() {
        match volume(&target.path) {
            Ok(volume_id) => {
                if let Some(position) = volumes.get(&volume_id) {
                    samples[*position].targets.push(index);
                } else {
                    let available_bytes = available(&volume_id);
                    volumes.insert(volume_id.clone(), samples.len());
                    samples.push(CapacitySample {
                        targets: vec![index],
                        volume_id: Some(volume_id),
                        available_bytes,
                    });
                }
            }
            Err(error) => samples.push(CapacitySample {
                targets: vec![index],
                volume_id: None,
                available_bytes: Err(error),
            }),
        }
    }
    samples
}

#[cfg(windows)]
fn native_error(operation: &'static str) -> CapacityUnavailable {
    let error = std::io::Error::last_os_error();
    CapacityUnavailable {
        operation,
        raw_os_error: error.raw_os_error(),
        detail: error.to_string(),
    }
}

#[cfg(windows)]
pub(crate) fn volume(path: &Path) -> Result<String, CapacityUnavailable> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };

    // Resolve the nearest existing ancestor for a future shard/temp file. A dangling
    // reparse point is an error: it must not silently inherit its parent's volume.
    let mut existing = path;
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or_else(|| CapacityUnavailable {
                    operation: "resolve_capacity_target",
                    raw_os_error: error.raw_os_error(),
                    detail: error.to_string(),
                })?;
            }
            Err(error) => {
                return Err(CapacityUnavailable {
                    operation: "resolve_capacity_target",
                    raw_os_error: error.raw_os_error(),
                    detail: error.to_string(),
                });
            }
        }
    }
    let resolved = existing
        .canonicalize()
        .map_err(|error| CapacityUnavailable {
            operation: "canonicalize_capacity_target",
            raw_os_error: error.raw_os_error(),
            detail: error.to_string(),
        })?;
    let path = resolved
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut mount = vec![0u16; 32_768];
    // SAFETY: both buffers are valid; input is terminated and the output size is exact.
    if unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return Err(native_error("resolve_capacity_volume"));
    }
    let mut identity = [0u16; 64];
    // SAFETY: GetVolumePathNameW supplied a terminated mount path; output is sized in WCHARs.
    if unsafe {
        GetVolumeNameForVolumeMountPointW(
            mount.as_ptr(),
            identity.as_mut_ptr(),
            identity.len() as u32,
        )
    } == 0
    {
        return Err(native_error("identify_capacity_volume"));
    }
    let end = identity
        .iter()
        .position(|value| *value == 0)
        .ok_or_else(|| CapacityUnavailable {
            operation: "identify_capacity_volume",
            raw_os_error: None,
            detail: "unterminated volume identity".into(),
        })?;
    String::from_utf16(&identity[..end])
        .map(|value| value.to_ascii_lowercase())
        .map_err(|error| CapacityUnavailable {
            operation: "identify_capacity_volume",
            raw_os_error: None,
            detail: error.to_string(),
        })
}

#[cfg(windows)]
fn available(volume: &str) -> Result<u64, CapacityUnavailable> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let volume = volume.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut bytes = 0u64;
    // SAFETY: the GUID path is terminated and the output pointer is live. This is
    // the space available to this caller, including any OS quota restrictions.
    if unsafe {
        GetDiskFreeSpaceExW(
            volume.as_ptr(),
            &mut bytes,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(native_error("sample_capacity"))
    } else {
        Ok(bytes)
    }
}

#[cfg(not(windows))]
pub(crate) fn volume(_: &Path) -> Result<String, CapacityUnavailable> {
    Err(CapacityUnavailable {
        operation: "identify_capacity_volume",
        raw_os_error: None,
        detail: "capacity sampling is unsupported on this platform".into(),
    })
}

#[cfg(not(windows))]
fn available(_: &str) -> Result<u64, CapacityUnavailable> {
    Err(CapacityUnavailable {
        operation: "sample_capacity",
        raw_os_error: None,
        detail: "capacity sampling is unsupported on this platform".into(),
    })
}
