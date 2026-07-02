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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendSelection {
    All,
    FastDeployPpocr,
    OnnxRuntime,
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

    let manifest = manifest.ok_or_else(|| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("--manifest is required\n{}", usage()),
        )
    })?;
    let mode = match (abi_check, artifact_lock, ocr_frame, nn_frame) {
        (true, false, None, None) => CheckMode::AbiCheck,
        (false, true, None, None) => CheckMode::ArtifactLock { out: lock_out },
        (false, false, Some(frame), None) => CheckMode::OcrSmoke {
            frame,
            region: ocr_region,
        },
        (false, false, None, Some(frame)) => CheckMode::NnSmoke {
            frame,
            model_id: nn_model_id,
        },
        (false, false, None, None) => CheckMode::Manifest,
        _ => unreachable!("checked above"),
    };

    Ok(CheckOptions {
        manifest,
        backend,
        require_existing,
        mode,
    })
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

fn collect_fastdeploy_artifacts(
    artifacts: &FastDeployPpocrArtifacts,
    out: &mut Vec<ArtifactLockEntry>,
) -> VisionFfiResult<()> {
    out.push(lock_entry(
        "fastdeploy_ppocr",
        "provider_library",
        &artifacts.provider_library_path,
    )?);
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
    "Usage: actingcommand-vision-provider-check --manifest <path> [--backend all|fastdeploy_ppocr|onnxruntime] [--require-existing] [--ocr-frame <png> [--ocr-region x,y,width,height] | --nn-frame <png> [--nn-model-id <id>] | --artifact-lock [--lock-out <json>] | --abi-check]"
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
                        "model_path": "{}",
                        "labels": ["home", "unknown"],
                        "labels_path": "{}",
                        "execution_provider": "cpu",
                        "default_timeout_ms": 1000
                    }}
                }}"#,
                json_path(&artifacts.join("onnx-provider.dll")),
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
        assert_eq!(report.artifacts.len(), 3);
        assert_eq!(report.total_size_bytes, 8 + 5 + 13);
        assert_eq!(
            report.artifacts[0].sha256,
            "5c4c1964340aca5b65393bbe9d3249cdd71be26665b3320ad694f034f2743283"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_lock_writes_report_when_requested() {
        let root = temp_fixture_dir("artifact-lock-out");
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&artifacts).expect("artifact dir");
        write_artifact(&artifacts.join("provider.dll"), b"nn");
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
                        "model_path": "{}",
                        "labels": ["home"],
                        "labels_path": null,
                        "execution_provider": "cpu",
                        "default_timeout_ms": 1000
                    }}
                }}"#,
                json_path(&artifacts.join("provider.dll")),
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
        assert!(written.contains("\"total_size_bytes\": 3"));
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
                        "model_path": "{}",
                        "labels": ["home"],
                        "labels_path": null,
                        "execution_provider": "cpu",
                        "default_timeout_ms": 1000
                    }}
                }}"#,
                json_path(&artifacts.join("provider.dll")),
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

    fn example_manifest_json() -> &'static str {
        r#"{
            "schema_version": "actingcommand.vision_provider_artifacts.v0.1",
            "fastdeploy_ppocr": {
                "provider_library_path": "external-tools/vision/fastdeploy/ac_fastdeploy_ppocr.dll",
                "detector_model_path": "external-tools/vision/ppocr/det/inference.pdmodel",
                "recognizer_model_path": "external-tools/vision/ppocr/rec/inference.pdmodel",
                "dictionary_path": "external-tools/vision/ppocr/ppocr_keys_v1.txt",
                "classifier_model_path": null,
                "supported_languages": ["zh_cn", "en"],
                "default_timeout_ms": 1000
            },
            "onnxruntime": {
                "provider_library_path": "external-tools/vision/onnxruntime/ac_onnxruntime.dll",
                "model_path": "external-tools/vision/onnxruntime/models/page_classifier.onnx",
                "labels": ["home", "unknown"],
                "labels_path": null,
                "execution_provider": "cpu",
                "default_timeout_ms": 1000
            }
        }"#
    }
}
