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
    "actingcommand.vision_provider_artifacts.v0.3";
pub const TRANSITIONAL_VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION: &str =
    "actingcommand.vision_provider_artifacts.v0.2";
pub const LEGACY_VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION: &str =
    "actingcommand.vision_provider_artifacts.v0.1";
pub const PPOCR_V6_MEDIUM_MODEL_REF: &str = "PP-OCRv6_medium";
pub const OCR_PROVIDER_REQUEST_SCHEMA_VERSION: &str = "actingcommand.ocr_provider_request.v1";
pub const OCR_PROVIDER_RESPONSE_SCHEMA_VERSION: &str = "actingcommand.ocr_provider_response.v1";
pub const OCR_EXECUTION_ATTESTATION_SCHEMA_VERSION: &str =
    "actingcommand.ocr_execution_attestation.v1";

const MAX_OCR_ID_BYTES: usize = 96;
const MAX_PROVIDER_IDENTITY_BYTES: usize = 256;
const MAX_PROVIDER_BUILD_INFO_BYTES: usize = 4_096;
pub const MAX_CUDA_DEVICES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OcrInvocationId(String);

impl OcrInvocationId {
    pub(crate) fn from_sequence(sequence: u64) -> Self {
        Self(format!("ocr-invocation-{sequence:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_opaque_id("ocr-invocation", "invocation_id", &self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OcrSessionId(String);

impl OcrSessionId {
    pub(crate) fn from_sequence(sequence: u64) -> Self {
        Self(format!("ocr-session-{sequence:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_opaque_id("ocr-session", "session_id", &self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CudaDeviceSelector {
    pub ordinal: u32,
    pub expected_stable_identity: String,
}

impl CudaDeviceSelector {
    pub fn validate(&self) -> VisionFfiResult<()> {
        if usize::try_from(self.ordinal).map_or(true, |ordinal| ordinal >= MAX_CUDA_DEVICES) {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr-device-selector",
                format!(
                    "CUDA ordinal {} exceeds the bounded device range 0..{}",
                    self.ordinal,
                    MAX_CUDA_DEVICES - 1
                ),
            ));
        }
        validate_provider_identity(
            "ocr-device-selector",
            "expected_stable_identity",
            &self.expected_stable_identity,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CudaDeviceIdentity {
    pub ordinal: u32,
    pub stable_identity: String,
    pub pci_bus_id: Option<String>,
}

impl CudaDeviceIdentity {
    pub fn validate(&self) -> VisionFfiResult<()> {
        if usize::try_from(self.ordinal).map_or(true, |ordinal| ordinal >= MAX_CUDA_DEVICES) {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidResponse,
                "ocr-device-inventory",
                format!(
                    "CUDA inventory ordinal {} exceeds the bounded device range",
                    self.ordinal
                ),
            ));
        }
        validate_provider_identity(
            "ocr-device-inventory",
            "stable_identity",
            &self.stable_identity,
        )?;
        if let Some(pci_bus_id) = &self.pci_bus_id {
            validate_provider_identity("ocr-device-inventory", "pci_bus_id", pci_bus_id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CudaDeviceInventory {
    pub driver_version: u32,
    pub devices: Vec<CudaDeviceIdentity>,
}

impl CudaDeviceInventory {
    pub fn validate(&self) -> VisionFfiResult<()> {
        if self.driver_version == 0 {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "ocr-device-inventory",
                "CUDA driver version must be non-zero",
            ));
        }
        if self.devices.is_empty() || self.devices.len() > MAX_CUDA_DEVICES {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "ocr-device-inventory",
                format!("CUDA inventory must contain 1..={MAX_CUDA_DEVICES} usable devices"),
            ));
        }
        let mut ordinals = std::collections::HashSet::new();
        let mut identities = std::collections::HashSet::new();
        for device in &self.devices {
            device.validate()?;
            if !ordinals.insert(device.ordinal) {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidResponse,
                    "ocr-device-inventory",
                    format!("CUDA inventory repeats ordinal {}", device.ordinal),
                ));
            }
            if !identities.insert(device.stable_identity.as_str()) {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidResponse,
                    "ocr-device-inventory",
                    format!(
                        "CUDA inventory contains ambiguous stable identity '{}'",
                        device.stable_identity
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn resolve(&self, selector: &CudaDeviceSelector) -> VisionFfiResult<CudaDeviceIdentity> {
        self.validate()?;
        selector.validate()?;
        let device = self
            .devices
            .iter()
            .find(|device| device.ordinal == selector.ordinal)
            .ok_or_else(|| {
                VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::ProviderUnavailable,
                    "ocr-device-selector",
                    format!("CUDA ordinal {} is unavailable", selector.ordinal),
                )
            })?;
        if device.stable_identity != selector.expected_stable_identity {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "ocr-device-selector",
                format!(
                    "CUDA ordinal {} resolved to '{}' instead of expected '{}'",
                    selector.ordinal, device.stable_identity, selector.expected_stable_identity
                ),
            ));
        }
        Ok(device.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrSessionKey {
    provider_library_sha256: String,
    runtime_library_path: String,
    runtime_library_sha256: String,
    onnxruntime_version: String,
    model_ref: String,
    model_sha256: String,
    requested_backend: OnnxExecutionProvider,
    requested_cuda_device: Option<CudaDeviceSelector>,
    resolved_cuda_device: Option<CudaDeviceIdentity>,
    provider_options_sha256: String,
}

impl OcrSessionKey {
    pub fn provider_library_sha256(&self) -> &str {
        &self.provider_library_sha256
    }

    pub fn runtime_library_path(&self) -> &str {
        &self.runtime_library_path
    }

    pub fn runtime_library_sha256(&self) -> &str {
        &self.runtime_library_sha256
    }

    pub fn onnxruntime_version(&self) -> &str {
        &self.onnxruntime_version
    }

    pub fn model_ref(&self) -> &str {
        &self.model_ref
    }

    pub fn model_sha256(&self) -> &str {
        &self.model_sha256
    }

    pub fn requested_backend(&self) -> OnnxExecutionProvider {
        self.requested_backend
    }

    pub fn requested_cuda_device(&self) -> Option<&CudaDeviceSelector> {
        self.requested_cuda_device.as_ref()
    }

    pub fn resolved_cuda_device(&self) -> Option<&CudaDeviceIdentity> {
        self.resolved_cuda_device.as_ref()
    }

    pub fn provider_options_sha256(&self) -> &str {
        &self.provider_options_sha256
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_sha256(
            "ocr-session-key",
            "provider_library_sha256",
            &self.provider_library_sha256,
        )?;
        validate_provider_identity(
            "ocr-session-key",
            "runtime_library_path",
            &self.runtime_library_path,
        )?;
        validate_sha256(
            "ocr-session-key",
            "runtime_library_sha256",
            &self.runtime_library_sha256,
        )?;
        validate_provider_identity(
            "ocr-session-key",
            "onnxruntime_version",
            &self.onnxruntime_version,
        )?;
        validate_provider_identity("ocr-session-key", "model_ref", &self.model_ref)?;
        if self.model_ref != PPOCR_V6_MEDIUM_MODEL_REF {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr-session-key",
                format!("OCR session model_ref must be '{PPOCR_V6_MEDIUM_MODEL_REF}'"),
            ));
        }
        validate_sha256("ocr-session-key", "model_sha256", &self.model_sha256)?;
        validate_sha256(
            "ocr-session-key",
            "provider_options_sha256",
            &self.provider_options_sha256,
        )?;
        match self.requested_backend {
            OnnxExecutionProvider::Cpu => {
                if self.requested_cuda_device.is_some() || self.resolved_cuda_device.is_some() {
                    return Err(VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr-session-key",
                        "CPU session must not contain CUDA selector or resolved CUDA identity",
                    ));
                }
            }
            OnnxExecutionProvider::Cuda => {
                let selector = self.requested_cuda_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr-session-key",
                        "CUDA session is missing the requested device selector",
                    )
                })?;
                let resolved = self.resolved_cuda_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr-session-key",
                        "CUDA session is missing the resolved device identity",
                    )
                })?;
                selector.validate()?;
                resolved.validate()?;
                if selector.ordinal != resolved.ordinal
                    || selector.expected_stable_identity != resolved.stable_identity
                {
                    return Err(VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr-session-key",
                        "CUDA session selector and resolved device identity do not match",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrSessionBinding {
    session_id: OcrSessionId,
    generation: u64,
    key: OcrSessionKey,
}

impl OcrSessionBinding {
    pub(crate) fn new(session_id: OcrSessionId, generation: u64, key: OcrSessionKey) -> Self {
        Self {
            session_id,
            generation,
            key,
        }
    }

    pub fn session_id(&self) -> &OcrSessionId {
        &self.session_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn key(&self) -> &OcrSessionKey {
        &self.key
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        self.session_id.validate()?;
        if self.generation == 0 {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr-session",
                "OCR session generation must be non-zero",
            ));
        }
        self.key.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OcrSessionIdentity {
    pub session_id: OcrSessionId,
    pub generation: u64,
}

impl From<&OcrSessionBinding> for OcrSessionIdentity {
    fn from(binding: &OcrSessionBinding) -> Self {
        Self {
            session_id: binding.session_id.clone(),
            generation: binding.generation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFallbackPolicy {
    Forbidden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrProviderBuildIdentity {
    pub implementation: String,
    pub crate_version: String,
    pub build_git_sha: Option<String>,
    pub binary_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrRuntimeBuildIdentity {
    pub onnxruntime_version: String,
    pub onnxruntime_build_info: String,
    pub cuda_driver_version: Option<u32>,
    pub cuda_runtime_version: Option<String>,
    pub cudnn_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrExecutionAttestation {
    pub schema_version: String,
    pub invocation_id: OcrInvocationId,
    pub session: OcrSessionBinding,
    pub resolved_execution_provider: OnnxExecutionProvider,
    pub provider: OcrProviderBuildIdentity,
    pub runtime: OcrRuntimeBuildIdentity,
    pub registered_execution_providers: Vec<OnnxExecutionProvider>,
    pub cpu_ep_registered: bool,
    pub cpu_fallback_disabled: bool,
    pub fallback_policy: OcrFallbackPolicy,
    pub fallback_observed: Option<bool>,
    pub complete: bool,
}

impl OcrExecutionAttestation {
    pub fn validate_against(
        &self,
        invocation_id: &OcrInvocationId,
        session: &OcrSessionBinding,
    ) -> VisionFfiResult<()> {
        if self.schema_version != OCR_EXECUTION_ATTESTATION_SCHEMA_VERSION {
            return Err(invalid_attestation(format!(
                "unsupported attestation schema_version '{}'",
                self.schema_version
            )));
        }
        if !self.complete {
            return Err(invalid_attestation(
                "OCR execution attestation is incomplete",
            ));
        }
        self.invocation_id.validate().map_err(|err| {
            invalid_attestation(format!("invalid invocation identity: {}", err.message()))
        })?;
        self.session.validate().map_err(|err| {
            invalid_attestation(format!("invalid session binding: {}", err.message()))
        })?;
        if &self.invocation_id != invocation_id || &self.session != session {
            return Err(invalid_attestation(
                "OCR execution attestation does not match invocation/session binding",
            ));
        }
        if self.resolved_execution_provider != session.key.requested_backend {
            return Err(invalid_attestation(
                "resolved OCR execution provider does not match requested backend",
            ));
        }
        validate_provider_identity(
            "ocr-attestation",
            "provider.implementation",
            &self.provider.implementation,
        )
        .map_err(|err| invalid_attestation(err.message()))?;
        validate_provider_identity(
            "ocr-attestation",
            "provider.crate_version",
            &self.provider.crate_version,
        )
        .map_err(|err| invalid_attestation(err.message()))?;
        if self.provider.implementation != "actingcommand-ppocr-onnx-json" {
            return Err(invalid_attestation(format!(
                "unexpected OCR provider implementation '{}'",
                self.provider.implementation
            )));
        }
        if self
            .provider
            .build_git_sha
            .as_ref()
            .is_some_and(|build_git_sha| {
                build_git_sha.len() != 40
                    || !build_git_sha
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(invalid_attestation(
                "provider.build_git_sha must be absent or exactly 40 lowercase hexadecimal characters",
            ));
        }
        validate_sha256(
            "ocr-attestation",
            "provider.binary_sha256",
            &self.provider.binary_sha256,
        )
        .map_err(|err| invalid_attestation(err.message()))?;
        if self.provider.binary_sha256 != session.key.provider_library_sha256 {
            return Err(invalid_attestation(
                "provider binary identity does not match the immutable session key",
            ));
        }
        validate_provider_identity(
            "ocr-attestation",
            "runtime.onnxruntime_version",
            &self.runtime.onnxruntime_version,
        )
        .map_err(|err| invalid_attestation(err.message()))?;
        validate_provider_build_info(&self.runtime.onnxruntime_build_info)?;
        if self.runtime.onnxruntime_version != session.key.onnxruntime_version {
            return Err(invalid_attestation(
                "ONNX Runtime version does not match the immutable session key",
            ));
        }
        for (field, value) in [
            (
                "runtime.cuda_runtime_version",
                self.runtime.cuda_runtime_version.as_deref(),
            ),
            (
                "runtime.cudnn_version",
                self.runtime.cudnn_version.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                validate_provider_identity("ocr-attestation", field, value)
                    .map_err(|err| invalid_attestation(err.message()))?;
            }
        }
        if self.fallback_policy != OcrFallbackPolicy::Forbidden {
            return Err(invalid_attestation(
                "OCR fallback policy must be explicitly forbidden",
            ));
        }
        match self.resolved_execution_provider {
            OnnxExecutionProvider::Cpu => {
                if self.runtime.cuda_driver_version.is_some()
                    || self.runtime.cuda_runtime_version.is_some()
                    || self.runtime.cudnn_version.is_some()
                    || self.registered_execution_providers != [OnnxExecutionProvider::Cpu]
                    || !self.cpu_ep_registered
                    || self.cpu_fallback_disabled
                {
                    return Err(invalid_attestation(
                        "CPU OCR attestation must describe one CPU-only session without CUDA evidence",
                    ));
                }
            }
            OnnxExecutionProvider::Cuda => {
                if self.runtime.cuda_driver_version == Some(0)
                    || self.runtime.cuda_driver_version.is_none()
                    || self.registered_execution_providers != [OnnxExecutionProvider::Cuda]
                    || self.cpu_ep_registered
                    || !self.cpu_fallback_disabled
                {
                    return Err(invalid_attestation(
                        "CUDA OCR attestation must describe one CUDA session with CPU fallback disabled",
                    ));
                }
            }
        }
        if self.fallback_observed.is_some() {
            return Err(invalid_attestation(
                "current provider API cannot observe fallback attempts; fallback_observed must be null",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FastDeployPpocrInvokeResponse {
    pub schema_version: String,
    pub invocation_id: OcrInvocationId,
    pub session_id: OcrSessionId,
    pub session_generation: u64,
    pub result: crate::OcrInferenceResult,
    pub attestation: OcrExecutionAttestation,
}

impl FastDeployPpocrInvokeResponse {
    pub fn validate_against(
        &self,
        invocation_id: &OcrInvocationId,
        session: &OcrSessionBinding,
    ) -> VisionFfiResult<()> {
        if self.schema_version != OCR_PROVIDER_RESPONSE_SCHEMA_VERSION {
            return Err(invalid_attestation(format!(
                "unsupported OCR response schema_version '{}'",
                self.schema_version
            )));
        }
        if &self.invocation_id != invocation_id
            || &self.session_id != session.session_id()
            || self.session_generation != session.generation()
        {
            return Err(invalid_attestation(
                "OCR response identity does not match the active invocation/session",
            ));
        }
        self.attestation.validate_against(invocation_id, session)
    }
}

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
            TRANSITIONAL_VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION => {
                if let Some(artifacts) = &self.fastdeploy_ppocr {
                    artifacts.validate_ppocr_v6_cuda_legacy()?;
                }
                if let Some(artifacts) = &self.onnxruntime {
                    artifacts.validate_production_model()?;
                }
            }
            VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION => {
                if let Some(artifacts) = &self.fastdeploy_ppocr {
                    artifacts.validate_ppocr_v6_execution()?;
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
                artifacts.validate_ppocr_v6_execution_existing_files()?;
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
        artifacts.validate_ppocr_v6_execution()?;
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
    pub provider_library_sha256: Option<String>,
    #[serde(default)]
    pub runtime_library_paths: Vec<PathBuf>,
    #[serde(default)]
    pub runtime_library_path: Option<PathBuf>,
    #[serde(default)]
    pub runtime_library_sha256: Option<String>,
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
    #[serde(default)]
    pub cuda_device: Option<CudaDeviceSelector>,
    #[serde(default)]
    pub strict_no_fallback: Option<bool>,
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
        if let Some(path) = &self.runtime_library_path {
            validate_required_path("fastdeploy-ppocr", "runtime_library_path", path)?;
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
            (
                "provider_library_sha256",
                self.provider_library_sha256.as_deref(),
            ),
            (
                "runtime_library_sha256",
                self.runtime_library_sha256.as_deref(),
            ),
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
        if let Some(selector) = &self.cuda_device {
            selector.validate()?;
        }
        Ok(())
    }

    fn validate_ppocr_v6_model(&self) -> VisionFfiResult<()> {
        self.validate()?;
        if self.model_ref.as_deref() != Some(PPOCR_V6_MEDIUM_MODEL_REF) {
            return Err(VisionFfiError::fatal(
                "fastdeploy-ppocr",
                format!("production OCR model_ref must be '{PPOCR_V6_MEDIUM_MODEL_REF}'"),
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

    fn validate_ppocr_v6_cuda_legacy(&self) -> VisionFfiResult<()> {
        self.validate_ppocr_v6_model()?;
        if self.execution_provider != Some(OnnxExecutionProvider::Cuda) {
            return Err(VisionFfiError::fatal(
                "fastdeploy-ppocr",
                "transitional production OCR execution_provider must be cuda",
            ));
        }
        Ok(())
    }

    pub fn validate_ppocr_v6_execution(&self) -> VisionFfiResult<()> {
        self.validate_ppocr_v6_model()?;
        require_hash(
            "fastdeploy-ppocr",
            "provider_library_sha256",
            self.provider_library_sha256.as_deref(),
        )?;
        require_hash(
            "fastdeploy-ppocr",
            "runtime_library_sha256",
            self.runtime_library_sha256.as_deref(),
        )?;
        if self.strict_no_fallback != Some(true) {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "fastdeploy-ppocr",
                "production OCR strict_no_fallback must be explicitly true",
            ));
        }
        match self.execution_provider {
            Some(OnnxExecutionProvider::Cpu) => {
                if self.cuda_device.is_some() {
                    return Err(VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "fastdeploy-ppocr",
                        "CPU OCR configuration must not include a CUDA device selector",
                    ));
                }
            }
            Some(OnnxExecutionProvider::Cuda) => {
                self.cuda_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "fastdeploy-ppocr",
                        "CUDA OCR configuration requires an explicit ordinal and stable identity",
                    )
                })?;
            }
            None => {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidRequest,
                    "fastdeploy-ppocr",
                    "production OCR execution_provider must be explicitly cpu or cuda",
                ));
            }
        }
        self.onnxruntime_library_path()?;
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

    pub fn validate_ppocr_v6_execution_existing_files(&self) -> VisionFfiResult<()> {
        self.validate_ppocr_v6_execution()?;
        self.validate_existing_files()?;
        let provider_hash = require_hash(
            "fastdeploy-ppocr",
            "provider_library_sha256",
            self.provider_library_sha256.as_deref(),
        )?;
        verify_file_sha256(
            "fastdeploy-ppocr",
            "provider_library_path",
            &self.provider_library_path,
            provider_hash,
        )?;
        let runtime_hash = require_hash(
            "fastdeploy-ppocr",
            "runtime_library_sha256",
            self.runtime_library_sha256.as_deref(),
        )?;
        verify_file_sha256(
            "fastdeploy-ppocr",
            "runtime_library_path",
            self.onnxruntime_library_path()?,
            runtime_hash,
        )?;
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
        self.validate_ppocr_v6_execution()?;
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

    pub fn onnxruntime_library_path(&self) -> VisionFfiResult<&Path> {
        let selected = self.runtime_library_path.as_deref().ok_or_else(|| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "fastdeploy-ppocr",
                "production OCR runtime_library_path must identify the exact ONNX Runtime library",
            )
        })?;
        let occurrences = self
            .runtime_library_paths
            .iter()
            .filter(|path| path.as_path() == selected)
            .count();
        if occurrences != 1 {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "fastdeploy-ppocr",
                "runtime_library_path must occur exactly once in runtime_library_paths",
            ));
        }
        Ok(selected)
    }

    pub fn production_session_key(
        &self,
        resolved_cuda_device: Option<CudaDeviceIdentity>,
        onnxruntime_version: impl Into<String>,
    ) -> VisionFfiResult<OcrSessionKey> {
        self.validate_ppocr_v6_execution()?;
        let requested_backend = self.execution_provider.ok_or_else(|| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "fastdeploy-ppocr",
                "validated execution_provider is missing",
            )
        })?;
        match requested_backend {
            OnnxExecutionProvider::Cpu if resolved_cuda_device.is_some() => {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidRequest,
                    "fastdeploy-ppocr",
                    "CPU OCR session must not resolve a CUDA device",
                ));
            }
            OnnxExecutionProvider::Cuda => {
                let selector = self.cuda_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "fastdeploy-ppocr",
                        "validated CUDA selector is missing",
                    )
                })?;
                let resolved = resolved_cuda_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::ProviderUnavailable,
                        "fastdeploy-ppocr",
                        "CUDA OCR session is missing a resolved device identity",
                    )
                })?;
                if selector.ordinal != resolved.ordinal
                    || selector.expected_stable_identity != resolved.stable_identity
                {
                    return Err(VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::ProviderUnavailable,
                        "fastdeploy-ppocr",
                        "resolved CUDA identity does not match the configured selector",
                    ));
                }
            }
            OnnxExecutionProvider::Cpu => {}
        }
        let provider_library_sha256 = require_hash(
            "fastdeploy-ppocr",
            "provider_library_sha256",
            self.provider_library_sha256.as_deref(),
        )?
        .to_string();
        let runtime_library = self.onnxruntime_library_path()?;
        let runtime_library_path = runtime_library.to_string_lossy().into_owned();
        validate_provider_identity(
            "fastdeploy-ppocr",
            "runtime_library_path",
            &runtime_library_path,
        )?;
        let runtime_library_sha256 = require_hash(
            "fastdeploy-ppocr",
            "runtime_library_sha256",
            self.runtime_library_sha256.as_deref(),
        )?
        .to_string();
        let onnxruntime_version = onnxruntime_version.into();
        validate_provider_identity(
            "fastdeploy-ppocr",
            "onnxruntime_version",
            &onnxruntime_version,
        )?;
        let model_sha256 = require_hash(
            "fastdeploy-ppocr",
            "model_sha256",
            self.model_sha256.as_deref(),
        )?
        .to_string();
        let model_ref = self
            .model_ref
            .as_deref()
            .ok_or_else(|| {
                VisionFfiError::fatal(
                    "fastdeploy-ppocr",
                    "validated production OCR model_ref is missing",
                )
            })?
            .to_string();
        let provider_options_sha256 = ppocr_provider_options_sha256(
            requested_backend,
            self.cuda_device.as_ref(),
            resolved_cuda_device.as_ref(),
        );
        let key = OcrSessionKey {
            provider_library_sha256,
            runtime_library_path,
            runtime_library_sha256,
            onnxruntime_version,
            model_ref,
            model_sha256,
            requested_backend,
            requested_cuda_device: self.cuda_device.clone(),
            resolved_cuda_device,
            provider_options_sha256,
        };
        key.validate()?;
        Ok(key)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnnxExecutionProvider {
    Cpu,
    Cuda,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FastDeployPpocrInvokeRequest {
    pub schema_version: String,
    pub invocation_id: OcrInvocationId,
    pub session: OcrSessionBinding,
    pub request: OcrInferenceRequest,
    pub artifacts: FastDeployPpocrArtifacts,
}

impl FastDeployPpocrInvokeRequest {
    pub(crate) fn new(
        invocation_id: OcrInvocationId,
        session: OcrSessionBinding,
        request: OcrInferenceRequest,
        artifacts: FastDeployPpocrArtifacts,
    ) -> Self {
        Self {
            schema_version: OCR_PROVIDER_REQUEST_SCHEMA_VERSION.to_string(),
            invocation_id,
            session,
            request,
            artifacts,
        }
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        if self.schema_version != OCR_PROVIDER_REQUEST_SCHEMA_VERSION {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr-provider-request",
                format!(
                    "unsupported OCR request schema_version '{}'",
                    self.schema_version
                ),
            ));
        }
        self.invocation_id.validate()?;
        self.session.validate()?;
        self.request.validate()?;
        self.artifacts.validate_ppocr_v6_execution()?;
        let expected_key = self.artifacts.production_session_key(
            self.session.key().resolved_cuda_device().cloned(),
            self.session.key().onnxruntime_version(),
        )?;
        if &expected_key != self.session.key() {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr-provider-request",
                "OCR request artifacts do not match the immutable session key",
            ));
        }
        Ok(())
    }
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

fn ppocr_provider_options_sha256(
    backend: OnnxExecutionProvider,
    selector: Option<&CudaDeviceSelector>,
    resolved: Option<&CudaDeviceIdentity>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"actingcommand.ppocr-provider-options.v1\0");
    hasher.update(match backend {
        OnnxExecutionProvider::Cpu => b"cpu".as_slice(),
        OnnxExecutionProvider::Cuda => b"cuda".as_slice(),
    });
    hasher.update(b"\0strict_no_fallback\0true\0intra_threads\0");
    hasher.update(b"1\0");
    if let Some(selector) = selector {
        hasher.update(b"requested_ordinal\0");
        hasher.update(selector.ordinal.to_string().as_bytes());
        hasher.update(b"\0requested_identity\0");
        hasher.update(selector.expected_stable_identity.as_bytes());
        hasher.update(b"\0");
    }
    if let Some(resolved) = resolved {
        hasher.update(b"resolved_ordinal\0");
        hasher.update(resolved.ordinal.to_string().as_bytes());
        hasher.update(b"\0resolved_identity\0");
        hasher.update(resolved.stable_identity.as_bytes());
        hasher.update(b"\0resolved_pci\0");
        hasher.update(resolved.pci_bus_id.as_deref().unwrap_or("none").as_bytes());
        hasher.update(b"\0");
    }
    lower_hex(&hasher.finalize())
}

fn validate_opaque_id(module: &'static str, field: &str, value: &str) -> VisionFfiResult<()> {
    let prefix = format!("{module}-");
    let suffix = value.strip_prefix(&prefix).ok_or_else(|| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidRequest,
            module,
            format!("{field} has an invalid opaque identity prefix"),
        )
    })?;
    if value.len() > MAX_OCR_ID_BYTES
        || suffix.len() != 16
        || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidRequest,
            module,
            format!("{field} must be a bounded adapter-issued opaque identity"),
        ));
    }
    Ok(())
}

fn validate_provider_identity(
    module: &'static str,
    field: &str,
    value: &str,
) -> VisionFfiResult<()> {
    if value.is_empty()
        || value.len() > MAX_PROVIDER_IDENTITY_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            module,
            format!("{field} must be a bounded non-blank identity"),
        ));
    }
    Ok(())
}

fn validate_provider_build_info(value: &str) -> VisionFfiResult<()> {
    if value.is_empty()
        || value.len() > MAX_PROVIDER_BUILD_INFO_BYTES
        || value.trim() != value
        || value.chars().any(|character| character == '\0')
    {
        return Err(invalid_attestation(
            "runtime.onnxruntime_build_info must be bounded, non-blank, and NUL-free",
        ));
    }
    Ok(())
}

fn invalid_attestation(message: impl Into<String>) -> VisionFfiError {
    VisionFfiError::fatal_with_code(
        VisionFfiErrorCode::InvalidResponse,
        "ocr-attestation",
        message,
    )
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
    fn production_ocr_accepts_explicit_cpu_without_cuda_selector() {
        let mut artifacts = test_ocr_artifacts();
        artifacts.execution_provider = Some(OnnxExecutionProvider::Cpu);
        artifacts.cuda_device = None;

        artifacts
            .validate_ppocr_v6_execution()
            .expect("explicit CPU OCR route");
        let key = artifacts
            .production_session_key(None, "1.24.0-test")
            .expect("CPU session key");

        assert_eq!(key.requested_backend(), OnnxExecutionProvider::Cpu);
        assert!(key.requested_cuda_device().is_none());
        assert!(key.resolved_cuda_device().is_none());
    }

    #[test]
    fn production_ocr_rejects_cpu_with_cuda_selector() {
        let mut artifacts = test_ocr_artifacts();
        artifacts.execution_provider = Some(OnnxExecutionProvider::Cpu);

        let err = artifacts
            .validate_ppocr_v6_execution()
            .expect_err("CPU plus CUDA selector rejected");

        assert!(err.message().contains("must not include a CUDA"));
    }

    #[test]
    fn production_ocr_rejects_cuda_without_selector_or_no_fallback() {
        let mut artifacts = test_ocr_artifacts();
        artifacts.cuda_device = None;

        let err = artifacts
            .validate_ppocr_v6_execution()
            .expect_err("missing CUDA selector rejected");
        assert!(err.message().contains("requires an explicit ordinal"));

        artifacts.cuda_device = Some(test_cuda_selector());
        artifacts.strict_no_fallback = Some(false);
        let err = artifacts
            .validate_ppocr_v6_execution()
            .expect_err("fallback-enabled OCR rejected");
        assert!(err.message().contains("strict_no_fallback"));
    }

    #[test]
    fn production_ocr_requires_one_explicit_onnxruntime_library_identity() {
        let mut artifacts = test_ocr_artifacts();
        artifacts
            .runtime_library_paths
            .push("external-tools/vision/fastdeploy/onnxruntime_providers_shared.dll".into());

        artifacts
            .validate_ppocr_v6_execution()
            .expect("supporting runtime libraries do not change the explicit identity");

        artifacts.runtime_library_path =
            Some("external-tools/vision/fastdeploy/missing.dll".into());
        let err = artifacts
            .validate_ppocr_v6_execution()
            .expect_err("unlisted explicit runtime identity rejected");

        assert_eq!(err.code(), VisionFfiErrorCode::InvalidRequest);
        assert!(err.message().contains("occur exactly once"));
    }

    #[test]
    fn cuda_selector_and_inventory_reject_invalid_unavailable_and_ambiguous_devices() {
        let invalid = CudaDeviceSelector {
            ordinal: MAX_CUDA_DEVICES as u32,
            expected_stable_identity: "cuda-uuid:ffffffffffffffffffffffffffffffff".to_string(),
        };
        assert_eq!(
            invalid.validate().expect_err("bounded ordinal").code(),
            VisionFfiErrorCode::InvalidRequest
        );

        let inventory = CudaDeviceInventory {
            driver_version: 12_800,
            devices: vec![CudaDeviceIdentity {
                ordinal: 0,
                stable_identity: "cuda-uuid:00000000000000000000000000000000".to_string(),
                pci_bus_id: Some("0000:01:00.0".to_string()),
            }],
        };
        let unavailable = CudaDeviceSelector {
            ordinal: 1,
            expected_stable_identity: "cuda-uuid:11111111111111111111111111111111".to_string(),
        };
        assert_eq!(
            inventory
                .resolve(&unavailable)
                .expect_err("unavailable ordinal")
                .code(),
            VisionFfiErrorCode::ProviderUnavailable
        );

        let ambiguous = CudaDeviceInventory {
            driver_version: 12_800,
            devices: vec![
                CudaDeviceIdentity {
                    ordinal: 0,
                    stable_identity: "cuda-uuid:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                    pci_bus_id: Some("0000:01:00.0".to_string()),
                },
                CudaDeviceIdentity {
                    ordinal: 1,
                    stable_identity: "cuda-uuid:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                    pci_bus_id: Some("0000:02:00.0".to_string()),
                },
            ],
        };
        assert_eq!(
            ambiguous
                .validate()
                .expect_err("ambiguous stable identity")
                .code(),
            VisionFfiErrorCode::InvalidResponse
        );
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
        artifacts.provider_library_sha256 = Some(lower_hex(&Sha256::digest(b"provider")));
        artifacts.runtime_library_paths = vec![runtime.clone()];
        artifacts.runtime_library_path = Some(runtime);
        artifacts.runtime_library_sha256 = Some(lower_hex(&Sha256::digest(b"runtime")));
        artifacts.detector_model_path = detector.clone();
        artifacts.recognizer_model_path = recognizer;
        artifacts.dictionary_path = dictionary;
        artifacts.detector_model_sha256 = Some(detector_hash);
        artifacts.recognizer_model_sha256 = Some(recognizer_hash);
        artifacts.dictionary_sha256 = Some(dictionary_hash);
        artifacts.model_sha256 = Some(model_hash);

        artifacts
            .validate_ppocr_v6_execution_existing_files()
            .expect("matching model hashes");
        fs::write(&detector, b"changed").expect("change fixture");
        let err = artifacts
            .validate_ppocr_v6_execution_existing_files()
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
            provider_library_sha256: Some("e".repeat(64)),
            runtime_library_paths: vec!["external-tools/vision/fastdeploy/onnxruntime.dll".into()],
            runtime_library_path: Some("external-tools/vision/fastdeploy/onnxruntime.dll".into()),
            runtime_library_sha256: Some("d".repeat(64)),
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
            cuda_device: Some(test_cuda_selector()),
            strict_no_fallback: Some(true),
            supported_languages: vec!["zh_cn".to_string(), "en".to_string()],
            default_timeout_ms: 1_000,
        }
    }

    fn test_cuda_selector() -> CudaDeviceSelector {
        CudaDeviceSelector {
            ordinal: 1,
            expected_stable_identity: "cuda-uuid:11111111111111111111111111111111".to_string(),
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
