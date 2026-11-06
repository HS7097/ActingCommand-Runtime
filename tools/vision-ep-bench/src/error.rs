// SPDX-License-Identifier: AGPL-3.0-only

//! The bench's named failures. Every failure ends the process with a non-zero exit status and,
//! once `--out` is known, a report whose `status` is `failed` and whose `error` names the code.

use actingcommand_ppocr_onnx_json_provider::bench::BenchOrtFailure;
use actingcommand_vision_ffi::{NativeLibraryLoadFailure, VisionFfiError};
use serde::{Serialize, Serializer};
use std::fmt;

/// One named failure of the bench; it serialises as its spelling ([`BenchCode::as_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchCode {
    /// The command line is malformed or combines options that do not belong together.
    CliInvalid,
    /// The workload file cannot be read, parsed or validated.
    WorkloadInvalid,
    /// A workload frame is missing, differs from its recorded digest or size, or is no PNG.
    FrameInvalid,
    /// A path lies under the production install root, or `--out` already exists.
    FootprintRefused,
    /// BELOW_NORMAL priority could not be set or did not read back.
    PriorityUnverified,
    /// The host had less available memory than the start floor.
    HostMemoryLowAtStart,
    /// The host's available memory fell below the running floor.
    HostMemoryLow,
    /// The bench's own private bytes exceeded their cap.
    PrivateBytesExceeded,
    /// A process or host sampler failed.
    SamplerFailed,
    /// The vision root's ONNX Runtime cannot be listed or initialised.
    RuntimeUnavailable,
    /// The model folder is missing, unreadable or not an OCR folder.
    ModelInvalid,
    /// The model folder's content identity differs from the workload's.
    ModelIdentityMismatch,
    /// The requested CUDA ordinal or DXGI adapter index does not exist.
    DeviceUnknown,
    /// An ONNX Runtime session could not be built.
    SessionFailed,
    /// An inference or an engine read failed.
    InferenceFailed,
    /// The `--baseline` report cannot be read or does not match the workload.
    BaselineInvalid,
    /// The report could not be written.
    OutputUnwritable,
    /// Workflow #407 GPU-0b: a generated ONNX graph is malformed, or a model does not read back
    /// as one.
    GraphInvalid,
    /// Workflow #407 GPU-0b: an exact score map or the direct enumeration failed for another
    /// reason than its time limit.
    TemplateSearchFailed,
    /// Workflow #407 GPU-0c: ONNX Runtime's node placement of a GPU session could not be read
    /// from its verbose session log.
    PlacementUnread,
}

impl BenchCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CliInvalid => "bench_cli_invalid",
            Self::WorkloadInvalid => "bench_workload_invalid",
            Self::FrameInvalid => "bench_frame_invalid",
            Self::FootprintRefused => "bench_footprint_refused",
            Self::PriorityUnverified => "bench_priority_unverified",
            Self::HostMemoryLowAtStart => "bench_host_memory_low_at_start",
            Self::HostMemoryLow => "bench_host_memory_low",
            Self::PrivateBytesExceeded => "bench_private_bytes_exceeded",
            Self::SamplerFailed => "bench_sampler_failed",
            Self::RuntimeUnavailable => "bench_runtime_unavailable",
            Self::ModelInvalid => "bench_model_invalid",
            Self::ModelIdentityMismatch => "bench_model_identity_mismatch",
            Self::DeviceUnknown => "bench_device_unknown",
            Self::SessionFailed => "bench_session_failed",
            Self::InferenceFailed => "bench_inference_failed",
            Self::BaselineInvalid => "bench_baseline_invalid",
            Self::OutputUnwritable => "bench_output_unwritable",
            Self::GraphInvalid => "bench_graph_invalid",
            Self::TemplateSearchFailed => "bench_template_search_failed",
            Self::PlacementUnread => "bench_placement_unread",
        }
    }

    /// The process exit status: 2 for a command line error, 3 for a safety stop, 1 otherwise.
    pub const fn exit_status(self) -> i32 {
        match self {
            Self::CliInvalid => 2,
            Self::PriorityUnverified
            | Self::HostMemoryLowAtStart
            | Self::HostMemoryLow
            | Self::PrivateBytesExceeded
            | Self::FootprintRefused => 3,
            _ => 1,
        }
    }
}

impl fmt::Display for BenchCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for BenchCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// A named failure with its detail.
///
/// Workflow #407 GPU-0c: nothing behind a failure is dropped. A failure ONNX Runtime raised
/// keeps ONNX Runtime's status whole (`ort_status`), and a failed native library load keeps the
/// library, its `LoadLibraryExW` flags, the Win32 `GetLastError` code and its system message
/// (`library_load`); both are in the report's JSON and in the stderr line ([`fmt::Display`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BenchError {
    pub code: BenchCode,
    pub message: String,
    pub ort_status: Option<Box<OrtStatus>>,
    pub library_load: Option<Box<NativeLibraryLoadFailure>>,
}

/// Workflow #407 GPU-0c: ONNX Runtime's own status of a failed call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrtStatus {
    /// What the bench or the engine was doing.
    pub step: String,
    /// `ort::ErrorCode` as `Debug` spells it; none for a step that is no ONNX Runtime call.
    pub code: Option<String>,
    /// ONNX Runtime's status message, verbatim.
    pub message: Option<String>,
}

impl From<&BenchOrtFailure> for OrtStatus {
    fn from(failure: &BenchOrtFailure) -> Self {
        Self {
            step: failure.step.clone(),
            code: failure.ort_code.map(|code| format!("{code:?}")),
            message: failure.ort_message.clone(),
        }
    }
}

impl BenchError {
    pub fn new(code: BenchCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            ort_status: None,
            library_load: None,
        }
    }

    /// A failure the vision boundary raised while `what`: its text, and the native library load
    /// behind it when there is one.
    pub fn from_vision(code: BenchCode, what: &str, error: &VisionFfiError) -> Self {
        let mut bench = Self::new(code, format!("{what}: {error}"));
        bench.library_load = error.library_load().cloned().map(Box::new);
        bench
    }

    /// A failed ONNX Runtime session build while `what`: its text, and ONNX Runtime's status.
    pub fn from_ort(code: BenchCode, what: &str, failure: &BenchOrtFailure) -> Self {
        let mut bench = Self::new(code, format!("{what}: {failure}"));
        bench.ort_status = Some(Box::new(OrtStatus::from(failure)));
        bench
    }

    /// A failed ONNX Runtime call while `what`, from the `ort` error itself.
    pub fn from_ort_error<R>(code: BenchCode, what: &str, error: &ort::Error<R>) -> Self {
        let status = OrtStatus {
            step: what.to_owned(),
            code: Some(format!("{:?}", error.code())),
            message: Some(error.message().to_owned()),
        };
        let mut bench = Self::new(
            code,
            format!(
                "{what}: {} (ONNX Runtime error code {:?})",
                error.message(),
                error.code()
            ),
        );
        bench.ort_status = Some(Box::new(status));
        bench
    }
}

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)?;
        // The message normally carries both already; they are appended when it does not.
        if let Some(status) = &self.ort_status
            && let Some(message) = &status.message
            && !self.message.contains(message.as_str())
        {
            write!(formatter, " [ONNX Runtime status: {message}")?;
            if let Some(code) = &status.code {
                write!(formatter, ", error code {code}")?;
            }
            formatter.write_str("]")?;
        }
        if let Some(load) = &self.library_load {
            let text = load.to_string();
            if !self.message.contains(text.as_str()) {
                write!(formatter, " [loading {}: {text}]", load.library)?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for BenchError {}
