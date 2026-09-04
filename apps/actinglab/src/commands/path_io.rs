use crate::{CliError, CliOutcome};
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) static JSON_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn absolute_lexical_path(path: &Path) -> CliOutcome<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|err| CliError::usage(format!("failed to resolve current dir: {err}")))?
            .join(path)
    };
    Ok(normalize_path_lexically(&absolute))
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn path_has_root_or_prefix(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
}

fn path_starts_with_case_aware(path: &Path, base: &Path) -> bool {
    #[cfg(windows)]
    {
        let path_components = windows_normalized_path_components(path);
        let base_components = windows_normalized_path_components(base);
        path_components.len() >= base_components.len()
            && path_components
                .iter()
                .zip(base_components.iter())
                .all(|(left, right)| left == right)
    }
    #[cfg(not(windows))]
    {
        path.starts_with(base)
    }
}

#[cfg(windows)]
fn windows_normalized_path_components(path: &Path) -> Vec<String> {
    let raw = path.to_string_lossy();
    let normalized = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.to_string()
    };
    Path::new(&normalized)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect()
}

fn canonicalize_required_base(
    base: &Path,
    reason: &str,
    blocked_by: &[&'static str],
) -> CliOutcome<PathBuf> {
    base.canonicalize().map(windows_long_path).map_err(|err| {
        CliError::safety_blocked(
            "path_escape",
            format!(
                "{reason}: allowed base {} cannot be canonicalized: {err}",
                base.display()
            ),
            blocked_by,
        )
    })
}

fn canonicalize_with_existing_parent(
    path: &Path,
    reason: &str,
    blocked_by: &[&'static str],
) -> CliOutcome<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::<OsString>::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(OsString::from) else {
            return Err(CliError::safety_blocked(
                "path_escape",
                format!(
                    "{reason}: path {} has no existing parent inside the allowed base",
                    path.display()
                ),
                blocked_by,
            ));
        };
        missing.push(name);
        if !existing.pop() {
            return Err(CliError::safety_blocked(
                "path_escape",
                format!(
                    "{reason}: path {} has no existing parent inside the allowed base",
                    path.display()
                ),
                blocked_by,
            ));
        }
    }
    let mut resolved = existing
        .canonicalize()
        .map(windows_long_path)
        .map_err(|err| {
            CliError::safety_blocked(
                "path_escape",
                format!(
                    "{reason}: existing parent {} cannot be canonicalized: {err}",
                    existing.display()
                ),
                blocked_by,
            )
        })?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(normalize_path_lexically(&resolved))
}

#[cfg(windows)]
fn windows_long_path(path: PathBuf) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    type Handle = *mut std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CloseHandle(hObject: Handle) -> i32;
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut std::ffi::c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: Handle,
        ) -> Handle;
        fn GetFinalPathNameByHandleW(
            hFile: Handle,
            lpszFilePath: *mut u16,
            cchFilePath: u32,
            dwFlags: u32,
        ) -> u32;
        fn GetLongPathNameW(
            lpszShortPath: *const u16,
            lpszLongPath: *mut u16,
            cchBuffer: u32,
        ) -> u32;
    }

    let input = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_SHARE_READ: u32 = 0x01;
    const FILE_SHARE_WRITE: u32 = 0x02;
    const FILE_SHARE_DELETE: u32 = 0x04;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const INVALID_HANDLE_VALUE: Handle = !0usize as Handle;

    // SAFETY: `input` is null-terminated; the handle is closed before this function returns.
    let handle = unsafe {
        CreateFileW(
            input.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle != INVALID_HANDLE_VALUE {
        let mut final_path = vec![0u16; 32_768];
        // SAFETY: `final_path` is a valid writable buffer and `handle` is a live file handle.
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, final_path.as_mut_ptr(), final_path.len() as u32, 0)
        };
        // SAFETY: `handle` was returned by `CreateFileW` above.
        let _ = unsafe { CloseHandle(handle) };
        if written > 0 && (written as usize) < final_path.len() {
            return OsString::from_wide(&final_path[..written as usize]).into();
        }
    }

    // Windows CI can expose temp paths with 8.3 short components; expand them
    // before safety prefix checks so canonicalization does not create a false escape.
    // SAFETY: `input` is null-terminated and the first call only queries the required buffer size.
    let required = unsafe { GetLongPathNameW(input.as_ptr(), std::ptr::null_mut(), 0) };
    if required == 0 {
        return path;
    }
    let mut buffer = vec![0u16; required as usize];
    // SAFETY: `buffer` has the size reported by Windows and remains valid for the call.
    let written = unsafe { GetLongPathNameW(input.as_ptr(), buffer.as_mut_ptr(), required) };
    if written == 0 || written >= required {
        return path;
    }
    OsString::from_wide(&buffer[..written as usize]).into()
}

#[cfg(not(windows))]
fn windows_long_path(path: PathBuf) -> PathBuf {
    path
}

pub(crate) fn ensure_path_within(
    base: &Path,
    candidate: &Path,
    reason: &str,
    blocked_by: &[&'static str],
) -> CliOutcome<PathBuf> {
    if !candidate.is_absolute() && path_has_root_or_prefix(candidate) {
        return Err(CliError::safety_blocked(
            "path_escape",
            format!(
                "{reason}: path {} uses a root or drive prefix outside the allowed base",
                candidate.display()
            ),
            blocked_by,
        ));
    }
    let lexical_base = absolute_lexical_path(base)?;
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        lexical_base.join(candidate)
    };
    let lexical_resolved = normalize_path_lexically(&joined);
    if !path_starts_with_case_aware(&lexical_resolved, &lexical_base) {
        return Err(CliError::safety_blocked(
            "path_escape",
            format!(
                "{reason}: path {} escapes allowed base {}",
                lexical_resolved.display(),
                lexical_base.display()
            ),
            blocked_by,
        ));
    }
    let canonical_base = canonicalize_required_base(&lexical_base, reason, blocked_by)?;
    let resolved = canonicalize_with_existing_parent(&lexical_resolved, reason, blocked_by)?;
    if !path_starts_with_case_aware(&resolved, &canonical_base) {
        return Err(CliError::safety_blocked(
            "path_escape",
            format!(
                "{reason}: path {} escapes allowed base {} after canonicalization",
                resolved.display(),
                canonical_base.display()
            ),
            blocked_by,
        ));
    }
    Ok(resolved)
}

pub(crate) fn read_json_file<T>(path: &Path) -> CliOutcome<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", path.display())))?;
    let value = serde_json::from_str(&text)
        .map_err(|err| CliError::usage(format!("failed to parse {}: {err}", path.display())))?;
    Ok(Some(value))
}

#[cfg(test)]
pub(crate) fn write_json_file<T>(path: &Path, value: &T) -> CliOutcome<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::usage(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| CliError::usage(format!("failed to serialize JSON: {err}")))?;
    fs::write(path, text)
        .map_err(|err| CliError::usage(format!("failed to write {}: {err}", path.display())))
}

pub(crate) fn write_json_file_atomic<T>(path: &Path, value: &T) -> CliOutcome<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::usage(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| CliError::usage(format!("failed to serialize JSON: {err}")))?;
    cleanup_current_process_json_tmp_files(path)?;
    let seq = JSON_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp-{}-{seq}", std::process::id()));
    let mut file = File::create(&tmp)
        .map_err(|err| CliError::usage(format!("failed to create {}: {err}", tmp.display())))?;
    file.write_all(text.as_bytes())
        .map_err(|err| CliError::usage(format!("failed to write {}: {err}", tmp.display())))?;
    file.sync_all()
        .map_err(|err| CliError::usage(format!("failed to sync {}: {err}", tmp.display())))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        CliError::usage(format!(
            "failed to publish {} from {}: {err}",
            path.display(),
            tmp.display()
        ))
    })
}

fn cleanup_current_process_json_tmp_files(path: &Path) -> CliOutcome<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if !parent.exists() {
        return Ok(());
    }
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{stem}.tmp-{}-", std::process::id());
    let entries = fs::read_dir(parent)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", parent.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            CliError::usage(format!("failed to inspect {}: {err}", parent.display()))
        })?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with(&prefix) {
            fs::remove_file(entry.path()).map_err(|err| {
                CliError::usage(format!(
                    "failed to remove stale temp file {}: {err}",
                    entry.path().display()
                ))
            })?;
        }
    }
    Ok(())
}
