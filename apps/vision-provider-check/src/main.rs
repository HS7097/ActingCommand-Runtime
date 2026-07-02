// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_vision_ffi::{
    FastDeployPpocrArtifacts, FastDeployPpocrBackend, NnEngine, NnInferenceRequest, OcrEngine,
    OcrInferenceRequest, OnnxRuntimeArtifacts, OnnxRuntimeBackend, VisionFfiError, VisionFfiResult,
    VisionFrame, VisionPixelFormat, VisionProviderArtifactManifest, VisionRect,
};
use image::ImageFormat;
use serde::Serialize;
use std::env;
use std::fs;
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
    let mode = match (ocr_frame, nn_frame) {
        (Some(frame), None) => CheckMode::OcrSmoke {
            frame,
            region: ocr_region,
        },
        (None, Some(frame)) => CheckMode::NnSmoke {
            frame,
            model_id: nn_model_id,
        },
        (None, None) => CheckMode::Manifest,
        (Some(_), Some(_)) => unreachable!("checked above"),
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
    "Usage: actingcommand-vision-provider-check --manifest <path> [--backend all|fastdeploy_ppocr|onnxruntime] [--require-existing] [--ocr-frame <png> [--ocr-region x,y,width,height] | --nn-frame <png> [--nn-model-id <id>]]"
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
