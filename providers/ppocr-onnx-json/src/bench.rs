// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-0a (experiment branch `exp/407-gpu0-bench` only, never merged): what the
//! vision EP bench `ac-vision-ep-bench` needs from the engine to time a raw ONNX Runtime
//! session with exactly the engine's session options and the engine's input tensors. Nothing
//! here changes what the engine does; actingd never calls it.

use super::{
    ENGINE_MODULE, PpocrModelRole, ProviderSessionPlan, failure, frame_region_to_detection_tensor,
    frame_region_to_recognition_tensor, load_ort_session, runtime_facts,
    select_detection_input_shape, select_recognition_input_shape,
};
use actingcommand_vision_ffi::{
    CudaDeviceIdentity, OnnxExecutionProvider, PpocrCtcParams, VisionFfiError, VisionFfiErrorCode,
    VisionFfiResult, VisionFrameView, VisionRect, VisionRuntimeSpec,
};
use ort::session::Session;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The process's ONNX Runtime facts, as the engine established them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchRuntimeFacts {
    pub onnxruntime_library: PathBuf,
    pub onnxruntime_version: String,
    pub onnxruntime_sha256: String,
    pub executable_sha256: String,
    pub resolved_cuda_device: Option<CudaDeviceIdentity>,
    pub cuda_driver_version: Option<u32>,
    /// `OrtGetBuildInfoString`, as the engine's execution attestation records it.
    pub onnxruntime_build_info: String,
}

/// Initialises ONNX Runtime for this process exactly as the engine's first model load does
/// (the runtime-library closure, `ort::init_from`, the CUDA inventory for a CUDA runtime). A
/// later `PpocrCtcLoader::load_ocr` with the same runtime reuses it.
pub fn initialise_runtime(
    spec: &VisionRuntimeSpec,
    wait_deadline: Instant,
) -> VisionFfiResult<BenchRuntimeFacts> {
    let (facts, _waited) = runtime_facts(spec, wait_deadline)?;
    Ok(BenchRuntimeFacts {
        onnxruntime_library: facts.onnxruntime_library.clone(),
        onnxruntime_version: facts.onnxruntime_version.clone(),
        onnxruntime_sha256: facts.onnxruntime_sha256.clone(),
        executable_sha256: facts.executable_sha256.clone(),
        resolved_cuda_device: facts.resolved_cuda_device.clone(),
        cuda_driver_version: facts.cuda_driver_version,
        onnxruntime_build_info: ort::info().to_string(),
    })
}

/// The engine's session plan for a runtime: the same execution provider registration, CPU
/// fallback rule, DirectML adapter and tuning knobs an OCR model load of that runtime uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchSessionPlan(ProviderSessionPlan);

impl BenchSessionPlan {
    pub fn for_runtime(
        spec: &VisionRuntimeSpec,
        facts: &BenchRuntimeFacts,
    ) -> VisionFfiResult<Self> {
        let cuda_ordinal = match spec.execution_provider {
            OnnxExecutionProvider::Cuda => {
                let device = facts.resolved_cuda_device.as_ref().ok_or_else(|| {
                    VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::InvalidRequest,
                        ENGINE_MODULE,
                        "a CUDA bench session plan needs the runtime's resolved CUDA device",
                    )
                })?;
                Some(
                    i32::try_from(device.ordinal)
                        .map_err(|_| failure("the resolved CUDA ordinal exceeds the i32 range"))?,
                )
            }
            OnnxExecutionProvider::Cpu | OnnxExecutionProvider::Directml => None,
        };
        Ok(Self(
            ProviderSessionPlan::for_backend(spec.execution_provider, cuda_ordinal)
                .with_runtime_options(spec),
        ))
    }

    pub fn execution_provider(&self) -> OnnxExecutionProvider {
        self.0.resolved_execution_provider
    }

    pub fn registered_execution_providers(&self) -> &[OnnxExecutionProvider] {
        &self.0.registered_execution_providers
    }

    pub fn cpu_ep_registered(&self) -> bool {
        self.0.cpu_ep_registered
    }

    pub fn cpu_fallback_disabled(&self) -> bool {
        self.0.cpu_fallback_disabled
    }
}

/// Which PP-OCR model a raw session holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchModelRole {
    Detector,
    Recognizer,
}

/// Builds one session from `model` with the engine's own session builder for `plan` (no
/// node-placement diagnostic).
pub fn build_session(
    model: &[u8],
    path: &Path,
    plan: &BenchSessionPlan,
    role: BenchModelRole,
) -> VisionFfiResult<Session> {
    let role = match role {
        BenchModelRole::Detector => PpocrModelRole::Detector,
        BenchModelRole::Recognizer => PpocrModelRole::Recognizer,
    };
    let mut diagnostics = Vec::new();
    load_ort_session(model, path, &plan.0, role, None, &mut diagnostics).map_err(failure)
}

/// One input tensor exactly as the engine builds it for a request.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchInput {
    /// NCHW.
    pub shape: Vec<i64>,
    pub data: Vec<f32>,
}

/// The detector input the engine builds for `region` of `frame`.
pub fn detection_input(
    session: &Session,
    params: &PpocrCtcParams,
    frame: &VisionFrameView<'_>,
    region: VisionRect,
) -> VisionFfiResult<BenchInput> {
    let shape = select_detection_input_shape(session, region, params).map_err(failure)?;
    let data = frame_region_to_detection_tensor(frame, region, &shape, params).map_err(failure)?;
    Ok(BenchInput {
        shape: shape.to_ort_shape(),
        data,
    })
}

/// The recognizer input the engine builds for `region` of `frame` (a region read, or one
/// detected box of a detect read).
pub fn recognition_input(
    session: &Session,
    params: &PpocrCtcParams,
    frame: &VisionFrameView<'_>,
    region: VisionRect,
) -> VisionFfiResult<BenchInput> {
    let shape = select_recognition_input_shape(session, region, params).map_err(failure)?;
    let data =
        frame_region_to_recognition_tensor(frame, region, &shape, params).map_err(failure)?;
    Ok(BenchInput {
        shape: shape.to_ort_shape(),
        data,
    })
}
