// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    NnInferenceRequest, OcrInferenceRequest, VisionFfiError, VisionFfiErrorCode, VisionFfiResult,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION: &str =
    "actingcommand.vision_provider_artifacts.v0.2";
pub const LEGACY_VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION: &str =
    "actingcommand.vision_provider_artifacts.v0.1";
pub const PPOCR_V6_MEDIUM_MODEL_REF: &str = "PP-OCRv6_medium";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        match self.schema_version.as_str() {
            LEGACY_VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION => {
                if let Some(artifacts) = &self.fastdeploy_ppocr {
                    artifacts.validate()?;
                }
                if let Some(artifacts) = &self.onnxruntime {
                    artifacts.validate()?;
                }
            }
            VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION => {
                if let Some(artifacts) = &self.fastdeploy_ppocr {
                    artifacts.validate_ppocr_v6_cuda()?;
                }
                if let Some(artifacts) = &self.onnxruntime {
                    artifacts.validate_production_model()?;
                }
            }
            _ => {
                return Err(VisionFfiError::fatal(
                    "vision-artifacts",
                    format!(
                        "unsupported vision provider artifact schema_version: {}",
                        self.schema_version
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn validate_existing_files(&self) -> VisionFfiResult<()> {
        self.validate()?;
        if let Some(artifacts) = &self.fastdeploy_ppocr {
            if self.schema_version == VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION {
                artifacts.validate_ppocr_v6_cuda_existing_files()?;
            } else {
                artifacts.validate_existing_files()?;
            }
        }
        if let Some(artifacts) = &self.onnxruntime {
            if self.schema_version == VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION {
                artifacts.validate_production_existing_files()?;
            } else {
                artifacts.validate_existing_files()?;
            }
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

    pub fn require_production_fastdeploy_ppocr(
        &self,
    ) -> VisionFfiResult<&FastDeployPpocrArtifacts> {
        if self.schema_version != VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION {
            return Err(VisionFfiError::fatal(
                "vision-artifacts",
                format!(
                    "production OCR requires schema_version '{VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION}', got '{}'",
                    self.schema_version
                ),
            ));
        }
        let artifacts = self.require_fastdeploy_ppocr()?;
        artifacts.validate_ppocr_v6_cuda()?;
        Ok(artifacts)
    }

    pub fn require_onnxruntime(&self) -> VisionFfiResult<&OnnxRuntimeArtifacts> {
        self.onnxruntime.as_ref().ok_or_else(|| {
            VisionFfiError::fatal(
                "vision-artifacts",
                "provider artifact manifest does not include onnxruntime",
            )
        })
    }

    pub fn require_production_onnxruntime(&self) -> VisionFfiResult<&OnnxRuntimeArtifacts> {
        if self.schema_version != VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION {
            return Err(VisionFfiError::fatal(
                "vision-artifacts",
                format!(
                    "production NN requires schema_version '{VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION}', got '{}'",
                    self.schema_version
                ),
            ));
        }
        let artifacts = self.require_onnxruntime()?;
        artifacts.validate_production_model()?;
        Ok(artifacts)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FastDeployPpocrArtifacts {
    pub provider_library_path: PathBuf,
    #[serde(default)]
    pub runtime_library_paths: Vec<PathBuf>,
    pub detector_model_path: PathBuf,
    pub recognizer_model_path: PathBuf,
    pub dictionary_path: PathBuf,
    pub classifier_model_path: Option<PathBuf>,
    #[serde(default)]
    pub model_ref: Option<String>,
    #[serde(default)]
    pub model_sha256: Option<String>,
    #[serde(default)]
    pub detector_model_sha256: Option<String>,
    #[serde(default)]
    pub recognizer_model_sha256: Option<String>,
    #[serde(default)]
    pub dictionary_sha256: Option<String>,
    #[serde(default)]
    pub classifier_model_sha256: Option<String>,
    #[serde(default)]
    pub execution_provider: Option<OnnxExecutionProvider>,
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
        for path in &self.runtime_library_paths {
            validate_required_path("fastdeploy-ppocr", "runtime_library_paths", path)?;
        }
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
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ModelMismatch,
                "fastdeploy-ppocr",
                "supported_languages must include at least one language",
            ));
        }
        if self
            .supported_languages
            .iter()
            .any(|language| language.trim().is_empty())
        {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "fastdeploy-ppocr",
                "supported_languages must not contain blank entries",
            ));
        }
        if self.default_timeout_ms == 0 {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ModelMismatch,
                "fastdeploy-ppocr",
                "default_timeout_ms must be non-zero",
            ));
        }
        for (field, hash) in [
            ("model_sha256", self.model_sha256.as_deref()),
            (
                "detector_model_sha256",
                self.detector_model_sha256.as_deref(),
            ),
            (
                "recognizer_model_sha256",
                self.recognizer_model_sha256.as_deref(),
            ),
            ("dictionary_sha256", self.dictionary_sha256.as_deref()),
            (
                "classifier_model_sha256",
                self.classifier_model_sha256.as_deref(),
            ),
        ] {
            if let Some(hash) = hash {
                validate_sha256("fastdeploy-ppocr", field, hash)?;
            }
        }
        Ok(())
    }

    pub fn validate_ppocr_v6_cuda(&self) -> VisionFfiResult<()> {
        self.validate()?;
        if self.model_ref.as_deref() != Some(PPOCR_V6_MEDIUM_MODEL_REF) {
            return Err(VisionFfiError::fatal(
                "fastdeploy-ppocr",
                format!("production OCR model_ref must be '{PPOCR_V6_MEDIUM_MODEL_REF}'"),
            ));
        }
        if self.execution_provider != Some(OnnxExecutionProvider::Cuda) {
            return Err(VisionFfiError::fatal(
                "fastdeploy-ppocr",
                "production OCR execution_provider must be cuda; CPU, DirectML, CoreML, and fallback routes are forbidden",
            ));
        }
        let detector = require_hash(
            "fastdeploy-ppocr",
            "detector_model_sha256",
            self.detector_model_sha256.as_deref(),
        )?;
        let recognizer = require_hash(
            "fastdeploy-ppocr",
            "recognizer_model_sha256",
            self.recognizer_model_sha256.as_deref(),
        )?;
        let dictionary = require_hash(
            "fastdeploy-ppocr",
            "dictionary_sha256",
            self.dictionary_sha256.as_deref(),
        )?;
        let classifier = match (
            self.classifier_model_path.as_ref(),
            self.classifier_model_sha256.as_deref(),
        ) {
            (Some(_), Some(hash)) => Some(hash),
            (Some(_), None) => {
                return Err(VisionFfiError::fatal(
                    "fastdeploy-ppocr",
                    "classifier_model_sha256 is required when classifier_model_path is present",
                ));
            }
            (None, Some(_)) => {
                return Err(VisionFfiError::fatal(
                    "fastdeploy-ppocr",
                    "classifier_model_sha256 must be omitted when classifier_model_path is absent",
                ));
            }
            (None, None) => None,
        };
        let expected = ppocr_model_content_sha256(detector, recognizer, dictionary, classifier)?;
        let declared = require_hash(
            "fastdeploy-ppocr",
            "model_sha256",
            self.model_sha256.as_deref(),
        )?;
        if declared != expected {
            return Err(VisionFfiError::fatal(
                "fastdeploy-ppocr",
                format!(
                    "model_sha256 mismatch for {PPOCR_V6_MEDIUM_MODEL_REF}: declared={declared}, computed={expected}"
                ),
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
        for path in &self.runtime_library_paths {
            require_existing_file("fastdeploy-ppocr", "runtime_library_paths", path)?;
        }
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

    pub fn validate_ppocr_v6_cuda_existing_files(&self) -> VisionFfiResult<()> {
        self.validate_ppocr_v6_cuda()?;
        self.validate_existing_files()?;
        let detector_hash = require_hash(
            "fastdeploy-ppocr",
            "detector_model_sha256",
            self.detector_model_sha256.as_deref(),
        )?;
        let recognizer_hash = require_hash(
            "fastdeploy-ppocr",
            "recognizer_model_sha256",
            self.recognizer_model_sha256.as_deref(),
        )?;
        let dictionary_hash = require_hash(
            "fastdeploy-ppocr",
            "dictionary_sha256",
            self.dictionary_sha256.as_deref(),
        )?;
        verify_file_sha256(
            "fastdeploy-ppocr",
            "detector_model_path",
            &self.detector_model_path,
            detector_hash,
        )?;
        verify_file_sha256(
            "fastdeploy-ppocr",
            "recognizer_model_path",
            &self.recognizer_model_path,
            recognizer_hash,
        )?;
        verify_file_sha256(
            "fastdeploy-ppocr",
            "dictionary_path",
            &self.dictionary_path,
            dictionary_hash,
        )?;
        if let (Some(path), Some(hash)) = (
            self.classifier_model_path.as_ref(),
            self.classifier_model_sha256.as_deref(),
        ) {
            verify_file_sha256("fastdeploy-ppocr", "classifier_model_path", path, hash)?;
        }
        Ok(())
    }

    pub fn production_model_identity(&self) -> VisionFfiResult<(&str, &str)> {
        self.validate_ppocr_v6_cuda()?;
        let model_ref = self.model_ref.as_deref().ok_or_else(|| {
            VisionFfiError::fatal("fastdeploy-ppocr", "validated model_ref is missing")
        })?;
        let model_sha256 = require_hash(
            "fastdeploy-ppocr",
            "model_sha256",
            self.model_sha256.as_deref(),
        )?;
        Ok((model_ref, model_sha256))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnnxRuntimeArtifacts {
    pub provider_library_path: PathBuf,
    #[serde(default)]
    pub runtime_library_path: Option<PathBuf>,
    pub model_path: PathBuf,
    #[serde(default)]
    pub model_ref: Option<String>,
    #[serde(default)]
    pub model_sha256: Option<String>,
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
        if let Some(path) = &self.runtime_library_path {
            validate_required_path("onnxruntime", "runtime_library_path", path)?;
        }
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
        if let Some(model_sha256) = &self.model_sha256 {
            validate_sha256("onnxruntime", "model_sha256", model_sha256)?;
        }
        Ok(())
    }

    pub fn validate_production_model(&self) -> VisionFfiResult<()> {
        self.validate()?;
        let model_ref = self.model_ref.as_deref().ok_or_else(|| {
            VisionFfiError::fatal(
                "onnxruntime",
                "production NN model_ref must be a non-empty logical identifier",
            )
        })?;
        if model_ref.trim().is_empty()
            || model_ref.contains('/')
            || model_ref.contains('\\')
            || model_ref.contains(':')
        {
            return Err(VisionFfiError::fatal(
                "onnxruntime",
                "production NN model_ref must be a logical identifier, not a host path",
            ));
        }
        require_hash("onnxruntime", "model_sha256", self.model_sha256.as_deref())?;
        Ok(())
    }

    pub fn validate_existing_files(&self) -> VisionFfiResult<()> {
        self.validate()?;
        require_existing_file(
            "onnxruntime",
            "provider_library_path",
            &self.provider_library_path,
        )?;
        if let Some(path) = &self.runtime_library_path {
            require_existing_file("onnxruntime", "runtime_library_path", path)?;
        }
        require_existing_file("onnxruntime", "model_path", &self.model_path)?;
        if let Some(path) = &self.labels_path {
            require_existing_file("onnxruntime", "labels_path", path)?;
        }
        Ok(())
    }

    pub fn validate_production_existing_files(&self) -> VisionFfiResult<()> {
        self.validate_production_model()?;
        self.validate_existing_files()?;
        let model_sha256 =
            require_hash("onnxruntime", "model_sha256", self.model_sha256.as_deref())?;
        verify_file_sha256("onnxruntime", "model_path", &self.model_path, model_sha256)
    }

    pub fn production_model_identity(&self) -> VisionFfiResult<(&str, &str)> {
        self.validate_production_model()?;
        let model_ref = self.model_ref.as_deref().ok_or_else(|| {
            VisionFfiError::fatal("onnxruntime", "validated model_ref is missing")
        })?;
        let model_sha256 =
            require_hash("onnxruntime", "model_sha256", self.model_sha256.as_deref())?;
        Ok((model_ref, model_sha256))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnnxExecutionProvider {
    Cpu,
    Cuda,
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

pub fn ppocr_model_content_sha256(
    detector_model_sha256: &str,
    recognizer_model_sha256: &str,
    dictionary_sha256: &str,
    classifier_model_sha256: Option<&str>,
) -> VisionFfiResult<String> {
    for (field, hash) in [
        ("detector_model_sha256", detector_model_sha256),
        ("recognizer_model_sha256", recognizer_model_sha256),
        ("dictionary_sha256", dictionary_sha256),
    ] {
        validate_sha256("fastdeploy-ppocr", field, hash)?;
    }
    if let Some(hash) = classifier_model_sha256 {
        validate_sha256("fastdeploy-ppocr", "classifier_model_sha256", hash)?;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"actingcommand.ppocr-model-set.v1\0");
    for (label, hash) in [
        ("detector", detector_model_sha256),
        ("recognizer", recognizer_model_sha256),
        ("dictionary", dictionary_sha256),
    ] {
        hasher.update(label.as_bytes());
        hasher.update(b"\0");
        hasher.update(hash.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"classifier\0");
    hasher.update(classifier_model_sha256.unwrap_or("none").as_bytes());
    hasher.update(b"\0");
    Ok(lower_hex(&hasher.finalize()))
}

fn validate_sha256(module: &'static str, field: &str, hash: &str) -> VisionFfiResult<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ModelMismatch,
            module,
            format!("{field} must be exactly 64 lowercase hexadecimal characters"),
        ));
    }
    Ok(())
}

fn require_hash<'a>(
    module: &'static str,
    field: &str,
    hash: Option<&'a str>,
) -> VisionFfiResult<&'a str> {
    let hash = hash.ok_or_else(|| {
        VisionFfiError::fatal(module, format!("{field} is required for production use"))
    })?;
    validate_sha256(module, field, hash)?;
    Ok(hash)
}

fn verify_file_sha256(
    module: &'static str,
    field: &str,
    path: &Path,
    expected: &str,
) -> VisionFfiResult<()> {
    validate_sha256(module, field, expected)?;
    let mut file = fs::File::open(path).map_err(|err| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            module,
            format!(
                "failed to open {field} {} for hash verification: {err}",
                path.display()
            ),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|err| {
            VisionFfiError::fatal(
                module,
                format!(
                    "failed to read {field} {} for hash verification: {err}",
                    path.display()
                ),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = lower_hex(&hasher.finalize());
    if actual != expected {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            module,
            format!(
                "{field} SHA-256 mismatch at {}: expected={expected}, actual={actual}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
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
    fn artifact_manifest_accepts_cuda_ocr_and_declared_nn_route() {
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
            }"#,
        )
        .expect("manifest JSON");

        assert_eq!(
            manifest
                .require_fastdeploy_ppocr()
                .expect("ocr artifacts")
                .runtime_library_paths[0],
            PathBuf::from("external-tools/vision/fastdeploy/fastdeploy_ppocr_maa.dll")
        );
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
    fn production_ocr_rejects_non_cuda_execution_provider() {
        let mut artifacts = test_ocr_artifacts();
        artifacts.execution_provider = Some(OnnxExecutionProvider::Cpu);

        let err = artifacts
            .validate_ppocr_v6_cuda()
            .expect_err("CPU OCR route rejected");

        assert!(err.message().contains("must be cuda"));
        assert!(err.message().contains("fallback"));
    }

    #[test]
    fn production_ocr_verifies_component_hashes_before_use() {
        let root = std::env::temp_dir().join(format!(
            "actingcommand-vision-artifacts-hash-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory");
        let provider = root.join("provider.dll");
        let runtime = root.join("onnxruntime.dll");
        let detector = root.join("detector.onnx");
        let recognizer = root.join("recognizer.onnx");
        let dictionary = root.join("dictionary.txt");
        for (path, bytes) in [
            (&provider, b"provider".as_slice()),
            (&runtime, b"runtime".as_slice()),
            (&detector, b"detector".as_slice()),
            (&recognizer, b"recognizer".as_slice()),
            (&dictionary, b"dictionary".as_slice()),
        ] {
            fs::write(path, bytes).expect("fixture artifact");
        }
        let detector_hash = lower_hex(&Sha256::digest(b"detector"));
        let recognizer_hash = lower_hex(&Sha256::digest(b"recognizer"));
        let dictionary_hash = lower_hex(&Sha256::digest(b"dictionary"));
        let model_hash =
            ppocr_model_content_sha256(&detector_hash, &recognizer_hash, &dictionary_hash, None)
                .expect("model hash");
        let mut artifacts = test_ocr_artifacts();
        artifacts.provider_library_path = provider;
        artifacts.runtime_library_paths = vec![runtime];
        artifacts.detector_model_path = detector.clone();
        artifacts.recognizer_model_path = recognizer;
        artifacts.dictionary_path = dictionary;
        artifacts.detector_model_sha256 = Some(detector_hash);
        artifacts.recognizer_model_sha256 = Some(recognizer_hash);
        artifacts.dictionary_sha256 = Some(dictionary_hash);
        artifacts.model_sha256 = Some(model_hash);

        artifacts
            .validate_ppocr_v6_cuda_existing_files()
            .expect("matching model hashes");
        fs::write(&detector, b"changed").expect("change fixture");
        let err = artifacts
            .validate_ppocr_v6_cuda_existing_files()
            .expect_err("hash mismatch rejected");

        assert!(err.message().contains("SHA-256 mismatch"));
        fs::remove_dir_all(root).expect("remove fixture directory");
    }

    #[test]
    fn ocr_artifacts_accept_missing_runtime_libraries_for_legacy_manifests() {
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
                "onnxruntime": null
            }"#,
        )
        .expect("legacy manifest JSON");

        assert!(
            manifest
                .require_fastdeploy_ppocr()
                .expect("ocr artifacts")
                .runtime_library_paths
                .is_empty()
        );
    }

    #[test]
    fn nn_artifacts_reject_empty_labels() {
        let mut artifacts = test_nn_artifacts();
        artifacts.labels.clear();

        let err = artifacts.validate().expect_err("empty labels rejected");

        assert_eq!(err.module(), "onnxruntime");
    }

    #[test]
    fn nn_artifacts_accept_missing_runtime_library_for_legacy_manifests() {
        let manifest = VisionProviderArtifactManifest::from_json_slice(
            br#"{
                "schema_version": "actingcommand.vision_provider_artifacts.v0.1",
                "fastdeploy_ppocr": null,
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
        .expect("legacy manifest JSON");

        assert!(
            manifest
                .require_onnxruntime()
                .expect("nn artifacts")
                .runtime_library_path
                .is_none()
        );
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
        let detector_hash = "a".repeat(64);
        let recognizer_hash = "b".repeat(64);
        let dictionary_hash = "c".repeat(64);
        let model_hash =
            ppocr_model_content_sha256(&detector_hash, &recognizer_hash, &dictionary_hash, None)
                .expect("fixture model hash");
        FastDeployPpocrArtifacts {
            provider_library_path: "external-tools/vision/fastdeploy/ac_fastdeploy_ppocr.dll"
                .into(),
            runtime_library_paths: vec![
                "external-tools/vision/fastdeploy/fastdeploy_ppocr_maa.dll".into(),
            ],
            detector_model_path: "external-tools/vision/ppocr/det/inference.pdmodel".into(),
            recognizer_model_path: "external-tools/vision/ppocr/rec/inference.pdmodel".into(),
            dictionary_path: "external-tools/vision/ppocr/ppocr_keys_v1.txt".into(),
            classifier_model_path: None,
            model_ref: Some(PPOCR_V6_MEDIUM_MODEL_REF.to_string()),
            model_sha256: Some(model_hash),
            detector_model_sha256: Some(detector_hash),
            recognizer_model_sha256: Some(recognizer_hash),
            dictionary_sha256: Some(dictionary_hash),
            classifier_model_sha256: None,
            execution_provider: Some(OnnxExecutionProvider::Cuda),
            supported_languages: vec!["zh_cn".to_string(), "en".to_string()],
            default_timeout_ms: 1_000,
        }
    }

    pub(crate) fn test_nn_artifacts() -> OnnxRuntimeArtifacts {
        OnnxRuntimeArtifacts {
            provider_library_path: "external-tools/vision/onnxruntime/ac_onnxruntime.dll".into(),
            runtime_library_path: Some("external-tools/vision/onnxruntime/onnxruntime.dll".into()),
            model_path: "external-tools/vision/onnxruntime/models/page_classifier.onnx".into(),
            model_ref: Some("page-classifier".to_string()),
            model_sha256: Some("d".repeat(64)),
            labels: vec!["home".to_string(), "unknown".to_string()],
            labels_path: None,
            execution_provider: OnnxExecutionProvider::Cpu,
            default_timeout_ms: 1_000,
        }
    }
}
