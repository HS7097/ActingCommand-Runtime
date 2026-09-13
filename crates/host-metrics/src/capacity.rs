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
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFinalPathNameByHandleW, VOLUME_NAME_GUID,
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
    // Query attributes with the same sharing and directory/reparse behavior as
    // Windows canonicalize. The File owns this call's temporary handle.
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(existing)
        .map_err(|error| CapacityUnavailable {
            operation: "canonicalize_capacity_target",
            raw_os_error: error.raw_os_error(),
            detail: error.to_string(),
        })?;
    // Cover the extended-path limit and a GUID-root prefix, including the NUL.
    // A required size beyond this bound fails without another path query.
    let mut resolved = vec![0u16; 32_768 + 49];
    // SAFETY: File keeps the handle live; the output buffer is sized in WCHARs.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            resolved.as_mut_ptr(),
            resolved.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_GUID,
        )
    } as usize;
    if length == 0 {
        return Err(native_error("identify_capacity_volume"));
    }
    if length >= resolved.len() {
        return Err(CapacityUnavailable {
            operation: "identify_capacity_volume",
            raw_os_error: None,
            detail: "final volume path exceeds capacity resolution bound".into(),
        });
    }
    // Only the 49-WCHAR GUID root is an identity. The private path suffix may
    // contain non-Unicode Windows names and is neither decoded nor retained.
    let root = resolved[..length]
        .get(..49)
        .ok_or_else(|| CapacityUnavailable {
            operation: "identify_capacity_volume",
            raw_os_error: None,
            detail: "missing final volume GUID root".into(),
        })?;
    let mut identity = String::from_utf16(root).map_err(|error| CapacityUnavailable {
        operation: "identify_capacity_volume",
        raw_os_error: None,
        detail: error.to_string(),
    })?;
    identity.make_ascii_lowercase();
    let valid = identity
        .strip_prefix(r"\\?\volume{")
        .and_then(|guid| guid.strip_suffix(r"}\"))
        .is_some_and(|guid| {
            guid.len() == 36
                && guid.bytes().enumerate().all(|(index, byte)| match index {
                    8 | 13 | 18 | 23 => byte == b'-',
                    _ => byte.is_ascii_hexdigit(),
                })
        });
    if !valid {
        return Err(CapacityUnavailable {
            operation: "identify_capacity_volume",
            raw_os_error: None,
            detail: "invalid final volume GUID root".into(),
        });
    }
    Ok(identity)
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
