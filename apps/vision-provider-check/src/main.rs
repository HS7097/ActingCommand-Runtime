// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_vision_ffi::{
    FastDeployPpocrArtifacts, OnnxRuntimeArtifacts, VisionFfiError, VisionFfiResult,
    VisionProviderArtifactManifest,
};
use serde::Serialize;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckOptions {
    manifest: PathBuf,
    backend: BackendSelection,
    require_existing: bool,
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
    let report = build_report(&options)?;
    let json = serde_json::to_string_pretty(&report).map_err(|err| {
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

    let manifest = manifest.ok_or_else(|| {
        VisionFfiError::fatal(
            "vision-provider-check",
            format!("--manifest is required\n{}", usage()),
        )
    })?;

    Ok(CheckOptions {
        manifest,
        backend,
        require_existing,
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
    "Usage: actingcommand-vision-provider-check --manifest <path> [--backend all|fastdeploy_ppocr|onnxruntime] [--require-existing]"
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
    }

    #[test]
    fn missing_manifest_arg_is_fatal() {
        let err = parse_args(Vec::<String>::new()).expect_err("missing manifest rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(err.message().contains("--manifest is required"));
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
        })
        .expect_err("missing files rejected");

        assert_eq!(err.module(), "fastdeploy-ppocr");
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
