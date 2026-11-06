// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-0c: `--probe-dlls <ort dir>`. Loads every DLL of the folder one at a time
//! with the `LoadLibraryExW` flags the engine's runtime-library closure uses
//! (`LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32`), each by its absolute path,
//! and unloads it again: first every DLL that is not ONNX Runtime's (the GPU vendor's libraries,
//! by name), then `onnxruntime_providers_shared.dll`, `onnxruntime.dll`,
//! `onnxruntime_providers_cuda.dll` and any other `onnxruntime*` DLL. For a DLL that fails it
//! records the Win32 `GetLastError` code and its system message, then walks the DLL's import
//! table: every imported module is loaded under the same search (a file of the folder by its
//! absolute path with the same flags, any other name with `LOAD_LIBRARY_SEARCH_SYSTEM32`, which
//! is what the folder-and-System32 search leaves for it), and the functions it does not export
//! are listed. That names the missing dependency or the missing export.
//!
//! Read-only: no ONNX Runtime API is called and no session is built; the probe writes JSON to
//! stdout and one line per DLL to stderr, and nothing else.

use crate::error::{BenchCode, BenchError};
use crate::pe::{self, ImportedFunction};
use crate::run::refuse_production_path;
use crate::safety::{SafetyLimits, SafetyMonitor};
use crate::win::{self, OsMemoryReader};
use actingcommand_vision_ffi::NativeLibraryLoadFailure;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;
use windows_sys::Win32::Foundation::{FreeLibrary, GetLastError, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

pub const PROBE_SCHEMA: &str = "actingcommand.vision-ep-bench.dll-probe/1";
/// The flags of the engine's runtime-library closure (`crates/vision-ffi/src/ffi.rs`).
pub const CLOSURE_LOAD_FLAGS: u32 = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
/// The most DLLs one folder may hold for the probe.
const MAX_DLLS: usize = 256;
/// The most missing functions listed per imported module.
const MAX_LISTED_MISSING: usize = 64;

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub schema: &'static str,
    pub folder: String,
    pub load_flags: u32,
    pub load_flags_hex: String,
    pub order: &'static str,
    pub process_id: u32,
    pub priority_class_read_back: u32,
    pub dlls: Vec<DllProbe>,
    /// The files that did not load.
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DllProbe {
    pub file: String,
    pub path: String,
    pub bytes: u64,
    pub file_version: Option<String>,
    pub loaded: bool,
    pub load_us: u64,
    /// The path the loaded module reports (`GetModuleFileNameW`).
    pub loaded_from: Option<String>,
    pub failure: Option<NativeLibraryLoadFailure>,
    /// For a DLL that failed: every module its import table names, and whether it resolves.
    pub imports: Option<Vec<ImportProbe>>,
    /// The import table could not be read.
    pub import_table_error: Option<String>,
    /// The imported modules that did not resolve, or resolve without a function the DLL imports.
    pub unresolved_imports: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportProbe {
    pub module: String,
    /// `folder`: a file of the probed folder, loaded by absolute path with the closure's flags;
    /// `system32`: any other name, loaded with `LOAD_LIBRARY_SEARCH_SYSTEM32` (System32, API
    /// sets and known DLLs).
    pub searched: &'static str,
    pub resolves: bool,
    pub resolved_path: Option<String>,
    pub failure: Option<NativeLibraryLoadFailure>,
    pub functions: usize,
    pub missing_function_count: usize,
    /// The first imported functions the resolved module does not export.
    pub missing_functions: Vec<String>,
}

/// Runs the probe of `folder`. The report, once the folder is listed, holds every DLL probed;
/// the result fails (after the last DLL) when any did not load, or when the probe could not run.
pub fn probe(folder: &Path) -> (Option<ProbeReport>, Result<(), BenchError>) {
    let mut report = None;
    let result = run_probe(folder, &mut report);
    (report, result)
}

fn run_probe(folder: &Path, slot: &mut Option<ProbeReport>) -> Result<(), BenchError> {
    let folder = std::path::absolute(folder).map_err(|error| {
        BenchError::new(
            BenchCode::CliInvalid,
            format!("--probe-dlls {}: {error}", folder.display()),
        )
    })?;
    refuse_production_path("--probe-dlls", &folder)?;
    let priority = win::set_below_normal_priority()?;
    let mut monitor = SafetyMonitor::new(SafetyLimits::default(), OsMemoryReader);
    monitor.preflight()?;
    let files = ordered_dlls(&folder)?;
    let names: Vec<String> = files
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    let report = slot.insert(ProbeReport {
        schema: PROBE_SCHEMA,
        folder: folder.display().to_string(),
        load_flags: CLOSURE_LOAD_FLAGS,
        load_flags_hex: format!("0x{CLOSURE_LOAD_FLAGS:08x}"),
        order: "every DLL that is not onnxruntime*.dll by name, then onnxruntime_providers_shared, onnxruntime, onnxruntime_providers_cuda, then any other onnxruntime*; one at a time, each unloaded before the next",
        process_id: win::current_process_id(),
        priority_class_read_back: priority,
        dlls: Vec::new(),
        failed: Vec::new(),
    });
    for (file, path) in &files {
        monitor.check()?;
        let bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        let started = Instant::now();
        let loaded = load(path, Some(CLOSURE_LOAD_FLAGS));
        let load_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let mut row = DllProbe {
            file: file.clone(),
            path: path.display().to_string(),
            bytes,
            file_version: win::file_version(path),
            loaded: false,
            load_us,
            loaded_from: None,
            failure: None,
            imports: None,
            import_table_error: None,
            unresolved_imports: Vec::new(),
        };
        match loaded {
            Ok(module) => {
                row.loaded = true;
                row.loaded_from = module_path(module);
                unload(module);
                eprintln!("ac-vision-ep-bench: probe {file}: loaded");
            }
            Err(failure) => {
                eprintln!("ac-vision-ep-bench: probe {file}: FAILED: {failure}");
                row.failure = Some(failure);
                match std::fs::read(path)
                    .map_err(|error| format!("{} cannot be read: {error}", path.display()))
                    .and_then(|bytes| pe::imports(&bytes))
                {
                    Ok(modules) => {
                        let imports: Vec<ImportProbe> = modules
                            .iter()
                            .map(|module| probe_import(&folder, &names, module))
                            .collect();
                        for import in &imports {
                            if !import.resolves || import.missing_function_count > 0 {
                                let why = match &import.failure {
                                    Some(failure) => failure.to_string(),
                                    None => format!(
                                        "{} imported functions missing: {}",
                                        import.missing_function_count,
                                        import.missing_functions.join(", ")
                                    ),
                                };
                                eprintln!(
                                    "ac-vision-ep-bench: probe {file}:   import {} ({}): {why}",
                                    import.module, import.searched
                                );
                                row.unresolved_imports.push(import.module.clone());
                            }
                        }
                        if row.unresolved_imports.is_empty() {
                            eprintln!(
                                "ac-vision-ep-bench: probe {file}:   every one of its {} imported modules resolves with every imported function",
                                imports.len()
                            );
                        }
                        row.imports = Some(imports);
                    }
                    Err(error) => {
                        eprintln!(
                            "ac-vision-ep-bench: probe {file}:   the import table cannot be read: {error}"
                        );
                        row.import_table_error = Some(error);
                    }
                }
                report.failed.push(file.clone());
            }
        }
        report.dlls.push(row);
    }
    if report.failed.is_empty() {
        Ok(())
    } else {
        Err(BenchError::new(
            BenchCode::RuntimeUnavailable,
            format!(
                "{} of {} DLLs in {} did not load with LoadLibraryExW flags 0x{CLOSURE_LOAD_FLAGS:08x}: {}",
                report.failed.len(),
                report.dlls.len(),
                report.folder,
                report.failed.join(", ")
            ),
        ))
    }
}

/// The folder's DLLs (file name, absolute path) in probe order.
fn ordered_dlls(folder: &Path) -> Result<Vec<(String, PathBuf)>, BenchError> {
    let unreadable = |error: std::io::Error| {
        BenchError::new(
            BenchCode::RuntimeUnavailable,
            format!(
                "--probe-dlls {} cannot be listed: {error}",
                folder.display()
            ),
        )
    };
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().map_err(unreadable)?.is_file()
            && name.to_ascii_lowercase().ends_with(".dll")
        {
            files.push((name, entry.path()));
        }
    }
    if files.is_empty() {
        return Err(BenchError::new(
            BenchCode::RuntimeUnavailable,
            format!("--probe-dlls {} holds no DLL", folder.display()),
        ));
    }
    if files.len() > MAX_DLLS {
        return Err(BenchError::new(
            BenchCode::RuntimeUnavailable,
            format!(
                "--probe-dlls {} holds {} DLLs, more than {MAX_DLLS}",
                folder.display(),
                files.len()
            ),
        ));
    }
    files.sort_by_key(|(name, _)| (probe_rank(name), name.to_ascii_lowercase()));
    Ok(files)
}

fn probe_rank(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "onnxruntime_providers_shared.dll" => 1,
        "onnxruntime.dll" => 2,
        "onnxruntime_providers_cuda.dll" => 3,
        other if other.starts_with("onnxruntime") => 4,
        _ => 0,
    }
}

fn probe_import(folder: &Path, names: &[String], module: &pe::ImportedModule) -> ImportProbe {
    let in_folder = names.contains(&module.name.to_ascii_lowercase());
    let (searched, loaded) = if in_folder {
        (
            "folder",
            load(&folder.join(&module.name), Some(CLOSURE_LOAD_FLAGS)),
        )
    } else {
        (
            "system32",
            load(Path::new(&module.name), Some(LOAD_LIBRARY_SEARCH_SYSTEM32)),
        )
    };
    let mut row = ImportProbe {
        module: module.name.clone(),
        searched,
        resolves: false,
        resolved_path: None,
        failure: None,
        functions: module.functions.len(),
        missing_function_count: 0,
        missing_functions: Vec::new(),
    };
    match loaded {
        Ok(handle) => {
            row.resolves = true;
            row.resolved_path = module_path(handle);
            for function in &module.functions {
                if !exports(handle, function) {
                    row.missing_function_count += 1;
                    if row.missing_functions.len() < MAX_LISTED_MISSING {
                        row.missing_functions.push(function.text());
                    }
                }
            }
            unload(handle);
        }
        Err(failure) => row.failure = Some(failure),
    }
    row
}

/// `LoadLibraryExW(path, NULL, flags)`; the Win32 code is read at once.
fn load(path: &Path, flags: Option<u32>) -> Result<HMODULE, NativeLibraryLoadFailure> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: a NUL-terminated path; the module is unloaded by the caller.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), flags.unwrap_or(0)) };
    if module.is_null() {
        // SAFETY: reads this thread's last-error value right after the failed call.
        let code = unsafe { GetLastError() };
        let error = std::io::Error::from_raw_os_error(code as i32);
        return Err(NativeLibraryLoadFailure::from_os_error(
            path.display().to_string(),
            flags,
            &error,
        ));
    }
    Ok(module)
}

fn unload(module: HMODULE) {
    // SAFETY: a module this probe loaded once; nothing of it is used afterwards.
    if unsafe { FreeLibrary(module) } == 0 {
        // SAFETY: reads this thread's last-error value right after the failed call.
        let code = unsafe { GetLastError() };
        eprintln!("ac-vision-ep-bench: probe: FreeLibrary failed with Win32 error {code}");
    }
}

fn module_path(module: HMODULE) -> Option<String> {
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: a loaded module and a buffer of the stated length.
    let length = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
    (length != 0).then(|| String::from_utf16_lossy(&buffer[..length as usize]))
}

fn exports(module: HMODULE, function: &ImportedFunction) -> bool {
    match function {
        ImportedFunction::Name(name) => {
            let mut bytes = name.as_bytes().to_vec();
            bytes.push(0);
            // SAFETY: a loaded module and a NUL-terminated name.
            unsafe { GetProcAddress(module, bytes.as_ptr()) }.is_some()
        }
        // SAFETY: a loaded module; an ordinal in the low word with a zero high word
        // (MAKEINTRESOURCEA) is GetProcAddress's documented by-ordinal form.
        ImportedFunction::Ordinal(ordinal) => unsafe {
            GetProcAddress(
                module,
                std::ptr::without_provenance::<u8>(usize::from(*ordinal)),
            )
        }
        .is_some(),
    }
}
