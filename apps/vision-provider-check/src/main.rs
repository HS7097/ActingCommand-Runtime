// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_vision_ffi::{
    FastDeployPpocrArtifacts, FastDeployPpocrBackend, NnEngine, NnInferenceRequest, OcrEngine,
    OcrInferenceRequest, OnnxRuntimeArtifacts, OnnxRuntimeBackend, VisionFfiError, VisionFfiResult,
    VisionFrame, VisionPixelFormat, VisionProviderArtifactManifest, VisionRect,
    validate_fastdeploy_ppocr_provider_abi, validate_onnxruntime_provider_abi,
};
use image::ImageFormat;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckOptions {
    manifest: PathBuf,
    backend: BackendSelection,
    require_existing: bool,
    mode: CheckMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CheckMode {
    Manifest,
    OcrSmoke {
        frame: PathBuf,
        region: Option<VisionRect>,
    },
    NnSmoke {
        frame: PathBuf,
        model_id: Option<String>,
    },
    ArtifactLock {
        out: Option<PathBuf>,
    },
    AbiCheck,
    ExportAudit {
        library: PathBuf,
        expectation: ExportExpectation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendSelection {
    All,
    FastDeployPpocr,
    OnnxRuntime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportExpectation {
    None,
    FastDeployPpocrProvider,
    OnnxRuntimeProvider,
}

impl ExportExpectation {
    fn required_symbols(self) -> &'static [&'static str] {
        match self {
            ExportExpectation::None => &[],
            ExportExpectation::FastDeployPpocrProvider => &[
                "ac_fastdeploy_ppocr_read_text_json",
                "ac_vision_free_buffer",
            ],
            ExportExpectation::OnnxRuntimeProvider => {
                &["ac_onnxruntime_classify_json", "ac_vision_free_buffer"]
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct CheckReport {
    ok: bool,
    schema_version: String,
    backend: &'static str,
    require_existing: bool,
    backends: Vec<BackendReport>,
}

#[derive(Debug, Serialize)]
struct BackendReport {
    id: &'static str,
    configured: bool,
    provider_library_path: Option<String>,
    required_paths: Vec<String>,
    execution_provider: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct InferenceSmokeReport<T> {
    ok: bool,
    backend: &'static str,
    frame: FrameReport,
    result: T,
}

#[derive(Debug, Serialize)]
struct FrameReport {
    path: String,
    width: u32,
    height: u32,
    pixel_format: &'static str,
}

#[derive(Debug, Serialize)]
struct ArtifactLockReport {
    ok: bool,
    schema_version: String,
    backend: &'static str,
    total_size_bytes: u64,
    artifacts: Vec<ArtifactLockEntry>,
}

#[derive(Debug, Serialize)]
struct ArtifactLockEntry {
    backend: &'static str,
    role: &'static str,
    path: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct AbiCheckReport {
    ok: bool,
    schema_version: String,
    backend: &'static str,
    backends: Vec<ProviderAbiReport>,
}

#[derive(Debug, Serialize)]
struct ExportAuditReport {
    ok: bool,
    library_path: String,
    export_count: usize,
    expected_symbols: Vec<&'static str>,
    present_symbols: Vec<&'static str>,
    missing_symbols: Vec<&'static str>,
    msvc_cxx_symbol_count: usize,
    sample_exports: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProviderAbiReport {
    id: &'static str,
    provider_library_path: String,
    required_symbols: Vec<&'static str>,
}

fn main() {
    if let Err(err) = run(env::args().skip(1)) {
        eprintln!("FATAL: {err}");
        std::process::exit(1);
    }
}

fn run<I>(args: I) -> VisionFfiResult<()>
where
    I: IntoIterator<Item = String>,
{
    let options = parse_args(args)?;
    let json = match &options.mode {
        CheckMode::Manifest => serde_json::to_string_pretty(&build_report(&options)?),
        CheckMode::OcrSmoke { .. } => serde_json::to_string_pretty(&run_ocr_smoke(&options)?),
        CheckMode::NnSmoke { .. } => serde_json::to_string_pretty(&run_nn_smoke(&options)?),
        CheckMode::ArtifactLock { .. } => {
            serde_json::to_string_pretty(&run_artifact_lock(&options)?)
        }
        CheckMode::AbiCheck => serde_json::to_string_pretty(&run_abi_check(&options)?),
        CheckMode::ExportAudit { .. } => serde_json::to_string_pretty(&run_export_audit(&options)?),
    }
    .map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("failed to serialize provider check report: {err}"),
        )
    })?;
    println!("{json}");
    Ok(())
}

fn parse_args<I>(args: I) -> VisionFfiResult<CheckOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut manifest = None;
    let mut backend = BackendSelection::All;
    let mut require_existing = false;
    let mut ocr_frame = None;
    let mut ocr_region = None;
    let mut nn_frame = None;
    let mut nn_model_id = None;
    let mut artifact_lock = false;
    let mut abi_check = false;
    let mut export_audit = None;
    let mut export_expectation = ExportExpectation::None;
    let mut backend_set = false;
    let mut lock_out = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal(
                        "vision-provider-check",
                        "--manifest requires a file path",
                    )
                })?;
                manifest = Some(PathBuf::from(value));
            }
            "--backend" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal(
                        "vision-provider-check",
                        "--backend requires all, fastdeploy_ppocr, or onnxruntime",
                    )
                })?;
                backend = parse_backend(&value)?;
                backend_set = true;
            }
            "--require-existing" => require_existing = true,
            "--ocr-frame" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal(
                        "vision-provider-check",
                        "--ocr-frame requires a PNG path",
                    )
                })?;
                ocr_frame = Some(PathBuf::from(value));
            }
            "--ocr-region" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal(
                        "vision-provider-check",
                        "--ocr-region requires x,y,width,height",
                    )
                })?;
                ocr_region = Some(parse_region(&value)?);
            }
            "--nn-frame" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal("vision-provider-check", "--nn-frame requires a PNG path")
                })?;
                nn_frame = Some(PathBuf::from(value));
            }
            "--nn-model-id" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal(
                        "vision-provider-check",
                        "--nn-model-id requires a non-empty value",
                    )
                })?;
                if value.trim().is_empty() {
                    return Err(VisionFfiError::fatal(
                        "vision-provider-check",
                        "--nn-model-id must be non-empty",
                    ));
                }
                nn_model_id = Some(value);
            }
            "--artifact-lock" => artifact_lock = true,
            "--abi-check" => abi_check = true,
            "--export-audit" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal(
                        "vision-provider-check",
                        "--export-audit requires a DLL path",
                    )
                })?;
                export_audit = Some(PathBuf::from(value));
            }
            "--expect" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal(
                        "vision-provider-check",
                        "--expect requires none, fastdeploy_ppocr_provider, or onnxruntime_provider",
                    )
                })?;
                export_expectation = parse_export_expectation(&value)?;
            }
            "--lock-out" => {
                let value = args.next().ok_or_else(|| {
                    VisionFfiError::fatal(
                        "vision-provider-check",
                        "--lock-out requires a JSON output path",
                    )
                })?;
                artifact_lock = true;
                lock_out = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                return Err(VisionFfiError::fatal("vision-provider-check", usage()));
            }
            _ => {
                return Err(VisionFfiError::fatal(
                    "vision-provider-check",
                    format!("unknown argument: {arg}\n{}", usage()),
                ));
            }
        }
    }

    if ocr_frame.is_some() && nn_frame.is_some() {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--ocr-frame and --nn-frame cannot be used in the same invocation",
        ));
    }
    if artifact_lock && (ocr_frame.is_some() || nn_frame.is_some()) {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--artifact-lock cannot be used with --ocr-frame or --nn-frame",
        ));
    }
    if abi_check && (artifact_lock || ocr_frame.is_some() || nn_frame.is_some()) {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--abi-check cannot be used with --artifact-lock, --ocr-frame, or --nn-frame",
        ));
    }
    if export_audit.is_some()
        && (artifact_lock || abi_check || ocr_frame.is_some() || nn_frame.is_some())
    {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--export-audit cannot be used with --artifact-lock, --abi-check, --ocr-frame, or --nn-frame",
        ));
    }
    if export_audit.is_some() && (backend_set || require_existing || lock_out.is_some()) {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--export-audit cannot be used with --backend, --require-existing, or --lock-out",
        ));
    }
    if export_audit.is_none() && export_expectation != ExportExpectation::None {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--expect requires --export-audit",
        ));
    }
    if ocr_region.is_some() && ocr_frame.is_none() {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--ocr-region requires --ocr-frame",
        ));
    }
    if nn_model_id.is_some() && nn_frame.is_none() {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--nn-model-id requires --nn-frame",
        ));
    }
    if ocr_frame.is_some() && backend == BackendSelection::OnnxRuntime {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--ocr-frame cannot be used with --backend onnxruntime",
        ));
    }
    if nn_frame.is_some() && backend == BackendSelection::FastDeployPpocr {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--nn-frame cannot be used with --backend fastdeploy_ppocr",
        ));
    }

    let mode = match (export_audit, abi_check, artifact_lock, ocr_frame, nn_frame) {
        (Some(library), false, false, None, None) => CheckMode::ExportAudit {
            library,
            expectation: export_expectation,
        },
        (None, true, false, None, None) => CheckMode::AbiCheck,
        (None, false, true, None, None) => CheckMode::ArtifactLock { out: lock_out },
        (None, false, false, Some(frame), None) => CheckMode::OcrSmoke {
            frame,
            region: ocr_region,
        },
        (None, false, false, None, Some(frame)) => CheckMode::NnSmoke {
            frame,
            model_id: nn_model_id,
        },
        (None, false, false, None, None) => CheckMode::Manifest,
        _ => unreachable!("checked above"),
    };
    let manifest = if matches!(mode, CheckMode::ExportAudit { .. }) {
        manifest.unwrap_or_default()
    } else {
        manifest.ok_or_else(|| {
            VisionFfiError::fatal(
                "vision-provider-check",
                format!("--manifest is required\n{}", usage()),
            )
        })?
    };

    Ok(CheckOptions {
        manifest,
        backend,
        require_existing,
        mode,
    })
}

fn parse_export_expectation(value: &str) -> VisionFfiResult<ExportExpectation> {
    match value {
        "none" => Ok(ExportExpectation::None),
        "fastdeploy_ppocr_provider" => Ok(ExportExpectation::FastDeployPpocrProvider),
        "onnxruntime_provider" => Ok(ExportExpectation::OnnxRuntimeProvider),
        _ => Err(VisionFfiError::fatal(
            "vision-provider-check",
            format!("unsupported export expectation: {value}"),
        )),
    }
}

fn parse_backend(value: &str) -> VisionFfiResult<BackendSelection> {
    match value {
        "all" => Ok(BackendSelection::All),
        "fastdeploy_ppocr" => Ok(BackendSelection::FastDeployPpocr),
        "onnxruntime" => Ok(BackendSelection::OnnxRuntime),
        _ => Err(VisionFfiError::fatal(
            "vision-provider-check",
            format!("unsupported backend: {value}"),
        )),
    }
}

fn parse_region(value: &str) -> VisionFfiResult<VisionRect> {
    let parts: Vec<_> = value.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "--ocr-region must use x,y,width,height",
        ));
    }
    let parse_i32 = |part: &str, label: &str| {
        part.parse::<i32>().map_err(|err| {
            VisionFfiError::fatal(
                "vision-provider-check",
                format!("failed to parse {label} in --ocr-region: {err}"),
            )
        })
    };
    Ok(VisionRect {
        x: parse_i32(parts[0], "x")?,
        y: parse_i32(parts[1], "y")?,
        width: parse_i32(parts[2], "width")?,
        height: parse_i32(parts[3], "height")?,
    })
}

fn build_report(options: &CheckOptions) -> VisionFfiResult<CheckReport> {
    let manifest = VisionProviderArtifactManifest::load_json_file(&options.manifest)?;
    let mut backends = Vec::new();

    if matches!(
        options.backend,
        BackendSelection::All | BackendSelection::FastDeployPpocr
    ) {
        let artifacts = manifest.require_fastdeploy_ppocr()?;
        if options.require_existing {
            artifacts.validate_existing_files()?;
        }
        backends.push(fastdeploy_report(artifacts));
    }

    if matches!(
        options.backend,
        BackendSelection::All | BackendSelection::OnnxRuntime
    ) {
        let artifacts = manifest.require_onnxruntime()?;
        if options.require_existing {
            artifacts.validate_existing_files()?;
        }
        backends.push(onnxruntime_report(artifacts));
    }

    Ok(CheckReport {
        ok: true,
        schema_version: manifest.schema_version,
        backend: options.backend.as_str(),
        require_existing: options.require_existing,
        backends,
    })
}

fn run_ocr_smoke(
    options: &CheckOptions,
) -> VisionFfiResult<InferenceSmokeReport<serde_json::Value>> {
    let CheckMode::OcrSmoke { frame, region } = &options.mode else {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "OCR smoke was called without --ocr-frame",
        ));
    };
    let manifest = VisionProviderArtifactManifest::load_json_file(&options.manifest)?;
    let artifacts = manifest.require_fastdeploy_ppocr()?.clone();
    let mut backend = FastDeployPpocrBackend::from_artifacts(artifacts.clone())?;
    let (frame_report, vision_frame) = load_png_frame(frame)?;
    let region = match region {
        Some(region) => *region,
        None => VisionRect::full_frame(&vision_frame)?,
    };
    let result = backend.read_text(OcrInferenceRequest {
        frame: vision_frame,
        region,
        languages: artifacts.supported_languages,
        timeout_ms: artifacts.default_timeout_ms,
    })?;
    let result = serde_json::to_value(result).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("failed to serialize OCR smoke result: {err}"),
        )
    })?;
    Ok(InferenceSmokeReport {
        ok: true,
        backend: "fastdeploy_ppocr",
        frame: frame_report,
        result,
    })
}

fn run_nn_smoke(
    options: &CheckOptions,
) -> VisionFfiResult<InferenceSmokeReport<serde_json::Value>> {
    let CheckMode::NnSmoke { frame, model_id } = &options.mode else {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "NN smoke was called without --nn-frame",
        ));
    };
    let manifest = VisionProviderArtifactManifest::load_json_file(&options.manifest)?;
    let artifacts = manifest.require_onnxruntime()?.clone();
    let mut backend = OnnxRuntimeBackend::from_artifacts(artifacts.clone())?;
    let (frame_report, vision_frame) = load_png_frame(frame)?;
    let result = backend.classify(NnInferenceRequest {
        frame: vision_frame,
        model_id: model_id
            .clone()
            .unwrap_or_else(|| path_string(&artifacts.model_path)),
        labels: artifacts.labels,
        timeout_ms: artifacts.default_timeout_ms,
    })?;
    let result = serde_json::to_value(result).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("failed to serialize NN smoke result: {err}"),
        )
    })?;
    Ok(InferenceSmokeReport {
        ok: true,
        backend: "onnxruntime",
        frame: frame_report,
        result,
    })
}

fn run_artifact_lock(options: &CheckOptions) -> VisionFfiResult<ArtifactLockReport> {
    let CheckMode::ArtifactLock { out } = &options.mode else {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "artifact lock was called without --artifact-lock",
        ));
    };
    let manifest = VisionProviderArtifactManifest::load_json_file(&options.manifest)?;
    let mut artifacts = Vec::new();

    if matches!(
        options.backend,
        BackendSelection::All | BackendSelection::FastDeployPpocr
    ) {
        collect_fastdeploy_artifacts(manifest.require_fastdeploy_ppocr()?, &mut artifacts)?;
    }

    if matches!(
        options.backend,
        BackendSelection::All | BackendSelection::OnnxRuntime
    ) {
        collect_onnxruntime_artifacts(manifest.require_onnxruntime()?, &mut artifacts)?;
    }

    let total_size_bytes = artifacts.iter().map(|entry| entry.size_bytes).sum();
    let report = ArtifactLockReport {
        ok: true,
        schema_version: manifest.schema_version,
        backend: options.backend.as_str(),
        total_size_bytes,
        artifacts,
    };
    if let Some(path) = out {
        write_json_file(path, &report)?;
    }
    Ok(report)
}

fn run_abi_check(options: &CheckOptions) -> VisionFfiResult<AbiCheckReport> {
    if options.mode != CheckMode::AbiCheck {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "ABI check was called without --abi-check",
        ));
    }
    let manifest = VisionProviderArtifactManifest::load_json_file(&options.manifest)?;
    let mut backends = Vec::new();

    if matches!(
        options.backend,
        BackendSelection::All | BackendSelection::FastDeployPpocr
    ) {
        let artifacts = manifest.require_fastdeploy_ppocr()?;
        artifacts.validate_existing_files()?;
        validate_fastdeploy_ppocr_provider_abi(&artifacts.provider_library_path)?;
        backends.push(ProviderAbiReport {
            id: "fastdeploy_ppocr",
            provider_library_path: path_string(&artifacts.provider_library_path),
            required_symbols: vec![
                "ac_fastdeploy_ppocr_read_text_json",
                "ac_vision_free_buffer",
            ],
        });
    }

    if matches!(
        options.backend,
        BackendSelection::All | BackendSelection::OnnxRuntime
    ) {
        let artifacts = manifest.require_onnxruntime()?;
        artifacts.validate_existing_files()?;
        validate_onnxruntime_provider_abi(&artifacts.provider_library_path)?;
        backends.push(ProviderAbiReport {
            id: "onnxruntime",
            provider_library_path: path_string(&artifacts.provider_library_path),
            required_symbols: vec!["ac_onnxruntime_classify_json", "ac_vision_free_buffer"],
        });
    }

    Ok(AbiCheckReport {
        ok: true,
        schema_version: manifest.schema_version,
        backend: options.backend.as_str(),
        backends,
    })
}

fn run_export_audit(options: &CheckOptions) -> VisionFfiResult<ExportAuditReport> {
    let CheckMode::ExportAudit {
        library,
        expectation,
    } = &options.mode
    else {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "export audit was called without --export-audit",
        ));
    };
    let bytes = fs::read(library).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!(
                "failed to read export audit library {}: {err}",
                library.display()
            ),
        )
    })?;
    let exports = parse_pe_exports(&bytes)?;
    let expected_symbols = expectation.required_symbols().to_vec();
    let present_symbols: Vec<_> = expected_symbols
        .iter()
        .copied()
        .filter(|symbol| exports.iter().any(|export| export == symbol))
        .collect();
    let missing_symbols: Vec<_> = expected_symbols
        .iter()
        .copied()
        .filter(|symbol| !exports.iter().any(|export| export == symbol))
        .collect();
    let msvc_cxx_symbol_count = exports
        .iter()
        .filter(|export| export.starts_with('?') || export.starts_with("??"))
        .count();
    let sample_exports = exports.iter().take(80).cloned().collect();

    Ok(ExportAuditReport {
        ok: missing_symbols.is_empty(),
        library_path: path_string(library),
        export_count: exports.len(),
        expected_symbols,
        present_symbols,
        missing_symbols,
        msvc_cxx_symbol_count,
        sample_exports,
    })
}

fn parse_pe_exports(bytes: &[u8]) -> VisionFfiResult<Vec<String>> {
    if bytes.len() < 0x40 {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "PE export audit input is too small",
        ));
    }
    if &bytes[0..2] != b"MZ" {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "PE export audit input is not an MZ executable",
        ));
    }
    let pe_offset = read_u32(bytes, 0x3c)? as usize;
    require_range(bytes, pe_offset, 24, "PE header")?;
    if &bytes[pe_offset..pe_offset + 4] != b"PE\0\0" {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "PE export audit input is missing PE signature",
        ));
    }
    let coff_offset = pe_offset + 4;
    let section_count = read_u16(bytes, coff_offset + 2)? as usize;
    let optional_header_size = read_u16(bytes, coff_offset + 16)? as usize;
    let optional_offset = coff_offset + 20;
    require_range(
        bytes,
        optional_offset,
        optional_header_size,
        "PE optional header",
    )?;
    let magic = read_u16(bytes, optional_offset)?;
    let data_directory_offset = match magic {
        0x10b => optional_offset + 96,
        0x20b => optional_offset + 112,
        _ => {
            return Err(VisionFfiError::fatal(
                "vision-provider-check",
                format!("unsupported PE optional header magic: 0x{magic:04x}"),
            ));
        }
    };
    require_range(bytes, data_directory_offset, 8, "PE export data directory")?;
    let export_rva = read_u32(bytes, data_directory_offset)?;
    if export_rva == 0 {
        return Ok(Vec::new());
    }
    let section_table_offset = optional_offset + optional_header_size;
    require_range(
        bytes,
        section_table_offset,
        section_count.saturating_mul(40),
        "PE section table",
    )?;
    let mut sections = Vec::with_capacity(section_count);
    for index in 0..section_count {
        let section_offset = section_table_offset + index * 40;
        let virtual_size = read_u32(bytes, section_offset + 8)?;
        let virtual_address = read_u32(bytes, section_offset + 12)?;
        let raw_size = read_u32(bytes, section_offset + 16)?;
        let raw_pointer = read_u32(bytes, section_offset + 20)?;
        sections.push(PeSection {
            virtual_address,
            size: virtual_size.max(raw_size),
            raw_pointer,
        });
    }

    let export_offset = rva_to_offset(export_rva, &sections)?;
    require_range(bytes, export_offset, 40, "PE export directory")?;
    let name_count = read_u32(bytes, export_offset + 24)? as usize;
    let names_rva = read_u32(bytes, export_offset + 32)?;
    if name_count == 0 {
        return Ok(Vec::new());
    }
    let names_offset = rva_to_offset(names_rva, &sections)?;
    require_range(
        bytes,
        names_offset,
        name_count.saturating_mul(4),
        "PE export name table",
    )?;

    let mut exports = Vec::with_capacity(name_count);
    for index in 0..name_count {
        let name_rva = read_u32(bytes, names_offset + index * 4)?;
        let name_offset = rva_to_offset(name_rva, &sections)?;
        exports.push(read_c_string(bytes, name_offset)?);
    }
    exports.sort();
    Ok(exports)
}

#[derive(Debug, Clone, Copy)]
struct PeSection {
    virtual_address: u32,
    size: u32,
    raw_pointer: u32,
}

fn rva_to_offset(rva: u32, sections: &[PeSection]) -> VisionFfiResult<usize> {
    for section in sections {
        let section_end = section.virtual_address.saturating_add(section.size);
        if rva >= section.virtual_address && rva < section_end {
            return Ok((section.raw_pointer + (rva - section.virtual_address)) as usize);
        }
    }
    Err(VisionFfiError::fatal(
        "vision-provider-check",
        format!("PE export RVA is not mapped by any section: 0x{rva:x}"),
    ))
}

fn read_u16(bytes: &[u8], offset: usize) -> VisionFfiResult<u16> {
    require_range(bytes, offset, 2, "u16")?;
    Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> VisionFfiResult<u32> {
    require_range(bytes, offset, 4, "u32")?;
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

fn read_c_string(bytes: &[u8], offset: usize) -> VisionFfiResult<String> {
    if offset >= bytes.len() {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            format!("PE export string offset is out of bounds: {offset}"),
        ));
    }
    let Some(end) = bytes[offset..].iter().position(|byte| *byte == 0) else {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            "PE export string is not null-terminated",
        ));
    };
    let slice = &bytes[offset..offset + end];
    String::from_utf8(slice.to_vec()).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("PE export name is not valid UTF-8: {err}"),
        )
    })
}

fn require_range(bytes: &[u8], offset: usize, len: usize, label: &str) -> VisionFfiResult<()> {
    let Some(end) = offset.checked_add(len) else {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            format!("PE export audit range overflow while reading {label}"),
        ));
    };
    if end > bytes.len() {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            format!("PE export audit input ended while reading {label}"),
        ));
    }
    Ok(())
}

fn collect_fastdeploy_artifacts(
    artifacts: &FastDeployPpocrArtifacts,
    out: &mut Vec<ArtifactLockEntry>,
) -> VisionFfiResult<()> {
    out.push(lock_entry(
        "fastdeploy_ppocr",
        "provider_library",
        &artifacts.provider_library_path,
    )?);
    for path in &artifacts.runtime_library_paths {
        out.push(lock_entry("fastdeploy_ppocr", "runtime_library", path)?);
    }
    out.push(lock_entry(
        "fastdeploy_ppocr",
        "detector_model",
        &artifacts.detector_model_path,
    )?);
    out.push(lock_entry(
        "fastdeploy_ppocr",
        "recognizer_model",
        &artifacts.recognizer_model_path,
    )?);
    out.push(lock_entry(
        "fastdeploy_ppocr",
        "dictionary",
        &artifacts.dictionary_path,
    )?);
    if let Some(path) = &artifacts.classifier_model_path {
        out.push(lock_entry("fastdeploy_ppocr", "classifier_model", path)?);
    }
    Ok(())
}

fn collect_onnxruntime_artifacts(
    artifacts: &OnnxRuntimeArtifacts,
    out: &mut Vec<ArtifactLockEntry>,
) -> VisionFfiResult<()> {
    out.push(lock_entry(
        "onnxruntime",
        "provider_library",
        &artifacts.provider_library_path,
    )?);
    if let Some(path) = &artifacts.runtime_library_path {
        out.push(lock_entry("onnxruntime", "runtime_library", path)?);
    }
    out.push(lock_entry("onnxruntime", "model", &artifacts.model_path)?);
    if let Some(path) = &artifacts.labels_path {
        out.push(lock_entry("onnxruntime", "labels", path)?);
    }
    Ok(())
}

fn lock_entry(
    backend: &'static str,
    role: &'static str,
    path: &Path,
) -> VisionFfiResult<ArtifactLockEntry> {
    let mut file = File::open(path).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!(
                "failed to read artifact {role} at {}: {err}",
                path.display()
            ),
        )
    })?;
    let metadata = file.metadata().map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!(
                "failed to read artifact metadata {role} at {}: {err}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(VisionFfiError::fatal(
            "vision-provider-check",
            format!("artifact {role} is not a file: {}", path.display()),
        ));
    }

    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|err| {
            VisionFfiError::fatal(
                "vision-provider-check",
                format!(
                    "failed to hash artifact {role} at {}: {err}",
                    path.display()
                ),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(ArtifactLockEntry {
        backend,
        role,
        path: path_string(path),
        size_bytes: metadata.len(),
        sha256: hex_sha256(&hasher.finalize()),
    })
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> VisionFfiResult<()> {
    let json = serde_json::to_vec_pretty(value).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("failed to serialize artifact lock report: {err}"),
        )
    })?;
    fs::write(path, json).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!(
                "failed to write artifact lock report {}: {err}",
                path.display()
            ),
        )
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn load_png_frame(path: &Path) -> VisionFfiResult<(FrameReport, VisionFrame)> {
    let bytes = fs::read(path).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("failed to read frame PNG {}: {err}", path.display()),
        )
    })?;
    let image = image::load_from_memory_with_format(&bytes, ImageFormat::Png).map_err(|err| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("failed to decode frame PNG {}: {err}", path.display()),
        )
    })?;
    let rgb = image.to_rgb8();
    let width = rgb.width();
    let height = rgb.height();
    let frame = VisionFrame::new(width, height, VisionPixelFormat::Rgb8, rgb.into_raw())?;
    Ok((
        FrameReport {
            path: path_string(path),
            width,
            height,
            pixel_format: "rgb8",
        },
        frame,
    ))
}

fn fastdeploy_report(artifacts: &FastDeployPpocrArtifacts) -> BackendReport {
    let mut required_paths = vec![
        path_string(&artifacts.detector_model_path),
        path_string(&artifacts.recognizer_model_path),
        path_string(&artifacts.dictionary_path),
    ];
    required_paths.extend(
        artifacts
            .runtime_library_paths
            .iter()
            .map(|path| path_string(path)),
    );
    if let Some(path) = &artifacts.classifier_model_path {
        required_paths.push(path_string(path));
    }
    BackendReport {
        id: "fastdeploy_ppocr",
        configured: true,
        provider_library_path: Some(path_string(&artifacts.provider_library_path)),
        required_paths,
        execution_provider: None,
    }
}

fn onnxruntime_report(artifacts: &OnnxRuntimeArtifacts) -> BackendReport {
    let mut required_paths = vec![path_string(&artifacts.model_path)];
    if let Some(path) = &artifacts.runtime_library_path {
        required_paths.push(path_string(path));
    }
    if let Some(path) = &artifacts.labels_path {
        required_paths.push(path_string(path));
    }
    BackendReport {
        id: "onnxruntime",
        configured: true,
        provider_library_path: Some(path_string(&artifacts.provider_library_path)),
        required_paths,
        execution_provider: Some("cpu"),
    }
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

impl BackendSelection {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::FastDeployPpocr => "fastdeploy_ppocr",
            Self::OnnxRuntime => "onnxruntime",
        }
    }
}

fn usage() -> &'static str {
    "Usage: actingcommand-vision-provider-check --manifest <path> [--backend all|fastdeploy_ppocr|onnxruntime] [--require-existing] [--ocr-frame <png> [--ocr-region x,y,width,height] | --nn-frame <png> [--nn-model-id <id>] | --artifact-lock [--lock-out <json>] | --abi-check]\n       actingcommand-vision-provider-check --export-audit <dll> [--expect none|fastdeploy_ppocr_provider|onnxruntime_provider]"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn parses_required_manifest_arg() {
        let options = parse_args([
            "--manifest".to_string(),
            "resources/vision-provider-artifacts.example.json".to_string(),
        ])
        .expect("parse");

        assert_eq!(
            options.manifest,
            PathBuf::from("resources/vision-provider-artifacts.example.json")
        );
        assert_eq!(options.backend, BackendSelection::All);
        assert!(!options.require_existing);
        assert_eq!(options.mode, CheckMode::Manifest);
    }

    #[test]
    fn parses_backend_and_existing_flag() {
        let options = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--backend".to_string(),
            "onnxruntime".to_string(),
            "--require-existing".to_string(),
        ])
        .expect("parse");

        assert_eq!(options.backend, BackendSelection::OnnxRuntime);
        assert!(options.require_existing);
        assert_eq!(options.mode, CheckMode::Manifest);
    }

    #[test]
    fn missing_manifest_arg_is_fatal() {
        let err = parse_args(Vec::<String>::new()).expect_err("missing manifest rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(err.message().contains("--manifest is required"));
    }

    #[test]
    fn parses_ocr_smoke_frame_and_region() {
        let options = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--ocr-frame".to_string(),
            "frame.png".to_string(),
            "--ocr-region".to_string(),
            "1,2,30,40".to_string(),
        ])
        .expect("parse");

        assert_eq!(
            options.mode,
            CheckMode::OcrSmoke {
                frame: PathBuf::from("frame.png"),
                region: Some(VisionRect {
                    x: 1,
                    y: 2,
                    width: 30,
                    height: 40
                })
            }
        );
    }

    #[test]
    fn parses_nn_smoke_frame_and_model_id() {
        let options = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--nn-frame".to_string(),
            "frame.png".to_string(),
            "--nn-model-id".to_string(),
            "page-classifier".to_string(),
        ])
        .expect("parse");

        assert_eq!(
            options.mode,
            CheckMode::NnSmoke {
                frame: PathBuf::from("frame.png"),
                model_id: Some("page-classifier".to_string())
            }
        );
    }

    #[test]
    fn parses_artifact_lock_with_output_path() {
        let options = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--backend".to_string(),
            "onnxruntime".to_string(),
            "--artifact-lock".to_string(),
            "--lock-out".to_string(),
            "lock.json".to_string(),
        ])
        .expect("parse");

        assert_eq!(options.backend, BackendSelection::OnnxRuntime);
        assert_eq!(
            options.mode,
            CheckMode::ArtifactLock {
                out: Some(PathBuf::from("lock.json"))
            }
        );
    }

    #[test]
    fn parses_abi_check_mode() {
        let options = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--backend".to_string(),
            "fastdeploy_ppocr".to_string(),
            "--abi-check".to_string(),
        ])
        .expect("parse");

        assert_eq!(options.backend, BackendSelection::FastDeployPpocr);
        assert_eq!(options.mode, CheckMode::AbiCheck);
    }

    #[test]
    fn parses_export_audit_without_manifest() {
        let options = parse_args([
            "--export-audit".to_string(),
            "provider.dll".to_string(),
            "--expect".to_string(),
            "fastdeploy_ppocr_provider".to_string(),
        ])
        .expect("parse");

        assert_eq!(options.manifest, PathBuf::new());
        assert_eq!(
            options.mode,
            CheckMode::ExportAudit {
                library: PathBuf::from("provider.dll"),
                expectation: ExportExpectation::FastDeployPpocrProvider,
            }
        );
    }

    #[test]
    fn rejects_expect_without_export_audit() {
        let err = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--expect".to_string(),
            "fastdeploy_ppocr_provider".to_string(),
        ])
        .expect_err("expect without export audit rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(err.message().contains("--expect requires --export-audit"));
    }

    #[test]
    fn rejects_export_audit_mixed_with_backend() {
        let err = parse_args([
            "--export-audit".to_string(),
            "provider.dll".to_string(),
            "--backend".to_string(),
            "fastdeploy_ppocr".to_string(),
        ])
        .expect_err("mixed export audit rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(err.message().contains("--export-audit cannot be used"));
    }

    #[test]
    fn rejects_artifact_lock_mixed_with_smoke() {
        let err = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--artifact-lock".to_string(),
            "--ocr-frame".to_string(),
            "frame.png".to_string(),
        ])
        .expect_err("mixed artifact lock and smoke rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(err.message().contains("cannot be used"));
    }

    #[test]
    fn rejects_abi_check_mixed_with_artifact_lock() {
        let err = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--abi-check".to_string(),
            "--artifact-lock".to_string(),
        ])
        .expect_err("mixed ABI check and artifact lock rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(err.message().contains("--abi-check cannot be used"));
    }

    #[test]
    fn rejects_abi_check_mixed_with_smoke() {
        let err = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--abi-check".to_string(),
            "--nn-frame".to_string(),
            "frame.png".to_string(),
        ])
        .expect_err("mixed ABI check and smoke rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(err.message().contains("--abi-check cannot be used"));
    }

    #[test]
    fn rejects_mixed_ocr_and_nn_smoke() {
        let err = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--ocr-frame".to_string(),
            "ocr.png".to_string(),
            "--nn-frame".to_string(),
            "nn.png".to_string(),
        ])
        .expect_err("mixed smoke modes rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(
            err.message()
                .contains("cannot be used in the same invocation")
        );
    }

    #[test]
    fn rejects_wrong_backend_for_smoke_mode() {
        let err = parse_args([
            "--manifest".to_string(),
            "manifest.json".to_string(),
            "--backend".to_string(),
            "onnxruntime".to_string(),
            "--ocr-frame".to_string(),
            "frame.png".to_string(),
        ])
        .expect_err("wrong backend rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(err.message().contains("--ocr-frame cannot be used"));
    }

    #[test]
    fn build_report_requires_both_backends_by_default() {
        let root = temp_fixture_dir("missing-backend");
        let manifest = root.join("manifest.json");
        fs::write(
            &manifest,
            r#"{
                "schema_version": "actingcommand.vision_provider_artifacts.v0.1",
                "fastdeploy_ppocr": null,
                "onnxruntime": null
            }"#,
        )
        .expect("manifest");

        let err = build_report(&CheckOptions {
            manifest,
            backend: BackendSelection::All,
            require_existing: false,
            mode: CheckMode::Manifest,
        })
        .expect_err("missing backend rejected");

        assert_eq!(err.module(), "vision-artifacts");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_report_accepts_example_shape_without_existing_files() {
        let root = temp_fixture_dir("example-shape");
        let manifest = root.join("manifest.json");
        fs::write(&manifest, example_manifest_json()).expect("manifest");

        let report = build_report(&CheckOptions {
            manifest,
            backend: BackendSelection::All,
            require_existing: false,
            mode: CheckMode::Manifest,
        })
        .expect("report");

        assert!(report.ok);
        assert_eq!(report.backends.len(), 2);
        assert_eq!(report.backends[0].id, "fastdeploy_ppocr");
        assert_eq!(report.backends[1].execution_provider, Some("cpu"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn require_existing_rejects_missing_artifacts() {
        let root = temp_fixture_dir("missing-files");
        let manifest = root.join("manifest.json");
        fs::write(&manifest, example_manifest_json()).expect("manifest");

        let err = build_report(&CheckOptions {
            manifest,
            backend: BackendSelection::FastDeployPpocr,
            require_existing: true,
            mode: CheckMode::Manifest,
        })
        .expect_err("missing files rejected");

        assert_eq!(err.module(), "fastdeploy-ppocr");
        assert!(err.message().contains("required artifact"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ocr_smoke_requires_existing_artifacts_before_fake_success() {
        let root = temp_fixture_dir("ocr-smoke-missing-files");
        let manifest = root.join("manifest.json");
        fs::write(&manifest, example_manifest_json()).expect("manifest");

        let err = run_ocr_smoke(&CheckOptions {
            manifest,
            backend: BackendSelection::FastDeployPpocr,
            require_existing: false,
            mode: CheckMode::OcrSmoke {
                frame: root.join("frame.png"),
                region: None,
            },
        })
        .expect_err("missing provider files rejected");

        assert_eq!(err.module(), "fastdeploy-ppocr");
        assert!(err.message().contains("required artifact"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nn_smoke_requires_existing_artifacts_before_fake_success() {
        let root = temp_fixture_dir("nn-smoke-missing-files");
        let manifest = root.join("manifest.json");
        fs::write(&manifest, example_manifest_json()).expect("manifest");

        let err = run_nn_smoke(&CheckOptions {
            manifest,
            backend: BackendSelection::OnnxRuntime,
            require_existing: false,
            mode: CheckMode::NnSmoke {
                frame: root.join("frame.png"),
                model_id: None,
            },
        })
        .expect_err("missing provider files rejected");

        assert_eq!(err.module(), "onnxruntime");
        assert!(err.message().contains("required artifact"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_lock_reports_size_and_sha256_for_selected_backend() {
        let root = temp_fixture_dir("artifact-lock");
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&artifacts).expect("artifact dir");
        write_artifact(&artifacts.join("onnx-provider.dll"), b"provider");
        write_artifact(&artifacts.join("onnxruntime.dll"), b"runtime");
        write_artifact(&artifacts.join("model.onnx"), b"model");
        write_artifact(&artifacts.join("labels.txt"), b"home\nunknown\n");
        let manifest = root.join("manifest.json");
        fs::write(
            &manifest,
            format!(
                r#"{{
                    "schema_version": "actingcommand.vision_provider_artifacts.v0.1",
                    "fastdeploy_ppocr": null,
                    "onnxruntime": {{
                        "provider_library_path": "{}",
                        "runtime_library_path": "{}",
                        "model_path": "{}",
                        "labels": ["home", "unknown"],
                        "labels_path": "{}",
                        "execution_provider": "cpu",
                        "default_timeout_ms": 1000
                    }}
                }}"#,
                json_path(&artifacts.join("onnx-provider.dll")),
                json_path(&artifacts.join("onnxruntime.dll")),
                json_path(&artifacts.join("model.onnx")),
                json_path(&artifacts.join("labels.txt")),
            ),
        )
        .expect("manifest");

        let report = run_artifact_lock(&CheckOptions {
            manifest,
            backend: BackendSelection::OnnxRuntime,
            require_existing: false,
            mode: CheckMode::ArtifactLock { out: None },
        })
        .expect("artifact lock");

        assert!(report.ok);
        assert_eq!(report.backend, "onnxruntime");
        assert_eq!(report.artifacts.len(), 4);
        assert_eq!(report.total_size_bytes, 8 + 7 + 5 + 13);
        assert_eq!(
            report.artifacts[0].sha256,
            "5c4c1964340aca5b65393bbe9d3249cdd71be26665b3320ad694f034f2743283"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_lock_includes_fastdeploy_runtime_libraries() {
        let root = temp_fixture_dir("artifact-lock-fastdeploy-runtime");
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&artifacts).expect("artifact dir");
        write_artifact(&artifacts.join("provider.dll"), b"provider");
        write_artifact(&artifacts.join("runtime.dll"), b"runtime");
        write_artifact(&artifacts.join("det.pdmodel"), b"det");
        write_artifact(&artifacts.join("rec.pdmodel"), b"rec");
        write_artifact(&artifacts.join("keys.txt"), b"keys");
        let manifest = root.join("manifest.json");
        fs::write(
            &manifest,
            format!(
                r#"{{
                    "schema_version": "actingcommand.vision_provider_artifacts.v0.1",
                    "fastdeploy_ppocr": {{
                        "provider_library_path": "{}",
                        "runtime_library_paths": ["{}"],
                        "detector_model_path": "{}",
                        "recognizer_model_path": "{}",
                        "dictionary_path": "{}",
                        "classifier_model_path": null,
                        "supported_languages": ["zh_cn"],
                        "default_timeout_ms": 1000
                    }},
                    "onnxruntime": null
                }}"#,
                json_path(&artifacts.join("provider.dll")),
                json_path(&artifacts.join("runtime.dll")),
                json_path(&artifacts.join("det.pdmodel")),
                json_path(&artifacts.join("rec.pdmodel")),
                json_path(&artifacts.join("keys.txt")),
            ),
        )
        .expect("manifest");

        let report = run_artifact_lock(&CheckOptions {
            manifest,
            backend: BackendSelection::FastDeployPpocr,
            require_existing: false,
            mode: CheckMode::ArtifactLock { out: None },
        })
        .expect("artifact lock");

        let roles: Vec<_> = report
            .artifacts
            .iter()
            .map(|artifact| artifact.role)
            .collect();
        assert_eq!(
            roles,
            vec![
                "provider_library",
                "runtime_library",
                "detector_model",
                "recognizer_model",
                "dictionary",
            ]
        );
        assert_eq!(report.total_size_bytes, 8 + 7 + 3 + 3 + 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_lock_writes_report_when_requested() {
        let root = temp_fixture_dir("artifact-lock-out");
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&artifacts).expect("artifact dir");
        write_artifact(&artifacts.join("provider.dll"), b"nn");
        write_artifact(&artifacts.join("runtime.dll"), b"rt");
        write_artifact(&artifacts.join("model.onnx"), b"x");
        let manifest = root.join("manifest.json");
        let out = root.join("lock.json");
        fs::write(
            &manifest,
            format!(
                r#"{{
                    "schema_version": "actingcommand.vision_provider_artifacts.v0.1",
                    "fastdeploy_ppocr": null,
                    "onnxruntime": {{
                        "provider_library_path": "{}",
                        "runtime_library_path": "{}",
                        "model_path": "{}",
                        "labels": ["home"],
                        "labels_path": null,
                        "execution_provider": "cpu",
                        "default_timeout_ms": 1000
                    }}
                }}"#,
                json_path(&artifacts.join("provider.dll")),
                json_path(&artifacts.join("runtime.dll")),
                json_path(&artifacts.join("model.onnx")),
            ),
        )
        .expect("manifest");

        run_artifact_lock(&CheckOptions {
            manifest,
            backend: BackendSelection::OnnxRuntime,
            require_existing: false,
            mode: CheckMode::ArtifactLock {
                out: Some(out.clone()),
            },
        })
        .expect("artifact lock");

        let written = fs::read_to_string(&out).expect("lock report");
        assert!(written.contains("\"total_size_bytes\": 5"));
        assert!(written.contains("\"sha256\""));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn abi_check_rejects_missing_artifacts_before_symbol_success() {
        let root = temp_fixture_dir("abi-missing-files");
        let manifest = root.join("manifest.json");
        fs::write(&manifest, example_manifest_json()).expect("manifest");

        let err = run_abi_check(&CheckOptions {
            manifest,
            backend: BackendSelection::FastDeployPpocr,
            require_existing: false,
            mode: CheckMode::AbiCheck,
        })
        .expect_err("missing artifacts rejected");

        assert_eq!(err.module(), "fastdeploy-ppocr");
        assert!(err.message().contains("required artifact"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn abi_check_rejects_existing_file_without_provider_abi() {
        let root = temp_fixture_dir("abi-bad-provider");
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&artifacts).expect("artifact dir");
        write_artifact(&artifacts.join("provider.dll"), b"not a dynamic library");
        write_artifact(&artifacts.join("runtime.dll"), b"runtime");
        write_artifact(&artifacts.join("model.onnx"), b"model");
        let manifest = root.join("manifest.json");
        fs::write(
            &manifest,
            format!(
                r#"{{
                    "schema_version": "actingcommand.vision_provider_artifacts.v0.1",
                    "fastdeploy_ppocr": null,
                    "onnxruntime": {{
                        "provider_library_path": "{}",
                        "runtime_library_path": "{}",
                        "model_path": "{}",
                        "labels": ["home"],
                        "labels_path": null,
                        "execution_provider": "cpu",
                        "default_timeout_ms": 1000
                    }}
                }}"#,
                json_path(&artifacts.join("provider.dll")),
                json_path(&artifacts.join("runtime.dll")),
                json_path(&artifacts.join("model.onnx")),
            ),
        )
        .expect("manifest");

        let err = run_abi_check(&CheckOptions {
            manifest,
            backend: BackendSelection::OnnxRuntime,
            require_existing: false,
            mode: CheckMode::AbiCheck,
        })
        .expect_err("invalid provider library rejected");

        assert_eq!(err.module(), "onnxruntime");
        assert!(err.message().contains("failed to load FFI library"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pe_export_parser_reads_synthetic_exports() {
        let exports = parse_pe_exports(&synthetic_pe_with_exports(&[
            "ac_vision_free_buffer",
            "?CxxSymbol@@YAXXZ",
        ]))
        .expect("parse exports");

        assert_eq!(
            exports,
            vec![
                "?CxxSymbol@@YAXXZ".to_string(),
                "ac_vision_free_buffer".to_string()
            ]
        );
    }

    #[test]
    fn export_audit_reports_missing_provider_symbols_without_fake_success() {
        let root = temp_fixture_dir("export-audit");
        let library = root.join("runtime.dll");
        fs::write(
            &library,
            synthetic_pe_with_exports(&["?CxxSymbol@@YAXXZ", "ac_vision_free_buffer"]),
        )
        .expect("synthetic PE");

        let report = run_export_audit(&CheckOptions {
            manifest: PathBuf::new(),
            backend: BackendSelection::All,
            require_existing: false,
            mode: CheckMode::ExportAudit {
                library,
                expectation: ExportExpectation::FastDeployPpocrProvider,
            },
        })
        .expect("export audit");

        assert!(!report.ok);
        assert_eq!(report.export_count, 2);
        assert_eq!(report.msvc_cxx_symbol_count, 1);
        assert_eq!(report.present_symbols, vec!["ac_vision_free_buffer"]);
        assert_eq!(
            report.missing_symbols,
            vec!["ac_fastdeploy_ppocr_read_text_json"]
        );
        let _ = fs::remove_dir_all(root);
    }

    fn temp_fixture_dir(label: &str) -> PathBuf {
        let index = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "actingcommand-vision-provider-check-{label}-{}-{index}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    fn write_artifact(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("artifact");
    }

    fn json_path(path: &Path) -> String {
        path.display().to_string().replace('\\', "\\\\")
    }

    fn synthetic_pe_with_exports(exports: &[&str]) -> Vec<u8> {
        let mut bytes = vec![0_u8; 0x800];
        bytes[0] = b'M';
        bytes[1] = b'Z';
        write_u32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        write_u16(&mut bytes, coff, 0x8664);
        write_u16(&mut bytes, coff + 2, 1);
        write_u16(&mut bytes, coff + 16, 240);
        let optional = coff + 20;
        write_u16(&mut bytes, optional, 0x20b);
        let data_directory = optional + 112;
        write_u32(&mut bytes, data_directory, 0x1000);
        write_u32(&mut bytes, data_directory + 4, 0x80);
        let section = optional + 240;
        bytes[section..section + 6].copy_from_slice(b".rdata");
        write_u32(&mut bytes, section + 8, 0x1000);
        write_u32(&mut bytes, section + 12, 0x1000);
        write_u32(&mut bytes, section + 16, 0x400);
        write_u32(&mut bytes, section + 20, 0x200);

        let export_dir = 0x200;
        write_u32(&mut bytes, export_dir + 24, exports.len() as u32);
        write_u32(&mut bytes, export_dir + 32, 0x1040);
        let name_table = 0x240;
        let mut string_offset = 0x260;
        for (index, export) in exports.iter().enumerate() {
            write_u32(
                &mut bytes,
                name_table + index * 4,
                0x1000 + (string_offset - 0x200) as u32,
            );
            bytes[string_offset..string_offset + export.len()].copy_from_slice(export.as_bytes());
            bytes[string_offset + export.len()] = 0;
            string_offset += export.len() + 1;
        }
        bytes
    }

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn example_manifest_json() -> &'static str {
        r#"{
            "schema_version": "actingcommand.vision_provider_artifacts.v0.1",
            "fastdeploy_ppocr": {
                "provider_library_path": "external-tools/vision/fastdeploy/ac_fastdeploy_ppocr.dll",
                "runtime_library_paths": [
                    "external-tools/vision/fastdeploy/fastdeploy_ppocr_maa.dll"
                ],
                "detector_model_path": "external-tools/vision/ppocr/det/inference.pdmodel",
                "recognizer_model_path": "external-tools/vision/ppocr/rec/inference.pdmodel",
                "dictionary_path": "external-tools/vision/ppocr/ppocr_keys_v1.txt",
                "classifier_model_path": null,
                "supported_languages": ["zh_cn", "en"],
                "default_timeout_ms": 1000
            },
            "onnxruntime": {
                "provider_library_path": "external-tools/vision/onnxruntime/ac_onnxruntime.dll",
                "runtime_library_path": "external-tools/vision/onnxruntime/onnxruntime.dll",
                "model_path": "external-tools/vision/onnxruntime/models/page_classifier.onnx",
                "labels": ["home", "unknown"],
                "labels_path": null,
                "execution_provider": "cpu",
                "default_timeout_ms": 1000
            }
        }"#
    }
}
