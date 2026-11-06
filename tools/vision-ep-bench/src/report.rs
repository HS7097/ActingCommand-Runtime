// SPDX-License-Identifier: AGPL-3.0-only

//! The report one bench process writes to `--out` (design §1.2: every cell has its numbers or a
//! named failure). All times are microseconds, all memory is bytes.

use crate::cli::BenchMode;
use crate::consistency::ConsistencySection;
use crate::error::{BenchError, OrtStatus};
use crate::safety::{MemoryReading, SafetyLimits};
use crate::stats::Percentiles;
use crate::template::{ExactEngine, RecallCounts};
use crate::workload::{RequestKind, TemplateMetric, WorkloadBlocks, WorkloadMask, WorkloadRect};
use actingcommand_ppocr_onnx_json_provider::bench::BenchPlacement;
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
    /// Workflow #407 GPU-0c: where ONNX Runtime placed each model's nodes, for a CUDA or
    /// DirectML run in OCR mode (empty for the CPU and for template mode).
    pub placement: Vec<ModelPlacement>,
    /// Workflow #407 GPU-0b: the template mode's rows (`--mode template`); the OCR sections
    /// above stay empty in that mode.
    pub template: Option<TemplateSection>,
    pub modules: Vec<ModuleRecord>,
    pub wall_clock_us: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Variant {
    pub mode: BenchMode,
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

/// Workflow #407 GPU-0c: where ONNX Runtime placed one model's nodes. Read from ONNX Runtime's
/// own verbose session log (its `Node placements` section, logged once the graph is partitioned
/// and optimised, so DirectML's fused nodes count as one each, and before the CPU-fallback
/// check), of a session built with the variant's exact options plus a verbose logger; no
/// inference runs on it.
#[derive(Debug, Clone, Serialize)]
pub struct ModelPlacement {
    /// `det` or `rec`.
    pub model: &'static str,
    /// Always `ort_verbose_session_log`.
    pub source: &'static str,
    /// `same_options`, or `fallback_allowed_twin` when the variant's own session failed without
    /// logging a whole placement and a twin with CPU fallback allowed was read instead.
    pub read_from: &'static str,
    /// Every execution provider ONNX Runtime named, with its node count, in log order.
    pub providers: Vec<ProviderNodes>,
    /// Nodes on any execution provider but the CPU's.
    pub non_cpu_nodes: usize,
    /// Nodes on ONNX Runtime's CPU execution provider (`CPUExecutionProvider`).
    pub cpu_nodes: usize,
    /// The CPU-placed nodes per operator type.
    pub cpu_op_types: BTreeMap<String, usize>,
    pub cpu_node_list: Vec<PlacedNode>,
    /// The variant's own session failure (a strict variant with CPU-placed nodes), with ONNX
    /// Runtime's status whole.
    pub session_failure: Option<OrtStatus>,
    /// Why no whole placement could be read; none when it was.
    pub log_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderNodes {
    pub provider: String,
    pub nodes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlacedNode {
    pub op_type: String,
    pub name: String,
}

impl ModelPlacement {
    /// The report row of `model` from the engine's placement read.
    pub fn from_read(model: &'static str, read: BenchPlacement) -> Self {
        let cpu_nodes: usize = read
            .providers
            .iter()
            .filter(|(provider, _)| provider == CPU_EXECUTION_PROVIDER)
            .map(|(_, nodes)| nodes)
            .sum();
        let non_cpu_nodes: usize = read
            .providers
            .iter()
            .filter(|(provider, _)| provider != CPU_EXECUTION_PROVIDER)
            .map(|(_, nodes)| nodes)
            .sum();
        let mut cpu_op_types = BTreeMap::new();
        for (op_type, _) in &read.cpu_nodes {
            *cpu_op_types.entry(op_type.clone()).or_insert(0) += 1;
        }
        Self {
            model,
            source: "ort_verbose_session_log",
            read_from: read.read_from,
            providers: read
                .providers
                .into_iter()
                .map(|(provider, nodes)| ProviderNodes { provider, nodes })
                .collect(),
            non_cpu_nodes,
            cpu_nodes,
            cpu_op_types,
            cpu_node_list: read
                .cpu_nodes
                .into_iter()
                .map(|(op_type, name)| PlacedNode { op_type, name })
                .collect(),
            session_failure: read.session_failure.as_ref().map(OrtStatus::from),
            log_error: read.log_error,
        }
    }

    /// `Op×n, …` of the CPU-placed nodes.
    pub fn cpu_op_summary(&self) -> String {
        self.cpu_op_types
            .iter()
            .map(|(op_type, count)| format!("{op_type}×{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// ONNX Runtime's name of its CPU execution provider.
pub const CPU_EXECUTION_PROVIDER: &str = "CPUExecutionProvider";

/// Workflow #407 GPU-0b: whole-frame template search (design §1.2 "模板匹配", §2.5). Every
/// template runs without and with its mask, once per scale set: one case each. All times are
/// microseconds over every frame (and every repetition), after the warm-up.
#[derive(Debug, Clone, Serialize)]
pub struct TemplateSection {
    pub frame_width: u32,
    pub frame_height: u32,
    pub frames: Vec<String>,
    /// Whether today's direct enumeration ran (`--direct-enumeration`).
    pub direct_enumeration: bool,
    pub suppress_iou_milli: u16,
    /// ε = this × the case's largest |Δ|.
    pub margin_factor: f64,
    pub templates: Vec<TemplateRecord>,
    pub cases: Vec<TemplateCaseRecord>,
    /// Cases that ended with a named failure; the run then fails after the last case.
    pub failed_cases: Vec<TemplateCaseFailure>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateRecord {
    pub id: String,
    pub size_class_px: u32,
    pub source: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub metric: TemplateMetric,
    pub threshold: f32,
    pub mask: WorkloadMask,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateCaseFailure {
    pub case: String,
    pub error: BenchError,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateCaseRecord {
    /// `<template>/<unmasked|masked>/<scales>`.
    pub case: String,
    pub template_id: String,
    pub masked: bool,
    pub scales_milli: Vec<u32>,
    pub scales: Vec<ScaledTemplateRecord>,
    /// Positions of one frame over every scale.
    pub positions_per_frame: u64,
    pub exact_map: ExactMapRow,
    pub direct_enumeration: DirectEnumerationRow,
    pub conv: ConvRow,
    pub accuracy: AccuracyRow,
    /// The first repetition on each frame.
    pub frames: Vec<TemplateFrameRecord>,
}

/// One scale of a case: the resampled template and its positions in one frame.
#[derive(Debug, Clone, Serialize)]
pub struct ScaledTemplateRecord {
    pub scale_milli: u32,
    pub width: u32,
    pub height: u32,
    pub positions: u64,
    /// The pixels scored (all of them without a mask).
    pub scored_pixels: u64,
    /// The engine the exact map sums with (the recognition crate's rule, see
    /// `template::exact_map_engine`).
    pub exact_map_engine: ExactEngine,
}

/// The CPU exact score map: `Scene::exact_score_map` (one scale) or `Scene::exact_score_maps`
/// (several), with the production 5 s limit on the system clock and a fresh `RegionSpectra` per
/// search, so the region's transforms are part of every sample.
#[derive(Debug, Clone, Serialize)]
pub struct ExactMapRow {
    pub call: &'static str,
    pub warm: Option<Percentiles>,
    /// Searches the 5 s limit stopped (their time is still a sample).
    pub incomplete: u64,
}

/// Today's direct enumeration: `Scene::match_template_all` (no mask, one scale per search; a
/// case of several scales runs one search per resampled template, each with its own 5 s limit),
/// every position admitted, overlap suppression as the workload says.
#[derive(Debug, Clone, Serialize)]
pub struct DirectEnumerationRow {
    /// `measured`, or `skipped: <why>`.
    pub status: String,
    pub searches_per_frame: usize,
    /// One frame's searches together.
    pub warm: Option<Percentiles>,
    pub complete: u64,
    /// Searches the 5 s limit stopped.
    pub incomplete: u64,
    /// Matches after suppression (complete searches) or found before the stop (incomplete ones).
    pub matches: u64,
}

/// The generated Conv graph on this run's execution provider.
#[derive(Debug, Clone, Serialize)]
pub struct ConvRow {
    pub graph_bytes: usize,
    pub graph_sha256: String,
    pub nodes: usize,
    pub outputs: usize,
    /// The session of the generated graph, built with the engine's session plan.
    pub session_build_us: u64,
    /// The first prefilter of the session (frame 0), before the warm-up.
    pub first_run_us: u64,
    /// `session.run` alone.
    pub run: Option<Percentiles>,
    /// The input tensor, `session.run`, and the CPU assembly of every float score (the window
    /// sums of an unmasked case included); the keep test against `threshold − ε` is counted
    /// after the run, once ε is known.
    pub prefilter: Option<Percentiles>,
    /// The process CPU time of the prefilter.
    pub prefilter_cpu: Option<Percentiles>,
}

/// Design §2.5: the float path against the exact map, over the first repetition of every frame.
#[derive(Debug, Clone, Serialize)]
pub struct AccuracyRow {
    /// The largest |exact − float| of the raw scores.
    pub max_abs_delta: f64,
    /// ε = `margin_factor` × `max_abs_delta`.
    pub margin: f64,
    pub counts: RecallCounts,
    /// `recalled / exact_matches`; none without an exact match.
    pub recall: Option<f64>,
    pub kept_per_frame: Option<Percentiles>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateFrameRecord {
    pub frame_id: String,
    pub exact_map_us: u64,
    pub exact_map_incomplete: bool,
    pub direct_us: Option<u64>,
    pub direct_incomplete: Option<bool>,
    pub conv_run_us: u64,
    pub prefilter_us: u64,
    pub max_abs_delta: f64,
    pub counts: RecallCounts,
}
