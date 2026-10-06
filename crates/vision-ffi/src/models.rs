// SPDX-License-Identifier: AGPL-3.0-only

//! What the Runtime hands an in-process vision engine so that it can build one model on
//! first use, and the loader contract that engine implements (Workflow #360). Nothing here
//! loads a library or runs a model; the hashing helpers read a file only when asked.

use crate::{
    CudaDeviceSelector, OcrEngine, OcrSessionId, OnnxExecutionProvider, VisionFfiError,
    VisionFfiErrorCode, VisionFfiResult,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Domain separator of the engine binding digest recorded as an OCR execution's
/// `provider_binary_sha256`.
pub const OCR_ENGINE_BINDING_SCHEMA: &str = "actingcommand.ocr-engine-binding.v1";

/// The ONNX Runtime an engine initialises once per process, on its first model build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionRuntimeSpec {
    /// The exact ONNX Runtime library the engine initialises.
    pub onnxruntime_library: PathBuf,
    /// The libraries loaded first, in order, through the process runtime-library closure;
    /// it contains `onnxruntime_library` exactly once.
    pub runtime_library_closure: Vec<PathBuf>,
    /// The SHA-256 the ONNX Runtime library must have, when the configuration pins one.
    pub expected_onnxruntime_sha256: Option<String>,
    pub execution_provider: OnnxExecutionProvider,
    pub cuda_device: Option<CudaDeviceSelector>,
}

/// The files of one OCR model, named by its `model_ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrModelSpec {
    pub model_ref: String,
    pub detector_path: PathBuf,
    pub recognizer_path: PathBuf,
    pub dictionary_path: PathBuf,
}

/// The outcome of one OCR model load.
pub enum OcrModelLoad {
    /// The content identity equals the requested one and the engine is ready.
    Loaded {
        engine: Box<dyn OcrEngine + Send>,
        model_sha256: String,
        /// How long the load waited for locks held by other requests (another request's
        /// ONNX Runtime initialisation); the request's budget pays for it, unlike the load.
        waited_on_others: Duration,
    },
    /// The files hash to a different content identity; no session was built.
    Mismatch { model_sha256: String },
}

/// Builds the engine of one model on first use. Implemented by the in-process engine and
/// injected by the composition root, so the Runtime never links ONNX Runtime itself.
pub trait VisionModelLoader: Send + Sync {
    /// Reads the model's files once, computes their content identity and, when it equals
    /// `expected_model_sha256`, builds the engine whose executions carry `session_id`.
    /// Waiting for another request's runtime initialisation ends at `wait_deadline`.
    fn load_ocr(
        &self,
        model: &OcrModelSpec,
        expected_model_sha256: &str,
        session_id: &OcrSessionId,
        wait_deadline: Instant,
    ) -> VisionFfiResult<OcrModelLoad>;
}

/// The engine binding digest: the running executable, the ONNX Runtime library and the
/// model description (`none` when the model has none) of one OCR session.
pub fn ocr_engine_binding_sha256(
    executable_sha256: &str,
    onnxruntime_sha256: &str,
    description_sha256: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(OCR_ENGINE_BINDING_SCHEMA.as_bytes());
    hasher.update(b"\0");
    for value in [
        executable_sha256,
        onnxruntime_sha256,
        description_sha256.unwrap_or("none"),
    ] {
        hasher.update(value.as_bytes());
        hasher.update(b"\0");
    }
    lower_hex(&hasher.finalize())
}

/// Lowercase hexadecimal SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

/// Lowercase hexadecimal SHA-256 of the file at `path`, read in bounded chunks.
pub fn sha256_file_hex(module: &'static str, path: &Path) -> VisionFfiResult<String> {
    let mut file = fs::File::open(path).map_err(|error| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            module,
            format!("failed to open {} for hashing: {error}", path.display()),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                module,
                format!("failed to read {} for hashing: {error}", path.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(lower_hex(&hasher.finalize()))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}
