// SPDX-License-Identifier: AGPL-3.0-only

use crate::{NnInferenceRequest, OcrInferenceRequest, VisionFfiError, VisionFfiResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION: &str =
    "actingcommand.vision_provider_artifacts.v0.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionProviderArtifactManifest {
    pub schema_version: String,
    pub fastdeploy_ppocr: Option<FastDeployPpocrArtifacts>,
    pub onnxruntime: Option<OnnxRuntimeArtifacts>,
}

impl VisionProviderArtifactManifest {
    pub fn from_json_slice(bytes: &[u8]) -> VisionFfiResult<Self> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(|err| {
            VisionFfiError::fatal(
                "vision-artifacts",
                format!("failed to parse provider artifact manifest JSON: {err}"),
            )
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load_json_file(path: impl AsRef<Path>) -> VisionFfiResult<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|err| {
            VisionFfiError::fatal(
                "vision-artifacts",
                format!(
                    "failed to read provider artifact manifest {}: {err}",
                    path.display()
                ),
            )
        })?;
        Self::from_json_slice(&bytes)
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        if self.schema_version != VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION {
            return Err(VisionFfiError::fatal(
                "vision-artifacts",
                format!(
                    "unsupported vision provider artifact schema_version: {}",
                    self.schema_version
                ),
            ));
        }
        if let Some(artifacts) = &self.fastdeploy_ppocr {
            artifacts.validate()?;
        }
        if let Some(artifacts) = &self.onnxruntime {
            artifacts.validate()?;
        }
        Ok(())
    }

    pub fn validate_existing_files(&self) -> VisionFfiResult<()> {
        self.validate()?;
        if let Some(artifacts) = &self.fastdeploy_ppocr {
            artifacts.validate_existing_files()?;
        }
        if let Some(artifacts) = &self.onnxruntime {
            artifacts.validate_existing_files()?;
        }
        Ok(())
    }

    pub fn require_fastdeploy_ppocr(&self) -> VisionFfiResult<&FastDeployPpocrArtifacts> {
        self.fastdeploy_ppocr.as_ref().ok_or_else(|| {
            VisionFfiError::fatal(
                "vision-artifacts",
                "provider artifact manifest does not include fastdeploy_ppocr",
            )
        })
    }

    pub fn require_onnxruntime(&self) -> VisionFfiResult<&OnnxRuntimeArtifacts> {
        self.onnxruntime.as_ref().ok_or_else(|| {
            VisionFfiError::fatal(
                "vision-artifacts",
                "provider artifact manifest does not include onnxruntime",
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastDeployPpocrArtifacts {
    pub provider_library_path: PathBuf,
    pub detector_model_path: PathBuf,
    pub recognizer_model_path: PathBuf,
    pub dictionary_path: PathBuf,
    pub classifier_model_path: Option<PathBuf>,
    pub supported_languages: Vec<String>,
    pub default_timeout_ms: u64,
}

impl FastDeployPpocrArtifacts {
    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_required_path(
            "fastdeploy-ppocr",
            "provider_library_path",
            &self.provider_library_path,
        )?;
        validate_required_path(
            "fastdeploy-ppocr",
            "detector_model_path",
            &self.detector_model_path,
        )?;
        validate_required_path(
            "fastdeploy-ppocr",
            "recognizer_model_path",
            &self.recognizer_model_path,
        )?;
        validate_required_path("fastdeploy-ppocr", "dictionary_path", &self.dictionary_path)?;
        if let Some(path) = &self.classifier_model_path {
            validate_required_path("fastdeploy-ppocr", "classifier_model_path", path)?;
        }
        if self.supported_languages.is_empty() {
            return Err(VisionFfiError::fatal(
                "fastdeploy-ppocr",
                "supported_languages must include at least one language",
            ));
        }
        if self
            .supported_languages
            .iter()
            .any(|language| language.trim().is_empty())
        {
            return Err(VisionFfiError::fatal(
                "fastdeploy-ppocr",
                "supported_languages must not contain blank entries",
            ));
        }
        if self.default_timeout_ms == 0 {
            return Err(VisionFfiError::fatal(
                "fastdeploy-ppocr",
                "default_timeout_ms must be non-zero",
            ));
        }
        Ok(())
    }

    pub fn validate_existing_files(&self) -> VisionFfiResult<()> {
        self.validate()?;
        require_existing_file(
            "fastdeploy-ppocr",
            "provider_library_path",
            &self.provider_library_path,
        )?;
        require_existing_file(
            "fastdeploy-ppocr",
            "detector_model_path",
            &self.detector_model_path,
        )?;
        require_existing_file(
            "fastdeploy-ppocr",
            "recognizer_model_path",
            &self.recognizer_model_path,
        )?;
        require_existing_file("fastdeploy-ppocr", "dictionary_path", &self.dictionary_path)?;
        if let Some(path) = &self.classifier_model_path {
            require_existing_file("fastdeploy-ppocr", "classifier_model_path", path)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnnxRuntimeArtifacts {
    pub provider_library_path: PathBuf,
    pub model_path: PathBuf,
    pub labels: Vec<String>,
    pub labels_path: Option<PathBuf>,
    pub execution_provider: OnnxExecutionProvider,
    pub default_timeout_ms: u64,
}

impl OnnxRuntimeArtifacts {
    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_required_path(
            "onnxruntime",
            "provider_library_path",
            &self.provider_library_path,
        )?;
        validate_required_path("onnxruntime", "model_path", &self.model_path)?;
        if let Some(path) = &self.labels_path {
            validate_required_path("onnxruntime", "labels_path", path)?;
        }
        if self.labels.is_empty() {
            return Err(VisionFfiError::fatal(
                "onnxruntime",
                "labels must include at least one label",
            ));
        }
        if self.labels.iter().any(|label| label.trim().is_empty()) {
            return Err(VisionFfiError::fatal(
                "onnxruntime",
                "labels must not contain blank entries",
            ));
        }
        if self.default_timeout_ms == 0 {
            return Err(VisionFfiError::fatal(
                "onnxruntime",
                "default_timeout_ms must be non-zero",
            ));
        }
        Ok(())
    }

    pub fn validate_existing_files(&self) -> VisionFfiResult<()> {
        self.validate()?;
        require_existing_file(
            "onnxruntime",
            "provider_library_path",
            &self.provider_library_path,
        )?;
        require_existing_file("onnxruntime", "model_path", &self.model_path)?;
        if let Some(path) = &self.labels_path {
            require_existing_file("onnxruntime", "labels_path", path)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnnxExecutionProvider {
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastDeployPpocrInvokeRequest {
    pub request: OcrInferenceRequest,
    pub artifacts: FastDeployPpocrArtifacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnnxRuntimeInvokeRequest {
    pub request: NnInferenceRequest,
    pub artifacts: OnnxRuntimeArtifacts,
}

fn validate_required_path(module: &'static str, field: &str, path: &Path) -> VisionFfiResult<()> {
    if path.as_os_str().is_empty() {
        return Err(VisionFfiError::fatal(
            module,
            format!("{field} must be a non-empty path"),
        ));
    }
    Ok(())
}

fn require_existing_file(module: &'static str, field: &str, path: &Path) -> VisionFfiResult<()> {
    let metadata = path.metadata().map_err(|err| {
        VisionFfiError::fatal(
            module,
            format!(
                "required artifact {field} is unavailable at {}: {err}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(VisionFfiError::fatal(
            module,
            format!(
                "required artifact {field} is not a file: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_manifest_accepts_cpu_only_route() {
        let manifest = VisionProviderArtifactManifest {
            schema_version: VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION.to_string(),
            fastdeploy_ppocr: Some(test_ocr_artifacts()),
            onnxruntime: Some(test_nn_artifacts()),
        };

        manifest.validate().expect("valid artifact manifest");
    }

    #[test]
    fn artifact_manifest_rejects_unknown_schema() {
        let manifest = VisionProviderArtifactManifest {
            schema_version: "unknown".to_string(),
            fastdeploy_ppocr: None,
            onnxruntime: None,
        };

        let err = manifest.validate().expect_err("unknown schema rejected");

        assert_eq!(err.module(), "vision-artifacts");
    }

    #[test]
    fn artifact_manifest_parses_json_contract() {
        let manifest = VisionProviderArtifactManifest::from_json_slice(
            br#"{
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
            }"#,
        )
        .expect("manifest JSON");

        assert_eq!(
            manifest
                .require_fastdeploy_ppocr()
                .expect("ocr artifacts")
                .supported_languages[0],
            "zh_cn"
        );
        assert_eq!(
            manifest
                .require_onnxruntime()
                .expect("nn artifacts")
                .execution_provider,
            OnnxExecutionProvider::Cpu
        );
    }

    #[test]
    fn artifact_manifest_rejects_invalid_json() {
        let err =
            VisionProviderArtifactManifest::from_json_slice(br#"{"#).expect_err("bad JSON fatal");

        assert_eq!(err.module(), "vision-artifacts");
        assert!(err.message().contains("failed to parse"));
    }

    #[test]
    fn artifact_manifest_requires_requested_backend_section() {
        let manifest = VisionProviderArtifactManifest {
            schema_version: VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION.to_string(),
            fastdeploy_ppocr: None,
            onnxruntime: None,
        };

        let err = manifest
            .require_fastdeploy_ppocr()
            .expect_err("missing OCR section rejected");

        assert_eq!(err.module(), "vision-artifacts");
        assert!(err.message().contains("fastdeploy_ppocr"));
    }

    #[test]
    fn ocr_artifacts_reject_blank_language() {
        let mut artifacts = test_ocr_artifacts();
        artifacts.supported_languages.push(" ".to_string());

        let err = artifacts.validate().expect_err("blank language rejected");

        assert_eq!(err.module(), "fastdeploy-ppocr");
    }

    #[test]
    fn ocr_artifacts_reject_zero_timeout() {
        let mut artifacts = test_ocr_artifacts();
        artifacts.default_timeout_ms = 0;

        let err = artifacts.validate().expect_err("zero timeout rejected");

        assert_eq!(err.module(), "fastdeploy-ppocr");
    }

    #[test]
    fn nn_artifacts_reject_empty_labels() {
        let mut artifacts = test_nn_artifacts();
        artifacts.labels.clear();

        let err = artifacts.validate().expect_err("empty labels rejected");

        assert_eq!(err.module(), "onnxruntime");
    }

    #[test]
    fn existing_file_validation_is_fatal_for_missing_artifact() {
        let err = test_nn_artifacts()
            .validate_existing_files()
            .expect_err("missing file rejected");

        assert_eq!(err.module(), "onnxruntime");
        assert!(err.message().contains("required artifact"));
    }

    pub(crate) fn test_ocr_artifacts() -> FastDeployPpocrArtifacts {
        FastDeployPpocrArtifacts {
            provider_library_path: "external-tools/vision/fastdeploy/ac_fastdeploy_ppocr.dll"
                .into(),
            detector_model_path: "external-tools/vision/ppocr/det/inference.pdmodel".into(),
            recognizer_model_path: "external-tools/vision/ppocr/rec/inference.pdmodel".into(),
            dictionary_path: "external-tools/vision/ppocr/ppocr_keys_v1.txt".into(),
            classifier_model_path: None,
            supported_languages: vec!["zh_cn".to_string(), "en".to_string()],
            default_timeout_ms: 1_000,
        }
    }

    pub(crate) fn test_nn_artifacts() -> OnnxRuntimeArtifacts {
        OnnxRuntimeArtifacts {
            provider_library_path: "external-tools/vision/onnxruntime/ac_onnxruntime.dll".into(),
            model_path: "external-tools/vision/onnxruntime/models/page_classifier.onnx".into(),
            labels: vec!["home".to_string(), "unknown".to_string()],
            labels_path: None,
            execution_provider: OnnxExecutionProvider::Cpu,
            default_timeout_ms: 1_000,
        }
    }
}
