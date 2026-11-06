// SPDX-License-Identifier: AGPL-3.0-only

//! The report one bench process writes to `--out` (design §1.2: every cell has its numbers or a
//! named failure). All times are microseconds, all memory is bytes.

use crate::consistency::ConsistencySection;
use crate::error::BenchError;
use crate::safety::{MemoryReading, SafetyLimits};
use crate::stats::Percentiles;
use crate::workload::{RequestKind, WorkloadBlocks, WorkloadRect};
use actingcommand_vision_ffi::{CudaDeviceIdentity, OnnxExecutionProvider, OrtSessionTuning};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const REPORT_SCHEMA: &str = "actingcommand.vision-ep-bench.report/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Ok,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub status: ReportStatus,
    pub error: Option<BenchError>,
    /// The phase the failure happened in.
    pub phase_at_failure: Option<String>,
    pub variant: Variant,
    pub bench: BenchIdentity,
    pub safety: SafetySection,
    pub host: HostSection,
    pub workload: Option<WorkloadSection>,
    pub models: Vec<ModelRow>,
    pub runtime: Option<RuntimeSection>,
    /// Private bytes and host available memory at the named points.
    pub memory: Vec<MemoryMark>,
    pub cold: ColdSection,
    pub raw: RawSection,
    pub engine: EngineSection,
    pub shapes: ShapeSummary,
    /// The watcher's samples, per phase.
    pub phases: Vec<PhaseStats>,
    pub gpu_counters: CounterStatus,
    pub consistency: Option<ConsistencySection>,
    pub modules: Vec<ModuleRecord>,
    pub wall_clock_us: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Variant {
    pub label: String,
    pub execution_provider: OnnxExecutionProvider,
    /// The CUDA ordinal or the DXGI adapter index.
    pub device: Option<u32>,
    pub tuning: OrtSessionTuning,
    pub warmup: u32,
    pub reps: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchIdentity {
    pub crate_version: &'static str,
    pub executable: Option<String>,
    pub executable_sha256: Option<String>,
    pub process_id: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SafetySection {
    pub limits: SafetyLimits,
    pub priority_class_expected: u32,
    pub priority_class_read_back: Option<u32>,
    pub preflight: Option<MemoryReading>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HostSection {
    /// `IDXGIFactory1::EnumAdapters1` order, the order a DirectML device index names.
    pub dxgi_adapters: Vec<DxgiAdapter>,
    pub dxgi_error: Option<BenchError>,
    pub directml_adapter: Option<DxgiAdapter>,
    pub cuda: Option<CudaSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DxgiAdapter {
    pub index: u32,
    pub description: String,
    pub vendor_id: String,
    pub device_id: String,
    pub sub_sys_id: String,
    pub revision: u32,
    pub dedicated_video_memory_bytes: u64,
    pub dedicated_system_memory_bytes: u64,
    pub shared_system_memory_bytes: u64,
    /// As the GPU performance counters spell it: `0x<high>_0x<low>`.
    pub luid: String,
    pub flags: u32,
    pub software: bool,
    pub remote: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CudaSection {
    pub driver_version: u32,
    pub devices: Vec<CudaDeviceIdentity>,
    pub selected: CudaDeviceIdentity,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkloadSection {
    pub path: String,
    pub sha256: String,
    pub frames: usize,
    pub full_frame_requests: usize,
    pub roi_requests: usize,
    pub model_ref: String,
    pub model_sha256: String,
}

/// One model row of design §1.2 (`det`, `rec`, `nn`).
#[derive(Debug, Clone, Serialize)]
pub struct ModelRow {
    pub role: &'static str,
    pub path: Option<String>,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
    /// `n/a` with its reason for a row the bench does not measure.
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSection {
    pub onnxruntime_library: String,
    pub onnxruntime_version: String,
    pub onnxruntime_sha256: String,
    pub onnxruntime_build_info: String,
    pub runtime_library_closure: Vec<String>,
    pub cuda_driver_version: Option<u32>,
    pub resolved_cuda_device: Option<CudaDeviceIdentity>,
    pub registered_execution_providers: Vec<OnnxExecutionProvider>,
    pub cpu_ep_registered: bool,
    pub cpu_fallback_disabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryMark {
    pub mark: String,
    pub reading: MemoryReading,
}

/// Design §1.2 cold start.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ColdSection {
    /// The runtime-library closure, `ort::init_from` and, for CUDA, the device inventory.
    pub ort_init_us: Option<u64>,
    /// The first session of the process (the raw detector): it creates the CUDA context or the
    /// DirectML device.
    pub first_session_us: Option<u64>,
    /// The second session (the raw recognizer).
    pub second_session_us: Option<u64>,
    /// `PpocrCtcLoader::load_ocr`: model hashing and the engine's recognizer session.
    pub engine_load_ocr_us: Option<u64>,
    /// The engine's first full-frame read, which builds its detector session first.
    pub engine_first_full_frame_us: Option<u64>,
    /// Distinct input shapes seen (detector and recognizer).
    pub distinct_input_shapes: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RawSection {
    pub detector: Vec<ShapeTiming>,
    pub recognizer: Vec<ShapeTiming>,
}

/// A raw `session.run` on one input shape: its first run in the session, then the warm runs.
#[derive(Debug, Clone, Serialize)]
pub struct ShapeTiming {
    /// NCHW, e.g. `1x3x544x960`.
    pub shape: String,
    /// The frame and region the input was built from.
    pub source: String,
    /// How many workload regions map to this shape.
    pub regions: usize,
    pub first_run_us: u64,
    pub warm: Option<Percentiles>,
    pub warm_cpu: Option<Percentiles>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EngineSection {
    pub requests: Vec<RequestRecord>,
    /// Every warm repetition of every full-frame request, pooled.
    pub full_frame: Option<Percentiles>,
    pub full_frame_cpu: Option<Percentiles>,
    /// Every warm repetition of every ROI request, pooled.
    pub roi: Option<Percentiles>,
    pub roi_cpu: Option<Percentiles>,
    /// Design §1.6 item 3 without PERF-READ weights: the mean of the two pooled p95 values.
    pub mixed_p95_us: Option<u64>,
    pub mixed_weights: BTreeMap<String, f64>,
    /// Warm repetitions whose reading differed from the request's first reading.
    pub unstable_repetitions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub frame_id: String,
    pub request_id: String,
    pub kind: RequestKind,
    pub region: WorkloadRect,
    pub blocks: WorkloadBlocks,
    pub first_run_us: u64,
    /// Input shapes this request was the first engine request to use.
    pub new_shapes: Vec<String>,
    pub output: RequestOutput,
    pub expected_text: Option<String>,
    pub matches_truth: Option<bool>,
    pub warm: Option<Percentiles>,
    pub warm_cpu: Option<Percentiles>,
    pub unstable_repetitions: u32,
}

/// What one engine read returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestOutput {
    pub text: String,
    pub confidence: Option<f32>,
    pub blocks: Vec<BlockOutput>,
    /// A detect read's box count before the folder's `max_boxes` cut.
    pub detected_boxes: Option<u32>,
    pub box_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockOutput {
    pub text: String,
    pub rect: WorkloadRect,
    pub confidence: Option<f32>,
}

impl RequestOutput {
    /// The same text, blocks (text and rectangle) and box count; confidences may differ.
    pub fn same_reading(&self, other: &Self) -> bool {
        self.text == other.text
            && self.detected_boxes == other.detected_boxes
            && self.blocks.len() == other.blocks.len()
            && self
                .blocks
                .iter()
                .zip(&other.blocks)
                .all(|(left, right)| left.text == right.text && left.rect == right.rect)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ShapeSummary {
    pub detector: Vec<String>,
    pub recognizer: Vec<String>,
}

/// The watcher's samples of one phase.
#[derive(Debug, Clone, Serialize)]
pub struct PhaseStats {
    pub phase: String,
    pub samples: u64,
    pub own_private_bytes_max: u64,
    pub host_available_bytes_min: Option<u64>,
    /// Per GPU (by LUID): this process's dedicated and shared GPU memory and the busiest engine's
    /// utilisation, for this process and for the whole host.
    pub gpus: BTreeMap<String, GpuPhaseStats>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GpuPhaseStats {
    pub samples: u64,
    pub own_dedicated_bytes_max: u64,
    pub own_shared_bytes_max: u64,
    pub own_busiest_engine_percent_max: f64,
    pub own_busiest_engine_percent_mean: f64,
    pub host_busiest_engine_percent_max: f64,
    pub host_busiest_engine_percent_mean: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CounterStatus {
    pub available: bool,
    pub error: Option<BenchError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleRecord {
    pub path: String,
    pub file_version: Option<String>,
}
