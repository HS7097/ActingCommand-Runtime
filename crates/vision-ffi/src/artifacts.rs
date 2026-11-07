// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    MAX_DXGI_ADAPTERS, VisionFfiError, VisionFfiErrorCode, VisionFfiResult, is_dxgi_pci_identity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const OCR_EXECUTION_ATTESTATION_SCHEMA_VERSION: &str =
    "actingcommand.ocr_execution_attestation.v1";

const MAX_OCR_ID_BYTES: usize = 96;
/// The prefix of an adapter-issued invocation id; the ids keep it whatever the error module
/// token is called (the module tokens are snake_case since Workflow #378 A2-P).
const OCR_INVOCATION_ID_PREFIX: &str = "ocr-invocation-";
/// The prefix of an adapter-issued session id.
const OCR_SESSION_ID_PREFIX: &str = "ocr-session-";
const MAX_PROVIDER_IDENTITY_BYTES: usize = 256;
pub const MAX_CUDA_DEVICES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OcrInvocationId(String);

impl OcrInvocationId {
    pub(crate) fn from_sequence(sequence: u64) -> Self {
        Self(format!("{OCR_INVOCATION_ID_PREFIX}{sequence:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_opaque_id(
            "ocr_invocation",
            OCR_INVOCATION_ID_PREFIX,
            "invocation_id",
            &self.0,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OcrSessionId(String);

impl OcrSessionId {
    pub(crate) fn from_sequence(sequence: u64) -> Self {
        Self(format!("{OCR_SESSION_ID_PREFIX}{sequence:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_opaque_id("ocr_session", OCR_SESSION_ID_PREFIX, "session_id", &self.0)
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
                "ocr_device_selector",
                format!(
                    "CUDA ordinal {} exceeds the bounded device range 0..{}",
                    self.ordinal,
                    MAX_CUDA_DEVICES - 1
                ),
            ));
        }
        validate_provider_identity(
            "ocr_device_selector",
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
                "ocr_device_inventory",
                format!(
                    "CUDA inventory ordinal {} exceeds the bounded device range",
                    self.ordinal
                ),
            ));
        }
        validate_provider_identity(
            "ocr_device_inventory",
            "stable_identity",
            &self.stable_identity,
        )?;
        if let Some(pci_bus_id) = &self.pci_bus_id {
            validate_provider_identity("ocr_device_inventory", "pci_bus_id", pci_bus_id)?;
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
                "ocr_device_inventory",
                "CUDA driver version must be non-zero",
            ));
        }
        if self.devices.is_empty() || self.devices.len() > MAX_CUDA_DEVICES {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "ocr_device_inventory",
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
                    "ocr_device_inventory",
                    format!("CUDA inventory repeats ordinal {}", device.ordinal),
                ));
            }
            if !identities.insert(device.stable_identity.as_str()) {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidResponse,
                    "ocr_device_inventory",
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
                    "ocr_device_selector",
                    format!("CUDA ordinal {} is unavailable", selector.ordinal),
                )
            })?;
        if device.stable_identity != selector.expected_stable_identity {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "ocr_device_selector",
                format!(
                    "CUDA ordinal {} resolved to '{}' instead of expected '{}'",
                    selector.ordinal, device.stable_identity, selector.expected_stable_identity
                ),
            ));
        }
        Ok(device.clone())
    }
}

/// Workflow #407 GPU-1 A (design §1.2, §1.3): the DirectML adapter a configuration asks for, by
/// its stable identity `dxgi-pci:<vendor>:<device>:<subsys>:<rev>` (lowercase hexadecimal of
/// 4, 4, 8 and 2 digits). It never names a DXGI index or a LUID: both change between boots.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectmlDeviceSelector {
    pub expected_identity: String,
}

impl DirectmlDeviceSelector {
    pub fn validate(&self) -> VisionFfiResult<()> {
        if is_dxgi_pci_identity(&self.expected_identity) {
            Ok(())
        } else {
            Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr_device_selector",
                "DirectML expected_identity must be dxgi-pci:<vendor>:<device>:<subsys>:<rev> in lowercase hexadecimal of 4, 4, 8 and 2 digits",
            ))
        }
    }
}

/// Workflow #407 GPU-1 A: the adapter one DirectML session runs on: its identity and the DXGI
/// index it had when it was resolved, which ONNX Runtime's DirectML `device_id` names. The
/// adapter's LUID is an observation of the resolution (`ResolvedDirectmlAdapter`) and never
/// part of a session's identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectmlDeviceIdentity {
    pub dxgi_index: u32,
    pub identity: String,
}

impl DirectmlDeviceIdentity {
    pub fn validate(&self) -> VisionFfiResult<()> {
        if self.dxgi_index >= MAX_DXGI_ADAPTERS {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidResponse,
                "ocr_device_inventory",
                format!(
                    "DXGI adapter index {} exceeds the bounded adapter range 0..{}",
                    self.dxgi_index,
                    MAX_DXGI_ADAPTERS - 1
                ),
            ));
        }
        if !is_dxgi_pci_identity(&self.identity) {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidResponse,
                "ocr_device_inventory",
                "DirectML adapter identity must be dxgi-pci:<vendor>:<device>:<subsys>:<rev>",
            ));
        }
        Ok(())
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
    requested_directml_device: Option<DirectmlDeviceSelector>,
    resolved_directml_device: Option<DirectmlDeviceIdentity>,
    provider_options_sha256: String,
}

/// The facts an in-process engine binds one OCR session to. `engine_binding_sha256` is
/// recorded as the key's `provider_library_sha256` (Workflow #360: the engine binding digest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrSessionKeyParts {
    pub engine_binding_sha256: String,
    pub runtime_library_path: String,
    pub runtime_library_sha256: String,
    pub onnxruntime_version: String,
    pub model_ref: String,
    pub model_sha256: String,
    pub requested_backend: OnnxExecutionProvider,
    pub requested_cuda_device: Option<CudaDeviceSelector>,
    pub resolved_cuda_device: Option<CudaDeviceIdentity>,
    /// Workflow #407 GPU-1 A: the DirectML selector and resolved adapter, both only for
    /// `directml`.
    pub requested_directml_device: Option<DirectmlDeviceSelector>,
    pub resolved_directml_device: Option<DirectmlDeviceIdentity>,
}

impl OcrSessionKey {
    /// Builds and validates the key of one in-process OCR session; the provider options digest
    /// is derived from the execution provider choice exactly as for a manifest session.
    pub fn from_parts(parts: OcrSessionKeyParts) -> VisionFfiResult<Self> {
        let provider_options_sha256 = ppocr_provider_options_sha256(
            parts.requested_backend,
            parts.requested_cuda_device.as_ref(),
            parts.resolved_cuda_device.as_ref(),
            parts.requested_directml_device.as_ref(),
            parts.resolved_directml_device.as_ref(),
        );
        let key = Self {
            provider_library_sha256: parts.engine_binding_sha256,
            runtime_library_path: parts.runtime_library_path,
            runtime_library_sha256: parts.runtime_library_sha256,
            onnxruntime_version: parts.onnxruntime_version,
            model_ref: parts.model_ref,
            model_sha256: parts.model_sha256,
            requested_backend: parts.requested_backend,
            requested_cuda_device: parts.requested_cuda_device,
            resolved_cuda_device: parts.resolved_cuda_device,
            requested_directml_device: parts.requested_directml_device,
            resolved_directml_device: parts.resolved_directml_device,
            provider_options_sha256,
        };
        key.validate()?;
        Ok(key)
    }

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

    pub fn requested_directml_device(&self) -> Option<&DirectmlDeviceSelector> {
        self.requested_directml_device.as_ref()
    }

    pub fn resolved_directml_device(&self) -> Option<&DirectmlDeviceIdentity> {
        self.resolved_directml_device.as_ref()
    }

    pub fn provider_options_sha256(&self) -> &str {
        &self.provider_options_sha256
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_sha256(
            "ocr_session_key",
            "provider_library_sha256",
            &self.provider_library_sha256,
        )?;
        validate_provider_identity(
            "ocr_session_key",
            "runtime_library_path",
            &self.runtime_library_path,
        )?;
        validate_sha256(
            "ocr_session_key",
            "runtime_library_sha256",
            &self.runtime_library_sha256,
        )?;
        validate_provider_identity(
            "ocr_session_key",
            "onnxruntime_version",
            &self.onnxruntime_version,
        )?;
        validate_provider_identity("ocr_session_key", "model_ref", &self.model_ref)?;
        validate_sha256("ocr_session_key", "model_sha256", &self.model_sha256)?;
        validate_sha256(
            "ocr_session_key",
            "provider_options_sha256",
            &self.provider_options_sha256,
        )?;
        if self.requested_backend != OnnxExecutionProvider::Directml
            && (self.requested_directml_device.is_some() || self.resolved_directml_device.is_some())
        {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr_session_key",
                "only a DirectML session may contain a DirectML selector or resolved adapter",
            ));
        }
        match self.requested_backend {
            OnnxExecutionProvider::Cpu => {
                if self.requested_cuda_device.is_some() || self.resolved_cuda_device.is_some() {
                    return Err(VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr_session_key",
                        "CPU session must not contain CUDA selector or resolved CUDA identity",
                    ));
                }
            }
            OnnxExecutionProvider::Cuda => {
                let selector = self.requested_cuda_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr_session_key",
                        "CUDA session is missing the requested device selector",
                    )
                })?;
                let resolved = self.resolved_cuda_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr_session_key",
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
                        "ocr_session_key",
                        "CUDA session selector and resolved device identity do not match",
                    ));
                }
            }
            OnnxExecutionProvider::Directml => {
                if self.requested_cuda_device.is_some() || self.resolved_cuda_device.is_some() {
                    return Err(VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr_session_key",
                        "DirectML session must not contain CUDA selector or resolved CUDA identity",
                    ));
                }
                let selector = self.requested_directml_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr_session_key",
                        "DirectML session is missing the requested adapter selector",
                    )
                })?;
                let resolved = self.resolved_directml_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr_session_key",
                        "DirectML session is missing the resolved adapter",
                    )
                })?;
                selector.validate()?;
                resolved.validate()?;
                if selector.expected_identity != resolved.identity {
                    return Err(VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        "ocr_session_key",
                        "DirectML session selector and resolved adapter identity do not match",
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
    pub fn new(session_id: OcrSessionId, generation: u64, key: OcrSessionKey) -> Self {
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
                "ocr_session",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnnxExecutionProvider {
    Cpu,
    Cuda,
    /// Workflow #407 GPU-1: ONNX Runtime's DirectML execution provider on one DXGI adapter.
    Directml,
}

impl OnnxExecutionProvider {
    /// The provider's spelling: its configuration value and its token of the vocabulary
    /// `execution_provider`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
            Self::Directml => "directml",
        }
    }
}

impl actingcommand_contract::outcome::VocabularyToken for OnnxExecutionProvider {
    const VOCABULARY: &'static str = "execution_provider";

    fn token(self) -> &'static str {
        self.as_str()
    }
}

pub fn ppocr_model_content_sha256(
    detector_model_sha256: &str,
    recognizer_model_sha256: &str,
    dictionary_sha256: &str,
    classifier_model_sha256: Option<&str>,
) -> VisionFfiResult<String> {
    ppocr_model_set_sha256(
        Some(detector_model_sha256),
        recognizer_model_sha256,
        dictionary_sha256,
        classifier_model_sha256,
    )
}

/// The composite content identity of one PP-OCR model set. A set without a detector hashes
/// `none` in its place, as an absent classifier always did; with a detector the bytes equal
/// `ppocr_model_content_sha256`.
pub fn ppocr_model_set_sha256(
    detector_model_sha256: Option<&str>,
    recognizer_model_sha256: &str,
    dictionary_sha256: &str,
    classifier_model_sha256: Option<&str>,
) -> VisionFfiResult<String> {
    for (field, hash) in [
        ("detector_model_sha256", detector_model_sha256),
        ("recognizer_model_sha256", Some(recognizer_model_sha256)),
        ("dictionary_sha256", Some(dictionary_sha256)),
        ("classifier_model_sha256", classifier_model_sha256),
    ] {
        if let Some(hash) = hash {
            validate_sha256("fastdeploy_ppocr", field, hash)?;
        }
    }
    let mut hasher = Sha256::new();
    hasher.update(b"actingcommand.ppocr-model-set.v1\0");
    for (label, hash) in [
        ("detector", detector_model_sha256),
        ("recognizer", Some(recognizer_model_sha256)),
        ("dictionary", Some(dictionary_sha256)),
        ("classifier", classifier_model_sha256),
    ] {
        hasher.update(label.as_bytes());
        hasher.update(b"\0");
        hasher.update(hash.unwrap_or("none").as_bytes());
        hasher.update(b"\0");
    }
    Ok(lower_hex(&hasher.finalize()))
}

fn ppocr_provider_options_sha256(
    backend: OnnxExecutionProvider,
    selector: Option<&CudaDeviceSelector>,
    resolved: Option<&CudaDeviceIdentity>,
    directml_selector: Option<&DirectmlDeviceSelector>,
    directml_resolved: Option<&DirectmlDeviceIdentity>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"actingcommand.ppocr-provider-options.v1\0");
    let backend_token = match backend {
        OnnxExecutionProvider::Cpu => b"cpu".as_slice(),
        OnnxExecutionProvider::Cuda => b"cuda".as_slice(),
        OnnxExecutionProvider::Directml => {
            return directml_provider_options_sha256(hasher, directml_selector, directml_resolved);
        }
    };
    hasher.update(backend_token);
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

/// The DirectML branch of the provider options digest (Workflow #407 GPU-1 design §2.1): a
/// DirectML session lets ONNX Runtime place the nodes DirectML cannot run on the CPU, runs one
/// intra-op thread, turns the memory pattern and parallel execution off, and runs on the
/// resolved adapter's DXGI index. The adapter's LUID is no option and is never hashed.
fn directml_provider_options_sha256(
    mut hasher: Sha256,
    selector: Option<&DirectmlDeviceSelector>,
    resolved: Option<&DirectmlDeviceIdentity>,
) -> String {
    hasher.update(b"directml\0cpu_nodes\0allowed\0intra_threads\x001\0");
    hasher.update(b"memory_pattern\0false\0parallel_execution\0false\0");
    if let Some(selector) = selector {
        hasher.update(b"requested_identity\0");
        hasher.update(selector.expected_identity.as_bytes());
        hasher.update(b"\0");
    }
    if let Some(resolved) = resolved {
        hasher.update(b"resolved_dxgi_index\0");
        hasher.update(resolved.dxgi_index.to_string().as_bytes());
        hasher.update(b"\0resolved_identity\0");
        hasher.update(resolved.identity.as_bytes());
        hasher.update(b"\0");
    }
    lower_hex(&hasher.finalize())
}

fn validate_opaque_id(
    module: &'static str,
    prefix: &str,
    field: &str,
    value: &str,
) -> VisionFfiResult<()> {
    let suffix = value.strip_prefix(prefix).ok_or_else(|| {
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

fn lower_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
