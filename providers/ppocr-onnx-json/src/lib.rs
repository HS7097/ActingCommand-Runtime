// SPDX-License-Identifier: AGPL-3.0-only

//! In-process ONNX Runtime PP-OCR engine of the `ppocr-ctc` family (Workflow #360).
//!
//! actingd links this crate and hands [`PpocrCtcLoader`] to the Runtime as its vision model
//! loader. The loader builds one engine per model on first use: it reads each model file
//! once, verifies the model's content identity from those bytes and builds the ONNX Runtime
//! sessions from the same bytes. ONNX Runtime itself is initialised once per process, on the
//! first model build. A request borrows the frame and only the pixels inside the requested
//! region are read. The engine does not copy MAA C++ code and does not bundle OCR models or
//! runtime DLLs.

use actingcommand_onnx_provider_support::{InferenceWatchdog, OrtRuntimeInitializer};
use actingcommand_vision_ffi::{
    AdapterSource, CudaDeviceIdentity, DIRECTML_RUNTIME_PIN, DirectmlDeviceIdentity,
    DirectmlDeviceSelector, DirectmlUnavailable, FileState, GpuFallback, GpuUnavailableReason,
    OCR_EXECUTION_ATTESTATION_SCHEMA_VERSION, OcrBlockMode, OcrDetectionCount, OcrEngine,
    OcrExecutionAttestation, OcrExecutionStatus, OcrFallbackPolicy, OcrInferenceOutput,
    OcrInferenceRequest, OcrInferenceRequestView, OcrInferenceResult, OcrInvocationId,
    OcrLineMerge, OcrModelExecution, OcrModelLoad, OcrModelSpec, OcrProviderBuildIdentity,
    OcrRuntimeBuildIdentity, OcrSessionBinding, OcrSessionId, OcrSessionKey, OcrSessionKeyParts,
    OcrTextBlock, OnnxExecutionProvider, Outcome, ResolvedDirectmlAdapter, SystemAdapterSource,
    VisionBackendKind, VisionFfiError, VisionFfiErrorCode, VisionFfiResult, VisionFrameView,
    VisionGpuCode, VisionModelLoader, VisionPixelFormat, VisionRect, VisionRuntimeSpec,
    enumerate_cuda_devices, establish_process_runtime_library_closure, next_ocr_invocation_id,
    ocr_engine_binding_sha256, onnxruntime_status, onnxruntime_version_string,
    ppocr_model_set_sha256, resolve_directml_adapter, sha256_file_hex, sha256_hex,
};
use actingcommand_vision_ffi::{
    ChannelOrder, DetectorResize, NnModelLoad, NnModelSpec, PpocrCtcParams, SpaceClass,
};
use actingcommand_vision_ffi::{
    PPOCR_CPU_EXECUTION_PROVIDER, PPOCR_DIRECTML_EXECUTION_PROVIDER, PPOCR_MAX_DIAGNOSTIC_REPORTS,
    PPOCR_NODE_PLACEMENT_RECORD_TYPE, PpocrCpuAssignedNodeDiagnostic as CpuAssignedNodeDiagnostic,
    PpocrDiagnosticDevice, PpocrDiagnostics,
    PpocrNodePlacementDiagnostic as NodePlacementDiagnostic, PpocrNodePlacementReason,
    validate_ppocr_call_diagnostics,
};
use ort::logging::{LogLevel, LoggerFunction};
use ort::session::builder::SessionBuilder;
use ort::session::{RunOptions, Session};
use ort::value::{Tensor, TensorElementType, ValueType};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsStr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::time::{Duration, Instant};

// Workflow #407 GPU-1 B1: DirectML sessions and their node placement, over a fake ONNX Runtime.
#[cfg(test)]
mod gate_407_gpu1b1;
// Workflow #407 GPU-1 B2: the whole-model fallback to the CPU, over a fake ONNX Runtime.
#[cfg(test)]
mod gate_407_gpu1b2;
// Workflow #407 GPU-1 E: the rebuild after a failed GPU run, over a fake ONNX Runtime.
#[cfg(test)]
mod gate_407_gpu1e;
mod nn;

const NODE_PLACEMENT_DIAGNOSTIC_ENV: &str = "ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC";
const MAX_NODE_PLACEMENT_DIAGNOSTIC_NODES: usize = 4_096;
const MAX_NODE_PLACEMENT_LOG_MESSAGE_BYTES: usize = 4_096;
const CPU_EXECUTION_PROVIDER: &str = PPOCR_CPU_EXECUTION_PROVIDER;
/// ONNX Runtime's names of the accelerated providers whose node placement a session reads.
const CUDA_EXECUTION_PROVIDER: &str = "CUDAExecutionProvider";
const DIRECTML_EXECUTION_PROVIDER: &str = PPOCR_DIRECTML_EXECUTION_PROVIDER;
/// The section header of ONNX Runtime's verbose node-placement log.
const NODE_PLACEMENTS_HEADER: &str = "Node placements";
/// ONNX Runtime's provider summaries: `All nodes placed on [<EP>]. Number of nodes: <n>` names
/// the one provider and lists no node; `Node(s) placed on [<EP>]. Number of nodes: <n>` is
/// followed by one `<op type> (<node name>)` line per node.
const PROVIDER_SUMMARY_PREFIXES: [(&str, bool); 2] = [
    ("All nodes placed on [", false),
    ("Node(s) placed on [", true),
];
/// The intra-op threads of every session (design §2.1; the provider options digest hashes it).
const SESSION_INTRA_THREADS: usize = 1;
const ENGINE_MODULE: &str = "ppocr_ctc_engine";
/// Recorded as the execution attestation's provider implementation.
const ENGINE_IMPLEMENTATION: &str = "actingcommand-ocr-engine/ppocr-ctc";
/// The default recognizer width cap; a dynamic width above it is sized with floor, as the
/// official PP-OCR pipeline sizes wide crops.
const OFFICIAL_FLOOR_WIDTH_ABOVE: usize = 320;

static ORT_RUNTIME: OrtRuntimeInitializer = OrtRuntimeInitializer::new();
static RUNTIME_FACTS: OnceLock<RuntimeFacts> = OnceLock::new();
static RUNTIME_INIT: Mutex<()> = Mutex::new(());
/// Workflow #407 GPU-1 E: the GPU-run history of each model of a `directml` configuration, by
/// its model reference and content, kept for the life of the process across the model's loads.
static GPU_RUNS: Mutex<BTreeMap<(String, String), GpuRuns>> = Mutex::new(BTreeMap::new());

/// The ONNX Runtime facts established once per process, on the first model build.
struct RuntimeFacts {
    onnxruntime_library: PathBuf,
    onnxruntime_version: String,
    onnxruntime_sha256: String,
    executable_sha256: String,
    resolved_cuda_device: Option<CudaDeviceIdentity>,
    cuda_driver_version: Option<u32>,
    /// Workflow #407 GPU-1 B2: what a `directml` configuration's process runs, decided once when
    /// its runtime was initialised; `None` for every other provider.
    directml: Option<ProcessRoute>,
}

/// Workflow #407 GPU-1 B2 (design §1.4, §2.3): what the process of a `directml` configuration
/// runs, decided once at its runtime initialisation and kept until it restarts.
#[derive(Debug, Clone)]
enum ProcessRoute {
    /// DirectML is in the closure: the adapter the first model load resolved, whose identity every
    /// later session build must resolve (B1).
    Directml(ResolvedDirectmlAdapter),
    /// DirectML could not be used under `gpu_fallback` `cpu`: the closure is the CPU one and every
    /// model runs on the CPU; the one warning of that fallback, with no `model_name`.
    Cpu(Outcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderSessionPlan {
    resolved_execution_provider: OnnxExecutionProvider,
    cuda_ordinal: Option<i32>,
    /// Workflow #407 GPU-1 B1: the DXGI index of a DirectML session's adapter, which ONNX
    /// Runtime's DirectML `device_id` names.
    directml_device_id: Option<i32>,
    registered_execution_providers: Vec<OnnxExecutionProvider>,
    cpu_ep_registered: bool,
    cpu_fallback_disabled: bool,
    /// The attestation's fallback policy (design §2.4): `forbidden` for CPU and CUDA,
    /// `cpu_nodes` for DirectML.
    fallback_policy: OcrFallbackPolicy,
}

impl ProviderSessionPlan {
    fn from_key(key: &OcrSessionKey) -> Result<Self, String> {
        match key.requested_backend() {
            OnnxExecutionProvider::Cpu => Ok(Self {
                resolved_execution_provider: OnnxExecutionProvider::Cpu,
                cuda_ordinal: None,
                directml_device_id: None,
                registered_execution_providers: vec![OnnxExecutionProvider::Cpu],
                cpu_ep_registered: true,
                cpu_fallback_disabled: false,
                fallback_policy: OcrFallbackPolicy::Forbidden,
            }),
            OnnxExecutionProvider::Cuda => {
                let ordinal = key
                    .resolved_cuda_device()
                    .ok_or_else(|| "CUDA OCR session key is missing resolved device".to_string())?
                    .ordinal;
                let ordinal = i32::try_from(ordinal)
                    .map_err(|_| "CUDA OCR ordinal exceeds i32 range".to_string())?;
                Ok(Self {
                    resolved_execution_provider: OnnxExecutionProvider::Cuda,
                    cuda_ordinal: Some(ordinal),
                    directml_device_id: None,
                    registered_execution_providers: vec![OnnxExecutionProvider::Cuda],
                    cpu_ep_registered: false,
                    cpu_fallback_disabled: true,
                    fallback_policy: OcrFallbackPolicy::Forbidden,
                })
            }
            // Workflow #407 GPU-1 B1 (design §2.1): DirectML alone is registered, and ONNX
            // Runtime may place the nodes DirectML cannot run on its CPU provider, which it adds
            // itself: CPU fallback stays allowed and the CPU provider is not registered.
            OnnxExecutionProvider::Directml => {
                // Workflow #407 GPU-1 B2 (design §2.3, §2.4): a DirectML key without a resolved
                // adapter is a model that DirectML could not be used for under `gpu_fallback`
                // `cpu`. The whole model runs on the CPU, as a CPU plan does, and attests the
                // fallback policy `cpu_model`.
                let Some(device) = key.resolved_directml_device() else {
                    return Ok(Self {
                        resolved_execution_provider: OnnxExecutionProvider::Cpu,
                        cuda_ordinal: None,
                        directml_device_id: None,
                        registered_execution_providers: vec![OnnxExecutionProvider::Cpu],
                        cpu_ep_registered: true,
                        cpu_fallback_disabled: false,
                        fallback_policy: OcrFallbackPolicy::CpuModel,
                    });
                };
                let index = device.dxgi_index;
                let device_id = i32::try_from(index)
                    .map_err(|_| "DirectML adapter index exceeds i32 range".to_string())?;
                Ok(Self {
                    resolved_execution_provider: OnnxExecutionProvider::Directml,
                    cuda_ordinal: None,
                    directml_device_id: Some(device_id),
                    registered_execution_providers: vec![OnnxExecutionProvider::Directml],
                    cpu_ep_registered: false,
                    cpu_fallback_disabled: false,
                    fallback_policy: OcrFallbackPolicy::CpuNodes,
                })
            }
        }
    }
}

/// The execution provider one session registers, with its device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionExecutionProvider {
    /// None: ONNX Runtime runs every node on its CPU provider.
    Cpu,
    Cuda {
        device_id: i32,
    },
    /// `device_id` is the adapter's DXGI index.
    Directml {
        device_id: i32,
    },
}

impl SessionExecutionProvider {
    /// ONNX Runtime's name of the provider, whose nodes a placement read counts; none for CPU.
    const fn accelerated_name(self) -> Option<&'static str> {
        match self {
            Self::Cpu => None,
            Self::Cuda { .. } => Some(CUDA_EXECUTION_PROVIDER),
            Self::Directml { .. } => Some(DIRECTML_EXECUTION_PROVIDER),
        }
    }

    const fn is_directml(self) -> bool {
        matches!(self, Self::Directml { .. })
    }
}

/// Workflow #407 GPU-1 B1 (design §2.1): every option the engine sets on an ONNX Runtime
/// session before its model, as data. [`session_options`] is their one source, and a session and
/// the zero-inference shadow that reads its node placement are built from the same value, so the
/// two cannot differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionOptions {
    intra_threads: usize,
    /// `Some(false)` turns ONNX Runtime's memory pattern off; `None` keeps its default.
    memory_pattern: Option<bool>,
    /// `Some(false)` runs the graph sequentially; `None` keeps ONNX Runtime's default.
    parallel_execution: Option<bool>,
    execution_provider: SessionExecutionProvider,
    /// Whether ONNX Runtime is forbidden to place a node on its CPU provider.
    cpu_fallback_disabled: bool,
}

/// The options of every session of `plan`. CPU and CUDA sessions set what they always have (one
/// intra-op thread; CUDA forbids CPU nodes). A DirectML session turns the memory pattern off and
/// runs sequentially, as DirectML requires (one session never runs concurrently: the model's
/// `&mut` session and its slot's lock guarantee that), and allows CPU nodes: the recognition
/// model has nodes only the CPU runs.
fn session_options(plan: &ProviderSessionPlan) -> VisionFfiResult<SessionOptions> {
    let (memory_pattern, parallel_execution, execution_provider) =
        match plan.resolved_execution_provider {
            OnnxExecutionProvider::Cpu => (None, None, SessionExecutionProvider::Cpu),
            OnnxExecutionProvider::Cuda => {
                let device_id = plan.cuda_ordinal.ok_or_else(|| {
                    failure("CUDA OCR session plan is missing the resolved device ordinal")
                })?;
                (None, None, SessionExecutionProvider::Cuda { device_id })
            }
            OnnxExecutionProvider::Directml => {
                let device_id = plan.directml_device_id.ok_or_else(|| {
                    failure("DirectML OCR session plan is missing its adapter's DXGI index")
                })?;
                (
                    Some(false),
                    Some(false),
                    SessionExecutionProvider::Directml { device_id },
                )
            }
        };
    Ok(SessionOptions {
        intra_threads: SESSION_INTRA_THREADS,
        memory_pattern,
        parallel_execution,
        execution_provider,
        cpu_fallback_disabled: plan.cpu_fallback_disabled,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PpocrModelRole {
    Recognizer,
    Detector,
}

impl PpocrModelRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Recognizer => "recognizer",
            Self::Detector => "detector",
        }
    }
}

/// ONNX Runtime's verbose `Node placements` section of one session (design §2.2): one provider
/// summary per provider (see [`PROVIDER_SUMMARY_PREFIXES`]), in any provider order, each
/// `Node(s)` summary followed by its per-node lines. The CPU-placed nodes are kept; the nodes of
/// the accelerated provider are counted. Lines outside the section are ignored. A summary or
/// per-node line that does not read, a provider named twice, or a group that ends before its
/// nodes are listed fails the read.
#[derive(Debug, Default)]
struct NodePlacementLogCapture {
    saw_placement_header: bool,
    /// Every provider the section named, with its node count, in log order.
    providers: Vec<(String, usize)>,
    /// The provider (its index in `providers`) whose per-node lines follow, and how many of
    /// them are still due.
    open_group: Option<(usize, usize)>,
    cpu_nodes: Vec<CpuAssignedNodeDiagnostic>,
    failure: Option<String>,
}

impl NodePlacementLogCapture {
    fn record(&mut self, message: &str) {
        if self.failure.is_some() {
            return;
        }
        let message = message.trim();
        if message == NODE_PLACEMENTS_HEADER {
            *self = Self {
                saw_placement_header: true,
                ..Self::default()
            };
            return;
        }
        if !self.saw_placement_header {
            return;
        }
        if let Some(&(prefix, has_details)) = PROVIDER_SUMMARY_PREFIXES
            .iter()
            .find(|(prefix, _)| message.starts_with(prefix))
        {
            self.record_provider_summary(message, prefix, has_details);
            return;
        }
        let Some((index, due)) = self.open_group else {
            return;
        };
        if message.len() > MAX_NODE_PLACEMENT_LOG_MESSAGE_BYTES {
            self.fail("ONNX Runtime node-placement log message exceeds the diagnostic bound");
            return;
        }
        if self.providers[index].0 == CPU_EXECUTION_PROVIDER {
            match parse_node_placement(message) {
                Ok(node) => self.cpu_nodes.push(node),
                Err(err) => {
                    self.fail(err);
                    return;
                }
            }
        } else if let Err(err) = parse_accelerated_node_placement(message) {
            let provider = self.providers[index].0.clone();
            self.fail(format!("{} of {provider}", err.message()));
            return;
        }
        self.open_group = (due > 1).then_some((index, due - 1));
    }

    fn record_provider_summary(&mut self, message: &str, prefix: &str, has_details: bool) {
        if message.len() > MAX_NODE_PLACEMENT_LOG_MESSAGE_BYTES {
            self.fail("ONNX Runtime node-placement log message exceeds the diagnostic bound");
            return;
        }
        if let Some((index, _)) = self.open_group {
            let provider = &self.providers[index].0;
            let failure = if provider == CPU_EXECUTION_PROVIDER {
                "ONNX Runtime ended the CPU placement group before every node was recorded"
                    .to_string()
            } else {
                format!(
                    "ONNX Runtime ended the {provider} placement group before every node was listed"
                )
            };
            self.fail(failure);
            return;
        }
        let (provider, count) = match parse_provider_summary(&message[prefix.len()..]) {
            Ok(summary) => summary,
            Err(err) => {
                self.fail(err);
                return;
            }
        };
        if self.providers.iter().any(|(named, _)| *named == provider) {
            self.fail(format!(
                "ONNX Runtime named {provider} twice in one node-placement section"
            ));
            return;
        }
        if provider == CPU_EXECUTION_PROVIDER {
            if count > MAX_NODE_PLACEMENT_DIAGNOSTIC_NODES {
                self.fail("ONNX Runtime CPU-assigned node count exceeds the diagnostic bound");
                return;
            }
            if !has_details && count > 0 {
                self.fail(
                    "ONNX Runtime did not emit per-node details for the CPU-only placement group",
                );
                return;
            }
        }
        if has_details && count > 0 {
            self.open_group = Some((self.providers.len(), count));
        }
        self.providers.push((provider, count));
    }

    /// The section as a v2 report of `role`: the one accelerated provider ONNX Runtime named
    /// with its node count, and the CPU-placed nodes. The caller checks the provider against the
    /// session's and adds a DirectML session's adapter.
    fn diagnostic(&self, role: PpocrModelRole) -> Result<NodePlacementDiagnostic, String> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if !self.saw_placement_header || self.providers.is_empty() {
            return Err(
                "ONNX Runtime did not emit a complete verbose node-placement section".to_string(),
            );
        }
        if let Some((index, due)) = self.open_group {
            let (provider, count) = &self.providers[index];
            return Err(if provider == CPU_EXECUTION_PROVIDER {
                format!(
                    "ONNX Runtime reported {count} CPU-assigned nodes but emitted details for {}",
                    self.cpu_nodes.len()
                )
            } else {
                format!(
                    "ONNX Runtime reported {count} nodes on {provider} but emitted details for {}",
                    count - due
                )
            });
        }
        let cpu_assigned_node_count = self
            .providers
            .iter()
            .find(|(provider, _)| provider == CPU_EXECUTION_PROVIDER)
            .map_or(0, |(_, count)| *count);
        if self.cpu_nodes.len() != cpu_assigned_node_count {
            return Err(format!(
                "ONNX Runtime reported {cpu_assigned_node_count} CPU-assigned nodes but emitted details for {}",
                self.cpu_nodes.len()
            ));
        }
        let mut accelerated = self
            .providers
            .iter()
            .filter(|(provider, _)| provider != CPU_EXECUTION_PROVIDER);
        let Some((accelerated_execution_provider, accelerated_node_count)) = accelerated.next()
        else {
            return Err(
                "ONNX Runtime's node-placement section names no accelerated execution provider"
                    .to_string(),
            );
        };
        if let Some((other, _)) = accelerated.next() {
            return Err(format!(
                "ONNX Runtime's node-placement section names {other} besides {accelerated_execution_provider}"
            ));
        }
        let mut nodes = self.cpu_nodes.clone();
        nodes.sort_by(|left, right| {
            left.node_name
                .cmp(&right.node_name)
                .then_with(|| left.operator_type.cmp(&right.operator_type))
        });
        Ok(NodePlacementDiagnostic {
            record_type: PPOCR_NODE_PLACEMENT_RECORD_TYPE.to_string(),
            model_role: role.as_str().to_string(),
            diagnostic_scope: "session_initialization_only".to_string(),
            inference_executed: false,
            accelerated_execution_provider: accelerated_execution_provider.clone(),
            accelerated_node_count: *accelerated_node_count,
            cpu_assigned_node_count,
            device: None,
            nodes,
        })
    }

    fn fail(&mut self, message: impl Into<String>) {
        if self.failure.is_none() {
            self.failure = Some(message.into());
        }
        self.open_group = None;
    }
}

/// A provider summary after its prefix: `<EP>]. Number of nodes: <n>`.
fn parse_provider_summary(rest: &str) -> Result<(String, usize), String> {
    let Some((provider, count)) = rest.split_once("]. Number of nodes: ") else {
        return Err("malformed ONNX Runtime node-placement provider summary".to_string());
    };
    if provider.is_empty() {
        return Err("ONNX Runtime node-placement provider is empty".to_string());
    }
    let count = count.parse::<usize>().map_err(|err| {
        format!("invalid ONNX Runtime node-placement provider count {count:?}: {err}")
    })?;
    Ok((provider.to_string(), count))
}

/// A per-node line of an accelerated provider's group, which is only counted: `<op type> (<node
/// name>)` with an operator type; a node ONNX Runtime inserted may have an empty name.
fn parse_accelerated_node_placement(message: &str) -> VisionFfiResult<()> {
    let readable = message
        .split_once(" (")
        .is_some_and(|(operator_type, node_name)| {
            !operator_type.is_empty() && node_name.ends_with(')')
        });
    if readable {
        Ok(())
    } else {
        Err(failure("malformed ONNX Runtime node-placement entry"))
    }
}

fn parse_node_placement(message: &str) -> Result<CpuAssignedNodeDiagnostic, String> {
    let Some((operator_type, node_name)) = message.split_once(" (") else {
        return Err("malformed ONNX Runtime CPU node-placement entry".to_string());
    };
    let Some(node_name) = node_name.strip_suffix(')') else {
        return Err("malformed ONNX Runtime CPU node-placement entry".to_string());
    };
    if operator_type.is_empty() || node_name.is_empty() {
        return Err(
            "ONNX Runtime CPU node-placement entry has an empty operator or node name".to_string(),
        );
    }
    Ok(CpuAssignedNodeDiagnostic {
        node_name: node_name.to_string(),
        operator_type: operator_type.to_string(),
        domain: "unavailable".to_string(),
        placement_reason: PpocrNodePlacementReason::Unavailable,
        assigned_execution_provider: CPU_EXECUTION_PROVIDER.to_string(),
    })
}

/// Whether a session of `plan` reads its node placement. `value` is the caller-injected variable
/// value the loader was built with (Workflow #318 cfg3); the engine never reads the environment
/// itself. A set value must be exactly `1`. A CUDA session reads the placement only when it is
/// set; a DirectML session always reads it (Workflow #407 GPU-1 design §2.2), so the variable is
/// met there; a CPU session refuses it.
fn node_placement_diagnostic_requested(
    plan: &ProviderSessionPlan,
    value: Option<&str>,
) -> Result<bool, String> {
    if let Some(value) = value
        && value != "1"
    {
        return Err(format!(
            "{NODE_PLACEMENT_DIAGNOSTIC_ENV} must be exactly 1 when set"
        ));
    }
    match plan.resolved_execution_provider {
        OnnxExecutionProvider::Directml => Ok(true),
        OnnxExecutionProvider::Cuda => Ok(value.is_some()),
        // Workflow #407 GPU-1 B2: a model that fell back from DirectML to the CPU has no placement
        // to read; the variable its DirectML configuration may carry stays met.
        OnnxExecutionProvider::Cpu if plan.fallback_policy == OcrFallbackPolicy::CpuModel => {
            Ok(false)
        }
        OnnxExecutionProvider::Cpu if value.is_some() => Err(format!(
            "{NODE_PLACEMENT_DIAGNOSTIC_ENV} is valid only for an explicit CUDA OCR session; a DirectML OCR session always reads its node placement"
        )),
        OnnxExecutionProvider::Cpu => Ok(false),
    }
}

/// Builds one `ppocr-ctc` OCR engine per model on first use.
pub struct PpocrCtcLoader {
    runtime: VisionRuntimeSpec,
    node_placement_diagnostic: Option<String>,
}

impl PpocrCtcLoader {
    /// `node_placement_diagnostic` is the caller-injected
    /// `ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC` value (Workflow #318 cfg3); the engine
    /// validates it and never reads the environment itself.
    pub fn new(runtime: VisionRuntimeSpec, node_placement_diagnostic: Option<String>) -> Self {
        Self {
            runtime,
            node_placement_diagnostic,
        }
    }
}

impl VisionModelLoader for PpocrCtcLoader {
    fn load_ocr(
        &self,
        model: &OcrModelSpec,
        expected_model_sha256: &str,
        session_id: &OcrSessionId,
        wait_deadline: Instant,
    ) -> VisionFfiResult<OcrModelLoad> {
        let detector_sha256 = model
            .detector_path
            .as_deref()
            .map(|path| read_model_file(path, "detector").map(|bytes| sha256_hex(&bytes)))
            .transpose()?;
        let recognizer = read_model_file(&model.recognizer_path, "recognizer")?;
        let recognizer_sha256 = sha256_hex(&recognizer);
        let dictionary = read_model_file(&model.dictionary_path, "dictionary")?;
        let model_sha256 = ppocr_model_set_sha256(
            detector_sha256.as_deref(),
            &recognizer_sha256,
            &sha256_hex(&dictionary),
            None,
        )?;
        if model_sha256 != expected_model_sha256 {
            return Ok(OcrModelLoad::Mismatch { model_sha256 });
        }
        let dictionary = parse_dictionary(&dictionary, &model.dictionary_path).map_err(failure)?;
        let (facts, waited_on_others) = runtime_facts(&self.runtime, wait_deadline)?;
        let basis = RouteBasis {
            session_id: session_id.clone(),
            parts: OcrSessionKeyParts {
                engine_binding_sha256: ocr_engine_binding_sha256(
                    &facts.executable_sha256,
                    &facts.onnxruntime_sha256,
                    &model.description_sha256,
                ),
                runtime_library_path: facts.onnxruntime_library.to_string_lossy().into_owned(),
                runtime_library_sha256: facts.onnxruntime_sha256.clone(),
                onnxruntime_version: facts.onnxruntime_version.clone(),
                model_ref: model.model_ref.clone(),
                model_sha256: model_sha256.clone(),
                requested_backend: self.runtime.execution_provider,
                requested_cuda_device: self.runtime.cuda_device.clone(),
                resolved_cuda_device: facts.resolved_cuda_device.clone(),
                requested_directml_device: self
                    .runtime
                    .directml
                    .as_ref()
                    .map(|directml| directml.device.clone()),
                resolved_directml_device: None,
            },
            gpu_fallback: self
                .runtime
                .directml
                .as_ref()
                .map(|directml| directml.gpu_fallback),
        };
        // Workflow #407 GPU-1 E (design §7 row E): what the model's DirectML runs showed earlier in
        // this process; a `cpu` or `cuda` model has none to keep.
        let gpu_runs = match basis.gpu_fallback {
            Some(_) => model_gpu_runs(&model.model_ref, &model_sha256),
            None => GpuRuns::default(),
        };
        // Workflow #407 GPU-1 B1, B2 (design §1.4, §2.3): every DirectML session build resolves
        // its adapter again, and a model whose adapter or session cannot be used moves to the CPU
        // under `gpu_fallback` `cpu`; E: a model whose first run after a rebuild failed too stays
        // on the CPU.
        let route = load_route(
            &basis,
            facts.directml.as_ref(),
            &SystemAdapterSource,
            &gpu_runs,
        )?;
        let mut diagnostics = Vec::new();
        let sessions = ModelSessions::load(
            &mut OrtSessions,
            basis,
            route,
            ModelInput {
                bytes: &recognizer,
                path: model.recognizer_path.as_path(),
            },
            self.node_placement_diagnostic.clone(),
            &mut diagnostics,
        )
        .map_err(|error| error.with_ppocr_diagnostics(diagnostics.clone()))?;
        require_recognizer_classes(
            &sessions.recognizer.session,
            dictionary.len(),
            &model.model_ref,
        )
        .map_err(|error| failure(error).with_ppocr_diagnostics(diagnostics.clone()))?;
        let execution = sessions.status.clone();
        Ok(OcrModelLoad::Loaded {
            engine: Box::new(PpocrCtcModel {
                sessions,
                cuda_driver_version: facts.cuda_driver_version,
                dictionary,
                params: model.description.ppocr_params(),
                recognizer_path: model.recognizer_path.clone(),
                recognizer_sha256,
                detector_path: model.detector_path.clone(),
                detector_sha256,
                pending_diagnostics: diagnostics,
                gpu_runs,
            }),
            model_sha256,
            waited_on_others,
            execution,
        })
    }

    fn load_nn(
        &self,
        model: &NnModelSpec,
        expected_model_sha256: &str,
        wait_deadline: Instant,
    ) -> VisionFfiResult<NnModelLoad> {
        let bytes = read_model_file(&model.model_path, "classifier")?;
        let model_sha256 = sha256_hex(&bytes);
        if model_sha256 != expected_model_sha256 {
            return Ok(NnModelLoad::Mismatch { model_sha256 });
        }
        let (facts, waited_on_others) = runtime_facts(&self.runtime, wait_deadline)?;
        let provider = nn_session_provider(self.runtime.execution_provider, facts)?;
        let session = nn::load_session(&bytes, &model.model_path, provider).map_err(failure)?;
        Ok(NnModelLoad::Loaded {
            engine: Box::new(nn::OnnxClassifyModel::new(session)),
            model_sha256,
            waited_on_others,
        })
    }
}

/// The provider of a classifier session: CUDA on the resolved device for `cuda`, else the CPU.
/// Workflow #407 GPU-1 B2 (design §9 Q7): a classifier runs on the CPU under `directml` too,
/// whatever the process's OCR route is, and that is no fallback and has no warning; its node
/// placement has no report to go to.
fn nn_session_provider(
    requested: OnnxExecutionProvider,
    facts: &RuntimeFacts,
) -> VisionFfiResult<nn::NnSessionProvider> {
    match requested {
        OnnxExecutionProvider::Cpu | OnnxExecutionProvider::Directml => {
            Ok(nn::NnSessionProvider::Cpu)
        }
        OnnxExecutionProvider::Cuda => {
            let device = facts
                .resolved_cuda_device
                .as_ref()
                .ok_or_else(|| failure("a CUDA classifier session has no resolved CUDA device"))?;
            let device_id = i32::try_from(device.ordinal)
                .map_err(|_| failure("the resolved CUDA ordinal exceeds the i32 range"))?;
            Ok(nn::NnSessionProvider::Cuda { device_id })
        }
    }
}

/// Fails unless a static recognizer class count is the dictionary size plus the CTC blank,
/// optionally plus the space class; a dynamic count is checked when the output is decoded.
fn require_recognizer_classes(
    session: &Session,
    dictionary_entries: usize,
    model_ref: &str,
) -> Result<(), String> {
    let Some(output) = session.outputs().first() else {
        return Err("PPOCR recognizer model has no outputs".to_string());
    };
    let ValueType::Tensor { shape, .. } = output.dtype() else {
        return Err(format!(
            "PPOCR recognizer output {} is not a tensor",
            output.name()
        ));
    };
    match shape.last().map(|classes| usize::try_from(*classes)) {
        Some(Ok(classes)) if classes > 0 => check_class_count(classes, dictionary_entries)
            .map_err(|message| format!("OCR model '{model_ref}': {message}")),
        _ => Ok(()),
    }
}

fn check_class_count(classes: usize, dictionary_entries: usize) -> Result<(), String> {
    if classes == dictionary_entries + 1 || classes == dictionary_entries + 2 {
        Ok(())
    } else {
        Err(format!(
            "the recognizer has {classes} output classes but the dictionary keys.txt has {dictionary_entries} entries; expected {} or {}",
            dictionary_entries + 1,
            dictionary_entries + 2
        ))
    }
}

/// One loaded `ppocr-ctc` model: the recognizer session, the verified dictionary and, after
/// the first full-frame request, the detector session.
struct PpocrCtcModel {
    /// The sessions, the route they run on and the model's execution status.
    sessions: ModelSessions<Session>,
    cuda_driver_version: Option<u32>,
    dictionary: Vec<String>,
    /// The effective preprocessing and decoding parameters of the model's description.
    params: PpocrCtcParams,
    /// Workflow #407 GPU-1 B2: the recognizer file and the hash verified when the model was
    /// loaded, from which a detector's fallback rebuilds the recognizer on the CPU.
    recognizer_path: PathBuf,
    recognizer_sha256: String,
    detector_path: Option<PathBuf>,
    detector_sha256: Option<String>,
    /// Session-load diagnostics, returned with the next request's result or error.
    pending_diagnostics: PpocrDiagnostics,
    /// Workflow #407 GPU-1 E: the model's GPU-run history, which outlives this engine.
    gpu_runs: GpuRuns,
}

/// Workflow #407 GPU-1 B2: one model file a session is built from: its verified bytes and its
/// path (for messages).
#[derive(Debug, Clone, Copy)]
struct ModelInput<'a> {
    bytes: &'a [u8],
    path: &'a Path,
}

impl<'a> ModelInput<'a> {
    /// The build of this file in `role`, on `adapter` for a DirectML session.
    fn request(
        self,
        role: PpocrModelRole,
        adapter: Option<&'a ResolvedDirectmlAdapter>,
    ) -> SessionRequest<'a> {
        SessionRequest {
            model: self.bytes,
            path: self.path,
            role,
            adapter,
        }
    }
}

/// Workflow #407 GPU-1 B2: what the route of one model is derived from: its session identity, the
/// key parts its configuration requests (no resolved DirectML adapter) and, for `directml`, what
/// a DirectML failure does.
#[derive(Debug, Clone)]
struct RouteBasis {
    session_id: OcrSessionId,
    parts: OcrSessionKeyParts,
    /// `Some` exactly for a `directml` configuration.
    gpu_fallback: Option<GpuFallback>,
}

/// Workflow #407 GPU-1 B2: the route one model's sessions run on: their binding and plan, the
/// adapter of a DirectML route, and where the model runs for the Runtime's status.
#[derive(Debug, Clone)]
struct ModelRoute {
    binding: OcrSessionBinding,
    plan: ProviderSessionPlan,
    adapter: Option<ResolvedDirectmlAdapter>,
    execution: OcrModelExecution,
}

impl RouteBasis {
    /// The route at session `generation`: on `adapter` for a DirectML route; without one, on the
    /// configured provider, or for `directml` on the CPU after the fallback whose warning is
    /// `warning` (every fallback has one).
    fn route(
        &self,
        generation: u64,
        adapter: Option<ResolvedDirectmlAdapter>,
        warning: Option<Outcome>,
    ) -> VisionFfiResult<ModelRoute> {
        if self.gpu_fallback.is_some() && adapter.is_none() && warning.is_none() {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Internal,
                ENGINE_MODULE,
                "a DirectML model's CPU route has no fallback warning",
            ));
        }
        let mut parts = self.parts.clone();
        parts.resolved_directml_device = adapter.as_ref().map(ResolvedDirectmlAdapter::device);
        let binding = OcrSessionBinding::new(
            self.session_id.clone(),
            generation,
            OcrSessionKey::from_parts(parts)?,
        );
        binding.validate()?;
        let plan = ProviderSessionPlan::from_key(binding.key()).map_err(failure)?;
        let execution = OcrModelExecution {
            requested: self.parts.requested_backend,
            actual: plan.resolved_execution_provider,
            warning,
        };
        Ok(ModelRoute {
            binding,
            plan,
            adapter,
            execution,
        })
    }

    /// What `failure` of a session build does (design §2.3): a DirectML failure under
    /// `gpu_fallback` `cpu` moves the whole model to the CPU, at session `generation`, with one
    /// warning that names the model; under `refuse`, and for a failure that names no DirectML
    /// reason (a CPU or CUDA session, or one of the CPU route), the failure stands and fails the
    /// request, and the next request tries again.
    fn fallback(
        &self,
        generation: u64,
        failure: SessionBuildFailure,
    ) -> VisionFfiResult<ModelRoute> {
        let warning = match self.gpu_fallback {
            Some(GpuFallback::Cpu) => {
                failure.warning(GpuFallback::Cpu, Some(self.parts.model_ref.as_str()))
            }
            Some(GpuFallback::Refuse) | None => None,
        };
        match warning {
            Some(warning) => self.route(generation, None, Some(warning)),
            None => Err(failure.into_error()),
        }
    }
}

/// The route a model load starts on (design §1.4, §2.3): the configured provider for `cpu` and
/// `cuda`. For `directml`: the CPU when the process fell back at its runtime initialisation,
/// with that fallback's one warning; else DirectML on the adapter this build resolves, whose
/// identity must be the first load's. An adapter that does not resolve is a session that cannot
/// be built ([`RouteBasis::fallback`]).
fn initial_route(
    basis: &RouteBasis,
    process: Option<&ProcessRoute>,
    adapters: &dyn AdapterSource,
) -> VisionFfiResult<ModelRoute> {
    match (process, basis.parts.requested_directml_device.as_ref()) {
        (None, None) => basis.route(1, None, None),
        (Some(ProcessRoute::Cpu(warning)), Some(_)) => basis.route(1, None, Some(warning.clone())),
        (Some(ProcessRoute::Directml(first)), Some(selector)) => {
            match resolve_session_adapter(adapters, selector, Some(first.identity.as_str()), None) {
                Ok(adapter) => basis.route(1, Some(adapter), None),
                Err(unusable) => basis.fallback(1, unusable),
            }
        }
        (Some(_), None) | (None, Some(_)) => Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::Internal,
            ENGINE_MODULE,
            "the process's DirectML route and the OCR model's configuration disagree",
        )),
    }
}

/// Workflow #407 GPU-1 E (design §7 row E, §9 Q8): the route of one model load. A model whose
/// first run after a rebuild failed too under `gpu_fallback` `cpu` (`gpu_runs`) runs whole on the
/// CPU, with that one warning, until the process ends; every other load starts on
/// [`initial_route`], so a load after a failed run (the rebuild) resolves its adapter again and
/// falls back as B2 does.
fn load_route(
    basis: &RouteBasis,
    process: Option<&ProcessRoute>,
    adapters: &dyn AdapterSource,
    gpu_runs: &GpuRuns,
) -> VisionFfiResult<ModelRoute> {
    match gpu_runs.cpu_warning() {
        Some(warning) => basis.route(1, None, Some(warning)),
        None => initial_route(basis, process, adapters),
    }
}

/// Workflow #407 GPU-1 E (design §7 row E, §9 Q8): what the DirectML runs of one model showed in
/// this process. It outlives the model's engine, which the kernel unloads when a DirectML run
/// fails, so the model's next load reads it.
#[derive(Debug, Clone, Default)]
enum GpuRunHistory {
    /// No DirectML run of the model failed since its last invocation on DirectML succeeded.
    #[default]
    Sound,
    /// A DirectML run failed and the engine was unloaded: the next load builds the model again,
    /// and the first invocation of that build decides.
    Rebuilt,
    /// The first invocation after a rebuild failed too under `gpu_fallback` `cpu`: every load of
    /// the model runs it whole on the CPU until the process ends, with this one warning.
    OnCpu(Outcome),
}

/// What a failed DirectML run leaves for the model's next load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterGpuRunFailure {
    /// The next load builds the model again on its configured route.
    BuildAgain,
    /// The next load, and every later one, runs the model on the CPU.
    CpuUntilRestart,
}

/// Workflow #407 GPU-1 E: one model's [`GpuRunHistory`], shared by its engines and the process.
#[derive(Debug, Clone, Default)]
struct GpuRuns(Arc<Mutex<GpuRunHistory>>);

impl GpuRuns {
    fn history(&self) -> MutexGuard<'_, GpuRunHistory> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The one warning of a model that runs on the CPU until the process ends, if it does.
    fn cpu_warning(&self) -> Option<Outcome> {
        match &*self.history() {
            GpuRunHistory::OnCpu(warning) => Some(warning.clone()),
            GpuRunHistory::Sound | GpuRunHistory::Rebuilt => None,
        }
    }

    /// An invocation on DirectML succeeded: a rebuilt model is sound again, so a later failed run
    /// builds it again once more.
    fn succeeded(&self) {
        let mut history = self.history();
        if matches!(*history, GpuRunHistory::Rebuilt) {
            *history = GpuRunHistory::Sound;
        }
    }

    /// A DirectML run failed. A sound model is built again; a rebuilt one whose first invocation
    /// this was runs on the CPU from now on under `gpu_fallback` `cpu`, with `warning`, and is
    /// built again under `refuse`.
    fn failed(
        &self,
        gpu_fallback: Option<GpuFallback>,
        warning: Option<Outcome>,
    ) -> AfterGpuRunFailure {
        let mut history = self.history();
        let next = match (&*history, gpu_fallback, warning) {
            (GpuRunHistory::OnCpu(warning), _, _) => GpuRunHistory::OnCpu(warning.clone()),
            (GpuRunHistory::Rebuilt, Some(GpuFallback::Cpu), Some(warning)) => {
                GpuRunHistory::OnCpu(warning)
            }
            _ => GpuRunHistory::Rebuilt,
        };
        let after = match next {
            GpuRunHistory::OnCpu(_) => AfterGpuRunFailure::CpuUntilRestart,
            GpuRunHistory::Sound | GpuRunHistory::Rebuilt => AfterGpuRunFailure::BuildAgain,
        };
        *history = next;
        after
    }
}

/// Workflow #407 GPU-1 E: the process's [`GpuRuns`] of one model, by its reference and content.
fn model_gpu_runs(model_ref: &str, model_sha256: &str) -> GpuRuns {
    GPU_RUNS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry((model_ref.to_string(), model_sha256.to_string()))
        .or_default()
        .clone()
}

/// Workflow #407 GPU-1 B2: the reports of sessions a fallback replaced are dropped. After a
/// whole-model fallback no session of the model runs on DirectML, so every DirectML report this
/// call still holds is of a replaced session; the sessions in use (CPU) read no placement.
fn drop_replaced_reports(diagnostics: &mut PpocrDiagnostics) {
    diagnostics
        .retain(|report| report.accelerated_execution_provider != DIRECTML_EXECUTION_PROVIDER);
}

/// Workflow #407 GPU-1 B2: the sessions of one model on its route and the model's live execution
/// status; generic over the session so the logic tests build them from a fake ONNX Runtime.
struct ModelSessions<S> {
    basis: RouteBasis,
    route: ModelRoute,
    recognizer: BuiltSession<S>,
    /// Built on the first full-frame request.
    detector: Option<BuiltSession<S>>,
    node_placement_diagnostic: Option<String>,
    status: OcrExecutionStatus,
}

impl<S> ModelSessions<S> {
    /// Loads the model's recognizer on `route` (design §2.3). A DirectML recognizer that cannot be
    /// built under `gpu_fallback` `cpu` moves the model to the CPU once: the failed attempt's
    /// report is dropped, the recognizer is built on the CPU and the status carries the
    /// fallback's one warning. Under `refuse` the load fails.
    fn load<B: SessionBuilds<Session = S>>(
        builds: &mut B,
        basis: RouteBasis,
        route: ModelRoute,
        recognizer: ModelInput<'_>,
        node_placement_diagnostic: Option<String>,
        diagnostics: &mut PpocrDiagnostics,
    ) -> VisionFfiResult<Self> {
        let role = PpocrModelRole::Recognizer;
        let npd = node_placement_diagnostic.as_deref();
        let first = load_session_with(
            builds,
            &recognizer.request(role, route.adapter.as_ref()),
            &route.plan,
            npd,
            diagnostics,
        );
        let (route, built) = match first {
            Ok(built) => (route, built),
            Err(unusable) => {
                let route = basis.fallback(route.binding.generation(), unusable)?;
                drop_replaced_reports(diagnostics);
                let built = load_session_with(
                    builds,
                    &recognizer.request(role, None),
                    &route.plan,
                    npd,
                    diagnostics,
                )
                .map_err(|cpu| cpu.of_cpu_fallback().into_error())?;
                (route, built)
            }
        };
        let status = OcrExecutionStatus::new(route.execution.clone());
        Ok(Self {
            basis,
            route,
            recognizer: built,
            detector: None,
            node_placement_diagnostic,
            status,
        })
    }

    /// Builds the detector on the model's route, on the first full-frame request (design §1.4,
    /// §2.3). A DirectML detector resolves its adapter again and must find it at the model's
    /// index. A DirectML detector that cannot be built under `gpu_fallback` `cpu` moves the whole
    /// model to the CPU: the recognizer is rebuilt on the CPU from its verified file
    /// (`read_recognizer`), the detector is built on the CPU, every report of the replaced
    /// DirectML sessions still in `diagnostics` is dropped, the session generation steps, and the
    /// status records the fallback's one warning. Nothing changes until both CPU sessions are
    /// built. Under `refuse` the request fails and the next one tries again.
    fn build_detector<B: SessionBuilds<Session = S>>(
        &mut self,
        builds: &mut B,
        detector: ModelInput<'_>,
        adapters: &dyn AdapterSource,
        read_recognizer: impl FnOnce() -> VisionFfiResult<(Vec<u8>, PathBuf)>,
        diagnostics: &mut PpocrDiagnostics,
    ) -> VisionFfiResult<()> {
        let role = PpocrModelRole::Detector;
        let npd = self.node_placement_diagnostic.as_deref();
        let key = self.route.binding.key();
        let attempt = match (
            key.requested_directml_device(),
            key.resolved_directml_device(),
        ) {
            (Some(selector), Some(bound)) => {
                match resolve_session_adapter(adapters, selector, None, Some(bound)) {
                    Ok(adapter) => load_session_with(
                        builds,
                        &detector.request(role, Some(&adapter)),
                        &self.route.plan,
                        npd,
                        diagnostics,
                    ),
                    Err(unusable) => Err(unusable),
                }
            }
            _ => load_session_with(
                builds,
                &detector.request(role, None),
                &self.route.plan,
                npd,
                diagnostics,
            ),
        };
        let unusable = match attempt {
            Ok(built) => {
                self.detector = Some(built);
                return Ok(());
            }
            Err(unusable) => unusable,
        };
        let generation = self.route.binding.generation().saturating_add(1);
        let route = self.basis.fallback(generation, unusable)?;
        drop_replaced_reports(diagnostics);
        let (recognizer_bytes, recognizer_path) = read_recognizer()?;
        let recognizer_input = ModelInput {
            bytes: &recognizer_bytes,
            path: &recognizer_path,
        };
        let recognizer = load_session_with(
            builds,
            &recognizer_input.request(PpocrModelRole::Recognizer, None),
            &route.plan,
            npd,
            diagnostics,
        )
        .map_err(|cpu| cpu.of_cpu_fallback().into_error())?;
        let detector = load_session_with(
            builds,
            &detector.request(role, None),
            &route.plan,
            npd,
            diagnostics,
        )
        .map_err(|cpu| cpu.of_cpu_fallback().into_error())?;
        self.status.set(route.execution.clone());
        self.route = route;
        self.recognizer = recognizer;
        self.detector = Some(detector);
        Ok(())
    }

    /// The CPU-placed node count of each built session whose node placement was read, for the
    /// attestation's `fallback_observed`.
    fn placements(&self) -> RolePlacements {
        RolePlacements {
            recognizer: self.recognizer.cpu_nodes,
            detector: self
                .detector
                .as_ref()
                .and_then(|detector| detector.cpu_nodes),
        }
    }

    /// Workflow #407 GPU-1 E (design §7 row E, §9 Q8): an invocation of the model ran and
    /// succeeded; on DirectML, a rebuilt model is sound again (`gpu_runs`).
    fn invocation_succeeded(&self, gpu_runs: &GpuRuns) {
        if self.route.plan.resolved_execution_provider == OnnxExecutionProvider::Directml {
            gpu_runs.succeeded();
        }
    }

    /// Workflow #407 GPU-1 E (design §7 row E, §9 Q8): the engine's error of an invocation that
    /// failed with `error`. A failed run of a session on DirectML (ONNX Runtime's error, not the
    /// watchdog's timeout: the device was removed or reset) is `ProviderUnavailable`, `DirectML
    /// cannot be used (inference_failed)`, on which the kernel unloads the engine instead of
    /// keeping it; the model's next request builds it again ([`load_route`]). `gpu_runs` records
    /// it: when the first invocation after that rebuild fails the same way under `gpu_fallback`
    /// `cpu`, the model runs whole on the CPU from its next load until the process ends, with one
    /// warning that names it and carries ONNX Runtime's status as its link; under `refuse` every
    /// such failure builds the model again. Every other failure, and every failure of a model on
    /// the CPU or on CUDA, is the engine's error as before.
    fn invocation_failed(&self, gpu_runs: &GpuRuns, error: ProviderInvokeError) -> VisionFfiError {
        let run = match error {
            ProviderInvokeError::Run(run)
                if self.route.plan.resolved_execution_provider
                    == OnnxExecutionProvider::Directml =>
            {
                run
            }
            error => return error.into_ffi_error(),
        };
        let mut unusable = SessionBuildFailure {
            reason: Some(GpuUnavailableReason::InferenceFailed),
            message: run.message,
            os_cause: None,
            ort_status: Some(run.ort_status),
        };
        let warning = unusable.warning(GpuFallback::Cpu, Some(self.basis.parts.model_ref.as_str()));
        let next = match gpu_runs.failed(self.basis.gpu_fallback, warning) {
            AfterGpuRunFailure::BuildAgain => {
                "the engine is unloaded and the model's next request builds it again"
            }
            AfterGpuRunFailure::CpuUntilRestart => {
                "the first run after the model was built again failed too, so from its next load it runs on the CPU until the process restarts"
            }
        };
        unusable.message = format!("{}; {next}", unusable.message);
        unusable.into_error()
    }
}

/// How many nodes of each role's session ONNX Runtime placed on the CPU, when the session's node
/// placement was read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RolePlacements {
    recognizer: Option<usize>,
    detector: Option<usize>,
}

/// The roles whose sessions ran in one invocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct InvocationRoles {
    recognizer: bool,
    detector: bool,
}

/// The attestation's `fallback_observed` (Workflow #407 GPU-1 design §2.2, §2.4): none for a plan
/// that admits no CPU node (CPU, CUDA); for a DirectML plan, whether a role whose session ran in
/// the invocation has a node ONNX Runtime placed on the CPU. A region read runs the recognizer; a
/// full-frame read runs the detector and, for each box it keeps, the recognizer. A DirectML
/// session that ran without a read placement fails: every DirectML session reads it. Workflow
/// #407 GPU-1 B2: a model that fell back from DirectML runs whole on the CPU, so it observes the
/// fallback in every invocation.
fn fallback_observed(
    plan: &ProviderSessionPlan,
    placements: RolePlacements,
    ran: InvocationRoles,
) -> VisionFfiResult<Option<bool>> {
    match plan.fallback_policy {
        OcrFallbackPolicy::Forbidden => return Ok(None),
        OcrFallbackPolicy::CpuModel => return Ok(Some(true)),
        OcrFallbackPolicy::CpuNodes => {}
    }
    let mut observed = false;
    for (role, role_ran, cpu_nodes) in [
        (
            PpocrModelRole::Recognizer,
            ran.recognizer,
            placements.recognizer,
        ),
        (PpocrModelRole::Detector, ran.detector, placements.detector),
    ] {
        if !role_ran {
            continue;
        }
        let cpu_nodes = cpu_nodes.ok_or_else(|| {
            failure(format!(
                "the PPOCR {} session ran without a read node placement",
                role.as_str()
            ))
        })?;
        observed |= cpu_nodes > 0;
    }
    Ok(Some(observed))
}

impl OcrEngine for PpocrCtcModel {
    fn read_text(&mut self, request: OcrInferenceRequest) -> VisionFfiResult<OcrInferenceResult> {
        self.read_text_view(request.view())
    }

    fn read_text_with_attestation(
        &mut self,
        request: OcrInferenceRequest,
    ) -> VisionFfiResult<OcrInferenceOutput> {
        self.read_text_with_attestation_view(request.view())
    }

    fn read_text_view(
        &mut self,
        request: OcrInferenceRequestView<'_>,
    ) -> VisionFfiResult<OcrInferenceResult> {
        self.read_text_with_attestation_view(request)
            .map(|output| output.result)
    }

    fn read_text_with_attestation_view(
        &mut self,
        request: OcrInferenceRequestView<'_>,
    ) -> VisionFfiResult<OcrInferenceOutput> {
        request.validate()?;
        let invocation_id = next_ocr_invocation_id()?;
        let mut diagnostics = std::mem::take(&mut self.pending_diagnostics);
        let mut ran = InvocationRoles::default();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.read_region(request, &mut diagnostics, &mut ran)
        }));
        let mut result = match outcome {
            Ok(Ok(result)) => {
                // Workflow #407 GPU-1 E: the model's sessions ran.
                self.sessions.invocation_succeeded(&self.gpu_runs);
                result
            }
            Ok(Err(error)) => {
                // Workflow #407 GPU-1 E: a failed DirectML run unloads the engine.
                let error = self.sessions.invocation_failed(&self.gpu_runs, error);
                return Err(error.with_ppocr_diagnostics(diagnostics));
            }
            Err(_) => {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::ProviderPanic,
                    ENGINE_MODULE,
                    "OCR engine panicked while reading text",
                )
                .with_ppocr_diagnostics(diagnostics));
            }
        };
        validate_ppocr_call_diagnostics(&diagnostics).map_err(|error| {
            failure(error.to_string()).with_ppocr_diagnostics(diagnostics.clone())
        })?;
        result.ppocr_diagnostics = diagnostics;
        result
            .validate_for(request)
            .map_err(|error| error.with_ppocr_diagnostics(result.ppocr_diagnostics.clone()))?;
        let attestation = self
            .attestation(invocation_id, ran)
            .map_err(|error| error.with_ppocr_diagnostics(result.ppocr_diagnostics.clone()))?;
        Ok(OcrInferenceOutput {
            execution_attestation: Some(attestation),
            result,
        })
    }
}

impl PpocrCtcModel {
    /// Reads the request; `ran` records the roles whose sessions ran.
    fn read_region(
        &mut self,
        request: OcrInferenceRequestView<'_>,
        diagnostics: &mut PpocrDiagnostics,
        ran: &mut InvocationRoles,
    ) -> Result<OcrInferenceResult, ProviderInvokeError> {
        let inference_deadline = Instant::now()
            .checked_add(Duration::from_millis(request.timeout_ms))
            .ok_or_else(|| {
                ProviderInvokeError::from("PPOCR inference deadline overflowed".to_string())
            })?;
        let frame = &request.frame;
        let merge_gap_factor = match read_route(
            frame,
            request.region,
            request.blocks,
            self.params.detector.merge_gap_factor,
        ) {
            ReadRoute::Region => {
                ran.recognizer = true;
                let decoded = recognize_region(
                    &mut self.sessions.recognizer.session,
                    &self.dictionary,
                    &self.params,
                    frame,
                    request.region,
                    remaining_inference_budget(inference_deadline, "PPOCR recognizer")?,
                )?;
                return Ok(canonical_roi_result(decoded, request.region));
            }
            ReadRoute::Detect { merge_gap_factor } => merge_gap_factor,
        };
        if self.sessions.detector.is_none() {
            self.load_detector(diagnostics)?;
        }
        let detector = &mut self
            .sessions
            .detector
            .as_mut()
            .ok_or_else(|| "PPOCR detector session is unavailable".to_string())?
            .session;
        ran.detector = true;
        let detected = detect_text_regions(
            detector,
            &self.params,
            frame,
            request.region,
            merge_gap_factor,
            remaining_inference_budget(inference_deadline, "PPOCR detector")?,
        )?;
        let (detected, count) = keep_detected_boxes(detected, self.params.detector.max_boxes);
        let mut blocks = Vec::new();
        for detected_box in &detected {
            ran.recognizer = true;
            let decoded = recognize_region(
                &mut self.sessions.recognizer.session,
                &self.dictionary,
                &self.params,
                frame,
                detected_box.rect,
                remaining_inference_budget(inference_deadline, "PPOCR recognizer")?,
            )?;
            if !decoded.text.is_empty() {
                blocks.push(OcrTextBlock {
                    text: decoded.text,
                    rect: detected_box.rect,
                    confidence: decoded.confidence.or(Some(detected_box.confidence)),
                });
            }
        }
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let confidence = average_confidence(blocks.iter().filter_map(|block| block.confidence));
        Ok(OcrInferenceResult {
            ppocr_diagnostics: Vec::new(),
            text,
            blocks,
            confidence,
            backend: VisionBackendKind::FastDeployPpocr,
            warnings: Vec::new(),
            // Only a request that chose `Detect` reports its count; an implicit full-frame
            // read keeps its result as before (Workflow #396 REC-1).
            detection: matches!(request.blocks, OcrBlockMode::Detect { .. }).then_some(count),
        })
    }

    /// Builds the detector session on the first full-frame request, from bytes whose hash
    /// equals the detector hash verified when the model was loaded, on the model's route
    /// ([`ModelSessions::build_detector`]): a DirectML detector resolves its adapter again and
    /// must find it at the model's index (design §1.4), and one that cannot be built under
    /// `gpu_fallback` `cpu` moves the whole model to the CPU (design §2.3).
    fn load_detector(&mut self, diagnostics: &mut PpocrDiagnostics) -> VisionFfiResult<()> {
        let (Some(path), Some(expected)) = (&self.detector_path, &self.detector_sha256) else {
            return Err(failure(format!(
                "OCR model '{}' has no detector; a full_frame region needs det.onnx",
                self.sessions.route.binding.key().model_ref()
            )));
        };
        let bytes = read_verified_model_file(path, expected, PpocrModelRole::Detector)?;
        let (recognizer_path, recognizer_sha256) = (&self.recognizer_path, &self.recognizer_sha256);
        self.sessions.build_detector(
            &mut OrtSessions,
            ModelInput {
                bytes: &bytes,
                path: path.as_path(),
            },
            &SystemAdapterSource,
            || {
                read_verified_model_file(
                    recognizer_path,
                    recognizer_sha256,
                    PpocrModelRole::Recognizer,
                )
                .map(|bytes| (bytes, recognizer_path.clone()))
            },
            diagnostics,
        )
    }

    fn attestation(
        &self,
        invocation_id: OcrInvocationId,
        ran: InvocationRoles,
    ) -> VisionFfiResult<OcrExecutionAttestation> {
        let route = &self.sessions.route;
        let key = route.binding.key();
        Ok(OcrExecutionAttestation {
            schema_version: OCR_EXECUTION_ATTESTATION_SCHEMA_VERSION.to_string(),
            invocation_id,
            session: route.binding.clone(),
            resolved_execution_provider: route.plan.resolved_execution_provider,
            provider: OcrProviderBuildIdentity {
                implementation: ENGINE_IMPLEMENTATION.to_string(),
                crate_version: env!("CARGO_PKG_VERSION").to_string(),
                build_git_sha: None,
                binary_sha256: key.provider_library_sha256().to_string(),
            },
            runtime: OcrRuntimeBuildIdentity {
                onnxruntime_version: key.onnxruntime_version().to_string(),
                onnxruntime_build_info: ort::info().to_string(),
                cuda_driver_version: self.cuda_driver_version,
                cuda_runtime_version: None,
                cudnn_version: None,
            },
            registered_execution_providers: route.plan.registered_execution_providers.clone(),
            cpu_ep_registered: route.plan.cpu_ep_registered,
            cpu_fallback_disabled: route.plan.cpu_fallback_disabled,
            fallback_policy: route.plan.fallback_policy,
            fallback_observed: fallback_observed(&route.plan, self.sessions.placements(), ran)?,
            complete: true,
        })
    }
}

/// A model file read again for a session built after the load, which must still hash to what
/// the load verified.
fn read_verified_model_file(
    path: &Path,
    expected: &str,
    role: PpocrModelRole,
) -> VisionFfiResult<Vec<u8>> {
    let bytes = std::fs::read(path).map_err(|error| {
        failure(format!(
            "failed to read the OCR {} file {}: {error}",
            role.as_str(),
            path.display()
        ))
    })?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(failure(format!(
            "OCR {} file {} changed since the model was verified: expected={expected}, actual={actual}; restart is required",
            role.as_str(),
            path.display()
        )));
    }
    Ok(bytes)
}

/// Initialises ONNX Runtime once per process. A request waiting for another request's
/// initialisation waits only until its own deadline.
/// The process runtime facts, initialising ONNX Runtime on first use, and how long this call
/// waited for another request's initialisation (Workflow #360 §6: that wait is charged to the
/// request's budget; this request's own initialisation is not).
fn runtime_facts(
    spec: &VisionRuntimeSpec,
    wait_deadline: Instant,
) -> VisionFfiResult<(&'static RuntimeFacts, Duration)> {
    if let Some(facts) = RUNTIME_FACTS.get() {
        return same_runtime(facts, spec).map(|facts| (facts, Duration::ZERO));
    }
    let waiting = Instant::now();
    let _initialising = lock_until(&RUNTIME_INIT, wait_deadline, "ONNX Runtime initialisation")?;
    let waited = waiting.elapsed();
    if let Some(facts) = RUNTIME_FACTS.get() {
        return same_runtime(facts, spec).map(|facts| (facts, waited));
    }
    let facts = establish_runtime(spec)?;
    Ok((RUNTIME_FACTS.get_or_init(move || facts), waited))
}

fn same_runtime(
    facts: &'static RuntimeFacts,
    spec: &VisionRuntimeSpec,
) -> VisionFfiResult<&'static RuntimeFacts> {
    if facts.onnxruntime_library == spec.onnxruntime_library {
        Ok(facts)
    } else {
        Err(unavailable(format!(
            "ONNX Runtime is already initialised from {}; refusing {}; restart is required",
            facts.onnxruntime_library.display(),
            spec.onnxruntime_library.display()
        )))
    }
}

fn establish_runtime(spec: &VisionRuntimeSpec) -> VisionFfiResult<RuntimeFacts> {
    if spec.execution_provider == OnnxExecutionProvider::Directml {
        return establish_directml_runtime(spec, &SystemAdapterSource);
    }
    let library = &spec.onnxruntime_library;
    require_onnxruntime_once(&spec.runtime_library_closure, library)?;
    let onnxruntime_sha256 = sha256_file_hex(ENGINE_MODULE, library)?;
    if let Some(expected) = &spec.expected_onnxruntime_sha256
        && *expected != onnxruntime_sha256
    {
        return Err(unavailable(format!(
            "ONNX Runtime library {} SHA-256 mismatch: expected={expected}, actual={onnxruntime_sha256}",
            library.display()
        )));
    }
    establish_process_runtime_library_closure(&spec.runtime_library_closure)?;
    let onnxruntime_version = onnxruntime_version_string(library)?;
    ORT_RUNTIME.ensure(library).map_err(unavailable)?;
    let executable = std::env::current_exe().map_err(|error| {
        unavailable(format!("the running executable cannot be located: {error}"))
    })?;
    let executable_sha256 = sha256_file_hex(ENGINE_MODULE, &executable)?;
    let (resolved_cuda_device, cuda_driver_version) = match spec.execution_provider {
        OnnxExecutionProvider::Cpu => {
            if spec.cuda_device.is_some() {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidRequest,
                    ENGINE_MODULE,
                    "a CPU OCR configuration must not include a CUDA device selector",
                ));
            }
            (None, None)
        }
        OnnxExecutionProvider::Cuda => {
            let selector = spec.cuda_device.as_ref().ok_or_else(|| {
                VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidRequest,
                    ENGINE_MODULE,
                    "a CUDA OCR configuration requires an explicit ordinal and stable identity",
                )
            })?;
            let inventory = enumerate_cuda_devices()?;
            (
                Some(inventory.resolve(selector)?),
                Some(inventory.driver_version),
            )
        }
        OnnxExecutionProvider::Directml => {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Internal,
                ENGINE_MODULE,
                "a DirectML runtime is initialised by its own path",
            ));
        }
    };
    Ok(RuntimeFacts {
        onnxruntime_library: library.clone(),
        onnxruntime_version,
        onnxruntime_sha256,
        executable_sha256,
        resolved_cuda_device,
        cuda_driver_version,
        directml: None,
    })
}

fn require_onnxruntime_once(closure: &[PathBuf], library: &Path) -> VisionFfiResult<()> {
    if closure.iter().filter(|path| *path == library).count() == 1 {
        Ok(())
    } else {
        Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidRequest,
            ENGINE_MODULE,
            "the ONNX Runtime library must occur exactly once in its runtime-library closure",
        ))
    }
}

/// Workflow #407 GPU-1 B1 (design §3): a DirectML runtime initialises in this order.
/// 1. The pin: `onnxruntime.dll` and `onnxruntime_providers_shared.dll` must be the pinned files
///    (`ProviderUnavailable`), and so must `DirectML.dll` (`directml_library_missing`,
///    `directml_library_mismatch`).
/// 2. The adapter the configuration names (DXGI and the kernel's adapter type, from System32).
/// 3. The closure `[providers_shared, DirectML.dll, onnxruntime]`, by absolute path; the process
///    DLL search is not changed.
/// 4. The version ONNX Runtime reports, which must be the pin's.
/// 5. ONNX Runtime itself, once per process.
/// 6. The running executable's hash.
///
/// Workflow #407 GPU-1 B2 (design §1.4): `DirectML.dll` and the adapter decide the process's route
/// in [`directml_runtime_route`]; under `gpu_fallback` `cpu` a process that cannot use DirectML
/// loads the CPU closure in step 3 and runs every model on the CPU until it restarts. An ONNX
/// Runtime or providers library off the pin, or another version, fails whatever `gpu_fallback`
/// says.
fn establish_directml_runtime(
    spec: &VisionRuntimeSpec,
    adapters: &dyn AdapterSource,
) -> VisionFfiResult<RuntimeFacts> {
    let directml = spec.directml.as_ref().ok_or_else(|| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidRequest,
            ENGINE_MODULE,
            "a DirectML OCR runtime has no DirectML part: use VisionRuntimeSpec::from_vision_root_directml",
        )
    })?;
    if spec.cuda_device.is_some() {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidRequest,
            ENGINE_MODULE,
            "a DirectML OCR configuration must not include a CUDA device selector",
        ));
    }
    let library = &spec.onnxruntime_library;
    require_onnxruntime_once(&spec.runtime_library_closure, library)?;
    let pin = &DIRECTML_RUNTIME_PIN;
    let onnxruntime_sha256 = sha256_file_hex(ENGINE_MODULE, library)?;
    let expected = spec
        .expected_onnxruntime_sha256
        .as_deref()
        .unwrap_or(pin.onnxruntime.sha256);
    if onnxruntime_sha256 != pin.onnxruntime.sha256 || onnxruntime_sha256 != expected {
        return Err(unavailable(format!(
            "ONNX Runtime library {} is not the pinned DirectML build of ONNX Runtime {}: expected={}, actual={onnxruntime_sha256}",
            library.display(),
            pin.onnxruntime_version,
            pin.onnxruntime.sha256
        )));
    }
    for path in spec
        .runtime_library_closure
        .iter()
        .filter(|path| *path != library)
    {
        check_pinned_runtime_file(path)?;
    }
    let route = directml_runtime_route(spec, check_directml_library(&directml.library), adapters)?;
    require_onnxruntime_once(&route.closure, library)?;
    establish_process_runtime_library_closure(&route.closure)?;
    let onnxruntime_version = onnxruntime_version_string(library)?;
    if onnxruntime_version != pin.onnxruntime_version {
        return Err(unavailable(format!(
            "ONNX Runtime library {} reports version {onnxruntime_version}; the DirectML runtime pin is {}",
            library.display(),
            pin.onnxruntime_version
        )));
    }
    ORT_RUNTIME.ensure(library).map_err(unavailable)?;
    let executable = std::env::current_exe().map_err(|error| {
        unavailable(format!("the running executable cannot be located: {error}"))
    })?;
    let executable_sha256 = sha256_file_hex(ENGINE_MODULE, &executable)?;
    Ok(RuntimeFacts {
        onnxruntime_library: library.clone(),
        onnxruntime_version,
        onnxruntime_sha256,
        executable_sha256,
        resolved_cuda_device: None,
        cuda_driver_version: None,
        directml: Some(route.process),
    })
}

/// Workflow #407 GPU-1 B2: what a DirectML process loads before ONNX Runtime and what it runs.
#[derive(Debug, Clone)]
struct DirectmlRuntimeRoute {
    closure: Vec<PathBuf>,
    process: ProcessRoute,
}

/// Workflow #407 GPU-1 B2 (design §1.4, §3): the route of a DirectML process, decided once when its
/// runtime is initialised; `library` is `ort\DirectML.dll` against the pin. With the library
/// pinned and the configured adapter resolved, the closure loads `DirectML.dll` right before ONNX
/// Runtime and the process registers DirectML. Otherwise, under `gpu_fallback` `cpu`, the closure
/// is the CPU one, `[providers_shared, onnxruntime]` without `DirectML.dll`, and every model of
/// the process runs on the CPU until it restarts, with the one warning of that fallback; under
/// `refuse` the initialisation fails (an adapter or library reason is `ProviderUnavailable`) and
/// the next model load tries again. No other adapter is ever chosen.
fn directml_runtime_route(
    spec: &VisionRuntimeSpec,
    library: Result<(), SessionBuildFailure>,
    adapters: &dyn AdapterSource,
) -> VisionFfiResult<DirectmlRuntimeRoute> {
    let directml = spec.directml.as_ref().ok_or_else(|| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidRequest,
            ENGINE_MODULE,
            "a DirectML OCR runtime has no DirectML part",
        )
    })?;
    let usable = library.and_then(|()| {
        resolve_directml_adapter(adapters, &directml.device)
            .decision
            .map_err(SessionBuildFailure::from_unavailable)
    });
    let unusable = match usable {
        Ok(adapter) => {
            let closure = spec.directml_runtime_library_closure().ok_or_else(|| {
                VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidRequest,
                    ENGINE_MODULE,
                    "a DirectML OCR runtime has no DirectML closure",
                )
            })?;
            return Ok(DirectmlRuntimeRoute {
                closure,
                process: ProcessRoute::Directml(adapter),
            });
        }
        Err(unusable) => unusable,
    };
    let warning = match directml.gpu_fallback {
        GpuFallback::Cpu => unusable.warning(GpuFallback::Cpu, None),
        GpuFallback::Refuse => None,
    };
    match warning {
        Some(warning) => Ok(DirectmlRuntimeRoute {
            closure: spec.runtime_library_closure.clone(),
            process: ProcessRoute::Cpu(warning),
        }),
        None => Err(unusable.into_error()),
    }
}

/// A member of a DirectML runtime's CPU closure other than `onnxruntime.dll` against the pin:
/// `onnxruntime_providers_shared.dll` that is not the pinned file is `ProviderUnavailable`, with
/// the OS failure behind an unreadable one.
fn check_pinned_runtime_file(path: &Path) -> VisionFfiResult<()> {
    let pinned = &DIRECTML_RUNTIME_PIN.providers_shared;
    if path.file_name() != Some(OsStr::new(pinned.file_name)) {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidRequest,
            ENGINE_MODULE,
            format!("{} is no file of the DirectML runtime pin", path.display()),
        ));
    }
    let check = pinned.check(path);
    if check.state == FileState::Ok {
        return Ok(());
    }
    let error = unavailable(format!(
        "{} is not the pinned file of the DirectML runtime ({}; the pin is {} B with SHA-256 {})",
        path.display(),
        check.state,
        pinned.size,
        pinned.sha256
    ));
    Err(match check.os_cause {
        Some(cause) => error.with_os_cause(cause),
        None => error,
    })
}

/// The runtime's `ort\DirectML.dll` against the pin: `directml_library_missing` when absent,
/// `directml_library_mismatch` when it differs or cannot be read (with the OS failure).
fn check_directml_library(library: &Path) -> Result<(), SessionBuildFailure> {
    let pin = &DIRECTML_RUNTIME_PIN;
    let check = pin.directml.check(library);
    let unusable = match check.state {
        FileState::Ok => return Ok(()),
        FileState::Missing => SessionBuildFailure::directml(
            GpuUnavailableReason::DirectmlLibraryMissing,
            format!("{} is missing", library.display()),
        ),
        state => SessionBuildFailure::directml(
            GpuUnavailableReason::DirectmlLibraryMismatch,
            format!(
                "{} is not the pinned DirectML {} ({state}; the pin is {} B with SHA-256 {})",
                library.display(),
                pin.directml_version,
                pin.directml.size,
                pin.directml.sha256
            ),
        ),
    };
    Err(SessionBuildFailure {
        os_cause: check.os_cause,
        ..unusable
    })
}

/// Workflow #407 GPU-1 B1 (design §1.4): the adapter of one DirectML session build, resolved
/// again for this build. Its identity must be the one the first model load resolved
/// (`first_identity`), and a session of a model already loaded (`bound`) must find its adapter
/// at the model's index. The LUID is recorded, never compared.
fn resolve_session_adapter(
    source: &dyn AdapterSource,
    selector: &DirectmlDeviceSelector,
    first_identity: Option<&str>,
    bound: Option<&DirectmlDeviceIdentity>,
) -> Result<ResolvedDirectmlAdapter, SessionBuildFailure> {
    let adapter = resolve_directml_adapter(source, selector)
        .decision
        .map_err(SessionBuildFailure::from_unavailable)?;
    if let Some(first) = first_identity
        && adapter.identity != first
    {
        return Err(SessionBuildFailure::directml(
            GpuUnavailableReason::AdapterMissing,
            format!(
                "this build resolved the DirectML adapter {} but the first model load resolved {first}; restart is required",
                adapter.identity
            ),
        ));
    }
    if let Some(bound) = bound
        && (adapter.identity != bound.identity || adapter.dxgi_index != bound.dxgi_index)
    {
        return Err(SessionBuildFailure::directml(
            GpuUnavailableReason::AdapterMissing,
            format!(
                "the model's DirectML adapter {} at DXGI index {} resolved as {} at DXGI index {}; the model is bound to its index until it is loaded again",
                bound.identity, bound.dxgi_index, adapter.identity, adapter.dxgi_index
            ),
        ));
    }
    Ok(adapter)
}

fn lock_until<'a, T>(
    mutex: &'a Mutex<T>,
    deadline: Instant,
    what: &str,
) -> VisionFfiResult<MutexGuard<'a, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(_)) => {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::Internal,
                    ENGINE_MODULE,
                    format!("{what} lock is poisoned; restart is required"),
                ));
            }
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(VisionFfiError::fatal_with_code(
                        VisionFfiErrorCode::Timeout,
                        ENGINE_MODULE,
                        format!("waiting for {what} exceeded the request's timeout"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn read_model_file(path: &Path, role: &str) -> VisionFfiResult<Vec<u8>> {
    std::fs::read(path).map_err(|error| {
        unavailable(format!(
            "failed to read the OCR {role} file {}: {error}",
            path.display()
        ))
    })
}

fn parse_dictionary(bytes: &[u8], path: &Path) -> Result<Vec<String>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|err| format!("failed to read PPOCR dictionary {}: {err}", path.display()))?;
    let dictionary: Vec<_> = text.lines().map(ToOwned::to_owned).collect();
    if dictionary.is_empty() {
        return Err(format!("PPOCR dictionary {} is empty", path.display()));
    }
    Ok(dictionary)
}

fn unavailable(message: impl Into<String>) -> VisionFfiError {
    VisionFfiError::fatal_with_code(
        VisionFfiErrorCode::ProviderUnavailable,
        ENGINE_MODULE,
        message,
    )
}

fn failure(message: impl Into<String>) -> VisionFfiError {
    VisionFfiError::fatal_with_code(VisionFfiErrorCode::ProviderFailure, ENGINE_MODULE, message)
}

fn canonical_roi_result(decoded: DecodedText, region: VisionRect) -> OcrInferenceResult {
    let blocks = if decoded.text.is_empty() {
        Vec::new()
    } else {
        vec![OcrTextBlock {
            text: decoded.text.clone(),
            rect: region,
            confidence: decoded.confidence,
        }]
    };

    OcrInferenceResult {
        ppocr_diagnostics: Vec::new(),
        text: decoded.text,
        confidence: decoded.confidence,
        blocks,
        backend: VisionBackendKind::FastDeployPpocr,
        warnings: Vec::new(),
        detection: None,
    }
}

// Workflow #396 REC-1: the read route of a request, the line merge it names and the count of
// boxes the folder's `max_boxes` dropped.

/// What a request runs: the recognizer over the whole region, or the detector inside the
/// region followed by the recognizer on every box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadRoute {
    Region,
    /// `merge_gap_factor` replaces the folder's `detector.merge_gap_factor`; `None` merges no
    /// boxes.
    Detect {
        merge_gap_factor: Option<u32>,
    },
}

/// An implicit request keeps the rule it had before `blocks` existed: the detector for a
/// full-frame region only, merged by the folder's factor. A request that chose `Region` or
/// `Detect` gets exactly that, on any region.
fn read_route(
    frame: &VisionFrameView<'_>,
    region: VisionRect,
    blocks: OcrBlockMode,
    folder_gap_factor: u32,
) -> ReadRoute {
    match blocks {
        OcrBlockMode::Implicit if is_full_frame_region(frame, region) => ReadRoute::Detect {
            merge_gap_factor: Some(folder_gap_factor),
        },
        OcrBlockMode::Implicit | OcrBlockMode::Region => ReadRoute::Region,
        OcrBlockMode::Detect {
            line_merge: OcrLineMerge::Never,
        } => ReadRoute::Detect {
            merge_gap_factor: None,
        },
        OcrBlockMode::Detect {
            line_merge: OcrLineMerge::GapFactor(factor),
        } => ReadRoute::Detect {
            merge_gap_factor: Some(factor),
        },
    }
}

/// The first `max_boxes` boxes in reading order, and how many the detector found before that
/// cut.
fn keep_detected_boxes(
    mut boxes: Vec<DetectedTextBox>,
    max_boxes: u32,
) -> (Vec<DetectedTextBox>, OcrDetectionCount) {
    let count = OcrDetectionCount {
        detected: u32::try_from(boxes.len()).unwrap_or(u32::MAX),
        limit: max_boxes,
    };
    boxes.truncate(usize::try_from(max_boxes).unwrap_or(usize::MAX));
    (boxes, count)
}

/// Workflow #407 GPU-1 B1: why one session could not be built. A DirectML session's failure
/// names why DirectML cannot be used (design §2.3), the reason a fallback's warning carries
/// (GPU-1 B2). CPU and CUDA failures name none and keep their messages. Workflow #407 GPU-1 E:
/// also why a DirectML session's run failed (`inference_failed`).
#[derive(Debug, Clone)]
struct SessionBuildFailure {
    reason: Option<GpuUnavailableReason>,
    message: String,
    /// The OS failure behind the reason, when there is one (the adapter listing, the library).
    os_cause: Option<Outcome>,
    /// Workflow #407 GPU-1 B2: ONNX Runtime's status behind a DirectML failure, kept whole for
    /// the fallback warning's link (design §2.3).
    ort_status: Option<Outcome>,
}

impl SessionBuildFailure {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            reason: None,
            message: message.into(),
            os_cause: None,
            ort_status: None,
        }
    }

    fn directml(reason: GpuUnavailableReason, message: impl Into<String>) -> Self {
        Self {
            reason: Some(reason),
            ..Self::plain(message)
        }
    }

    /// One failed step of building a session with `options`: `reason` only for DirectML.
    fn step(options: &SessionOptions, reason: GpuUnavailableReason, message: String) -> Self {
        Self {
            reason: options.execution_provider.is_directml().then_some(reason),
            ..Self::plain(message)
        }
    }

    /// No adapter for a DirectML session, with the OS failure behind it.
    fn from_unavailable(unavailable: DirectmlUnavailable) -> Self {
        Self {
            reason: Some(unavailable.reason),
            message: unavailable.to_string(),
            os_cause: unavailable
                .cause
                .as_ref()
                .and_then(VisionFfiError::os_cause)
                .cloned(),
            ort_status: None,
        }
    }

    /// This failure with ONNX Runtime's status (its error code and its message with its causes)
    /// kept for a DirectML failure's warning (design §2.3, as GPU-0c's `OrtStepFailure` kept it).
    fn with_ort_status<R>(mut self, error: &ort::Error<R>) -> Self {
        if self.reason.is_some() {
            self.ort_status = Some(onnxruntime_error_status(error));
        }
        self
    }

    /// The same failure, of the zero-inference shadow that reads the node placement.
    fn of_placement_shadow(mut self) -> Self {
        self.message = format!(
            "the zero-inference node-placement session failed: {}",
            self.message
        );
        self
    }

    /// Workflow #407 GPU-1 B2: the failure of a CPU session that replaces DirectML after a
    /// fallback; it names no DirectML reason and fails the request.
    fn of_cpu_fallback(self) -> Self {
        Self::plain(format!(
            "DirectML could not be used and the CPU session that replaces it failed: {}",
            self.message
        ))
    }

    /// Workflow #407 GPU-1 B2 (design §2.5): the one warning `vision_gpu_unavailable` of falling
    /// back from this DirectML failure under `gpu_fallback`: its reason, `model_name` for one
    /// model's fallback (none for the process's), and ONNX Runtime's status or the OS failure
    /// behind the reason as its `caused_by` link. `None` for a failure that names no DirectML
    /// reason, which is no reason to fall back.
    fn warning(&self, gpu_fallback: GpuFallback, model_name: Option<&str>) -> Option<Outcome> {
        let reason = self.reason?;
        let warning = Outcome::new(VisionGpuCode::VisionGpuUnavailable)
            .execution_provider(OnnxExecutionProvider::Directml)
            .gpu_unavailable_reason(reason)
            .gpu_fallback(gpu_fallback);
        let warning = match model_name {
            Some(model_name) => warning.model_name(model_name),
            None => warning,
        };
        Some(match self.ort_status.as_ref().or(self.os_cause.as_ref()) {
            Some(link) => warning.caused_by(link.clone()),
            None => warning,
        })
    }

    /// The engine's error: an adapter or library reason is `ProviderUnavailable`, and so is a
    /// failed run (`inference_failed`, Workflow #407 GPU-1 E: the kernel unloads the engine on it);
    /// every other failure is `ProviderFailure`. A DirectML failure's message leads with its
    /// reason.
    fn into_error(self) -> VisionFfiError {
        let Some(reason) = self.reason else {
            return failure(self.message);
        };
        let message = format!("DirectML cannot be used ({reason}): {}", self.message);
        let error = match reason {
            GpuUnavailableReason::AdapterMissing
            | GpuUnavailableReason::AdapterAmbiguous
            | GpuUnavailableReason::AdapterNotPhysical
            | GpuUnavailableReason::DirectmlLibraryMissing
            | GpuUnavailableReason::DirectmlLibraryMismatch
            | GpuUnavailableReason::InferenceFailed => unavailable(message),
            GpuUnavailableReason::ProviderRegisterFailed
            | GpuUnavailableReason::SessionBuildFailed
            | GpuUnavailableReason::PlacementUnread => failure(message),
        };
        match self.os_cause {
            Some(cause) => error.with_os_cause(cause),
            None => error,
        }
    }
}

/// ONNX Runtime's status of `error`: its error code and its message with its causes, kept whole
/// for a DirectML failure's warning (design §2.3, as GPU-0c's `OrtStepFailure` kept it).
fn onnxruntime_error_status<R>(error: &ort::Error<R>) -> Outcome {
    let mut message = error.message().to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str("; caused by: ");
        message.push_str(&cause.to_string());
        source = std::error::Error::source(cause);
    }
    onnxruntime_status(&error.code(), &message)
}

/// Builds ONNX Runtime sessions from [`SessionOptions`]: [`OrtSessions`] in the engine; the logic
/// tests give a fake that writes ONNX Runtime's log lines into the placement logger it is given.
trait SessionBuilds {
    type Session;

    /// Builds a session of `model` with exactly `options`. With `placement_logger` the session
    /// also logs verbosely into it: the zero-inference shadow that reads the node placement.
    fn build(
        &mut self,
        model: &[u8],
        path: &Path,
        options: &SessionOptions,
        placement_logger: Option<LoggerFunction>,
    ) -> Result<Self::Session, SessionBuildFailure>;
}

/// ONNX Runtime's sessions.
struct OrtSessions;

impl SessionBuilds for OrtSessions {
    type Session = Session;

    fn build(
        &mut self,
        model: &[u8],
        path: &Path,
        options: &SessionOptions,
        placement_logger: Option<LoggerFunction>,
    ) -> Result<Session, SessionBuildFailure> {
        let step = |message: String| {
            SessionBuildFailure::step(options, GpuUnavailableReason::SessionBuildFailed, message)
        };
        let mut builder = configured_session_builder(options)?;
        if let Some(logger) = placement_logger {
            builder = builder
                .with_logger(logger)
                .map_err(|err| {
                    step(format!(
                        "failed to configure bounded PPOCR node-placement diagnostic: {err}"
                    ))
                    .with_ort_status(&err)
                })?
                .with_log_level(LogLevel::Verbose)
                .map_err(|err| {
                    step(format!(
                        "failed to enable verbose PPOCR node-placement diagnostic: {err}"
                    ))
                    .with_ort_status(&err)
                })?
                .with_log_verbosity(0)
                .map_err(|err| {
                    step(format!(
                        "failed to bound PPOCR node-placement diagnostic: {err}"
                    ))
                    .with_ort_status(&err)
                })?;
        }
        builder.commit_from_memory(model).map_err(|err| {
            let failed = step(match options.execution_provider {
                SessionExecutionProvider::Cpu => format!(
                    "failed to load CPU-only PPOCR ONNX model {}: {err}",
                    path.display()
                ),
                SessionExecutionProvider::Cuda { device_id } => format!(
                    "failed to load CUDA PPOCR ONNX model {} on device {device_id}: {err}",
                    path.display()
                ),
                SessionExecutionProvider::Directml { device_id } => format!(
                    "failed to load DirectML PPOCR ONNX model {} on DXGI adapter {device_id} (CPU nodes allowed): {err}",
                    path.display()
                ),
            });
            failed.with_ort_status(&err)
        })
    }
}

/// Ported from `configured_session_builder` of the GPU-0c experiment (`exp/407-gpu0-bench`
/// `f9ba0c47`): ONNX Runtime's session builder with every option of `options` set, the one
/// place that sets them. A CPU session sets one intra-op thread; a CUDA session also registers
/// CUDA and disables CPU fallback, as before. A DirectML session turns the memory pattern off,
/// runs sequentially and registers DirectML on its adapter, and never disables CPU fallback.
fn configured_session_builder(
    options: &SessionOptions,
) -> Result<SessionBuilder, SessionBuildFailure> {
    let step = |reason: GpuUnavailableReason, message: String| {
        SessionBuildFailure::step(options, reason, message)
    };
    let failed = GpuUnavailableReason::SessionBuildFailed;
    let builder = Session::builder()
        .map_err(|err| {
            step(
                failed,
                format!("failed to create ONNXRuntime session builder: {err}"),
            )
            .with_ort_status(&err)
        })?
        .with_intra_threads(options.intra_threads)
        .map_err(|err| {
            step(
                failed,
                format!("failed to configure ONNXRuntime intra threads: {err}"),
            )
            .with_ort_status(&err)
        })?;
    let builder = match options.memory_pattern {
        Some(enabled) => builder.with_memory_pattern(enabled).map_err(|err| {
            step(
                failed,
                format!("failed to set the ONNXRuntime memory pattern to {enabled}: {err}"),
            )
            .with_ort_status(&err)
        })?,
        None => builder,
    };
    let builder = match options.parallel_execution {
        Some(parallel) => builder.with_parallel_execution(parallel).map_err(|err| {
            step(
                failed,
                format!("failed to set ONNXRuntime parallel execution to {parallel}: {err}"),
            )
            .with_ort_status(&err)
        })?,
        None => builder,
    };
    let builder = match options.execution_provider {
        SessionExecutionProvider::Cpu => builder,
        SessionExecutionProvider::Cuda { device_id } => builder
            .with_execution_providers([
                ort::ep::CUDA::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            ])
            .map_err(|err| {
                step(
                    GpuUnavailableReason::ProviderRegisterFailed,
                    format!(
                        "failed to register required CUDA execution provider for device {device_id}; CPU/DirectML/CoreML fallback is disabled: {err}"
                    ),
                )
                .with_ort_status(&err)
            })?,
        SessionExecutionProvider::Directml { device_id } => builder
            .with_execution_providers([
                ort::ep::DirectML::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            ])
            .map_err(|err| {
                step(
                    GpuUnavailableReason::ProviderRegisterFailed,
                    format!(
                        "failed to register the DirectML execution provider on DXGI adapter {device_id}; CPU nodes are allowed: {err}"
                    ),
                )
                .with_ort_status(&err)
            })?,
    };
    if options.cpu_fallback_disabled {
        builder.with_disable_cpu_fallback().map_err(|err| {
            step(
                failed,
                format!("failed to disable PPOCR CPU fallback: {err}"),
            )
            .with_ort_status(&err)
        })
    } else {
        Ok(builder)
    }
}

/// One session to build: the model's bytes, its file (for messages), its role and, for
/// DirectML, the adapter this build resolved.
struct SessionRequest<'a> {
    model: &'a [u8],
    path: &'a Path,
    role: PpocrModelRole,
    adapter: Option<&'a ResolvedDirectmlAdapter>,
}

/// A built session and, when its node placement was read, how many of its nodes ONNX Runtime
/// placed on the CPU.
struct BuiltSession<S> {
    session: S,
    cpu_nodes: Option<usize>,
}

/// Builds one session of `plan` (Workflow #407 GPU-1 B1, design §2.1, §2.2). Its options come
/// from [`session_options`] alone. A session that reads its node placement (DirectML always,
/// CUDA when the diagnostic is requested) first builds the zero-inference shadow of
/// [`read_node_placement`] from the same options; its report joins `diagnostics`.
fn load_session_with<B: SessionBuilds>(
    builds: &mut B,
    request: &SessionRequest<'_>,
    plan: &ProviderSessionPlan,
    node_placement_diagnostic: Option<&str>,
    diagnostics: &mut PpocrDiagnostics,
) -> Result<BuiltSession<B::Session>, SessionBuildFailure> {
    // A failure without a reason is the engine's `ProviderFailure`, the code of `session_options`.
    let options =
        session_options(plan).map_err(|error| SessionBuildFailure::plain(error.message()))?;
    let device = session_device(&options, request.adapter)?;
    let reads_placement = node_placement_diagnostic_requested(plan, node_placement_diagnostic)
        .map_err(SessionBuildFailure::plain)?;
    let cpu_nodes = if reads_placement {
        Some(read_node_placement(
            builds,
            request,
            &options,
            device,
            diagnostics,
        )?)
    } else {
        None
    };
    let session = builds.build(request.model, request.path, &options, None)?;
    Ok(BuiltSession { session, cpu_nodes })
}

/// The adapter a DirectML session's report names: the one this build resolved, which must be
/// at the DXGI index the session registers.
fn session_device(
    options: &SessionOptions,
    adapter: Option<&ResolvedDirectmlAdapter>,
) -> Result<Option<PpocrDiagnosticDevice>, SessionBuildFailure> {
    match (options.execution_provider, adapter) {
        (SessionExecutionProvider::Directml { device_id }, Some(adapter)) => {
            if u32::try_from(device_id).ok() != Some(adapter.dxgi_index) {
                return Err(SessionBuildFailure::directml(
                    GpuUnavailableReason::AdapterMissing,
                    format!(
                        "the DirectML adapter {} resolved at DXGI index {} but the session names index {device_id}",
                        adapter.identity, adapter.dxgi_index
                    ),
                ));
            }
            Ok(Some(PpocrDiagnosticDevice {
                identity: adapter.identity.clone(),
                dxgi_index: adapter.dxgi_index,
                luid: adapter.luid.to_string(),
            }))
        }
        (SessionExecutionProvider::Directml { .. }, None) => Err(SessionBuildFailure::plain(
            "a DirectML session build has no resolved adapter",
        )),
        (_, None) => Ok(None),
        (_, Some(_)) => Err(SessionBuildFailure::plain(
            "only a DirectML session build names a DirectML adapter",
        )),
    }
}

/// Reads where ONNX Runtime places the request's nodes under `options` (design §2.2): a
/// zero-inference shadow session built from exactly `options` with a verbose logger, dropped
/// before the session itself is built; no inference runs. A whole placement, checked against
/// the provider the session registers, joins `diagnostics` as one v2 report, also when the
/// shadow failed after logging it (a CUDA session refuses its CPU nodes only once they are
/// placed): the shadow's failure is then the session's. A shadow that built but logged no whole
/// placement is a session that cannot be built (`placement_unread`). Returns the CPU-placed node
/// count.
fn read_node_placement<B: SessionBuilds>(
    builds: &mut B,
    request: &SessionRequest<'_>,
    options: &SessionOptions,
    device: Option<PpocrDiagnosticDevice>,
    diagnostics: &mut PpocrDiagnostics,
) -> Result<usize, SessionBuildFailure> {
    if diagnostics.len() >= PPOCR_MAX_DIAGNOSTIC_REPORTS {
        return Err(SessionBuildFailure::plain(
            "PPOCR invocation diagnostic report bound exceeded",
        ));
    }
    diagnostics.try_reserve(1).map_err(|error| {
        SessionBuildFailure::plain(format!(
            "PPOCR diagnostic result allocation failed: {error}"
        ))
    })?;
    let capture = Arc::new(Mutex::new(NodePlacementLogCapture::default()));
    // A shadow that was built is dropped here, before anything else is built.
    let shadow_failure = builds
        .build(
            request.model,
            request.path,
            options,
            Some(placement_logger(&capture)),
        )
        .err()
        .map(SessionBuildFailure::of_placement_shadow);
    let read = match capture.lock() {
        Ok(state) => state.diagnostic(request.role),
        Err(poisoned) => {
            let mut state = poisoned.into_inner();
            state.fail("PPOCR node-placement diagnostic mutex is poisoned");
            state.diagnostic(request.role)
        }
    }
    .map_err(failure)
    .and_then(|report| complete_placement_report(report, options, device));
    match (read, shadow_failure) {
        (Ok(report), shadow_failure) => {
            let cpu_nodes = report.cpu_assigned_node_count;
            diagnostics.push(Arc::new(report));
            match shadow_failure {
                Some(failure) => Err(failure),
                None => Ok(cpu_nodes),
            }
        }
        (Err(_), Some(failure)) => Err(failure),
        (Err(unread), None) => Err(SessionBuildFailure::step(
            options,
            GpuUnavailableReason::PlacementUnread,
            format!(
                "the node placement of the PPOCR {} model {} cannot be read: {}",
                request.role.as_str(),
                request.path.display(),
                unread.message()
            ),
        )),
    }
}

/// The verbose logger of a placement read: every message goes to `capture`.
fn placement_logger(capture: &Arc<Mutex<NodePlacementLogCapture>>) -> LoggerFunction {
    let sink = Arc::clone(capture);
    Arc::new(
        move |_: LogLevel, _: &str, _: &str, _: &str, message: &str| match sink.lock() {
            Ok(mut state) => state.record(message),
            Err(poisoned) => poisoned
                .into_inner()
                .fail("PPOCR node-placement diagnostic mutex is poisoned"),
        },
    )
}

/// A read report checked against the session: ONNX Runtime must have placed the accelerated
/// nodes on the provider the session registered, and a DirectML report names its adapter.
fn complete_placement_report(
    mut report: NodePlacementDiagnostic,
    options: &SessionOptions,
    device: Option<PpocrDiagnosticDevice>,
) -> VisionFfiResult<NodePlacementDiagnostic> {
    let registered = options
        .execution_provider
        .accelerated_name()
        .ok_or_else(|| failure("a CPU session has no node placement to read"))?;
    if report.accelerated_execution_provider != registered {
        return Err(failure(format!(
            "ONNX Runtime placed nodes on {} but the session registered {registered}",
            report.accelerated_execution_provider
        )));
    }
    report.device = device;
    report
        .validate()
        .map_err(|error| failure(error.to_string()))?;
    Ok(report)
}

fn recognize_region(
    session: &mut Session,
    dictionary: &[String],
    params: &PpocrCtcParams,
    frame: &VisionFrameView<'_>,
    region: VisionRect,
    timeout: Duration,
) -> Result<DecodedText, ProviderInvokeError> {
    let input_shape = select_recognition_input_shape(session, region, params)?;
    let input_data = frame_region_to_recognition_tensor(frame, region, &input_shape, params)?;
    let input = Tensor::from_array((input_shape.to_ort_shape(), input_data.into_boxed_slice()))
        .map_err(|err| format!("failed to create PPOCR recognizer input tensor: {err}"))?;
    let run_options = Arc::new(
        RunOptions::new().map_err(|err| format!("failed to create ONNX run options: {err}"))?,
    );
    let watchdog = InferenceWatchdog::start(Arc::clone(&run_options), timeout);
    let outputs = session.run_with_options(ort::inputs![input], &*run_options);
    if watchdog.timed_out() {
        return Err(ProviderInvokeError::timeout(
            "PPOCR recognizer inference exceeded the configured timeout",
        ));
    }
    let outputs = outputs.map_err(|err| {
        ProviderInvokeError::run(format!("PPOCR recognizer inference failed: {err}"), &err)
    })?;
    if outputs.len() == 0 {
        return Err("PPOCR recognizer returned no outputs".to_string().into());
    }
    let (output_shape, scores) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("PPOCR recognizer first output is not an f32 tensor: {err}"))?;
    Ok(decode_ctc_output(
        output_shape.as_ref(),
        scores,
        dictionary,
        params,
    )?)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DetectedTextBox {
    rect: VisionRect,
    confidence: f32,
}

/// The detector's boxes inside `region`, padded and clipped to the region, merged on a line
/// with `merge_gap_factor` (`None` merges nothing), in reading order.
fn detect_text_regions(
    session: &mut Session,
    params: &PpocrCtcParams,
    frame: &VisionFrameView<'_>,
    region: VisionRect,
    merge_gap_factor: Option<u32>,
    timeout: Duration,
) -> Result<Vec<DetectedTextBox>, ProviderInvokeError> {
    let input_shape = select_detection_input_shape(session, region, params)?;
    let input_data = frame_region_to_detection_tensor(frame, region, &input_shape, params)?;
    let input = Tensor::from_array((input_shape.to_ort_shape(), input_data.into_boxed_slice()))
        .map_err(|err| format!("failed to create PPOCR detector input tensor: {err}"))?;
    let run_options = Arc::new(
        RunOptions::new().map_err(|err| format!("failed to create ONNX run options: {err}"))?,
    );
    let watchdog = InferenceWatchdog::start(Arc::clone(&run_options), timeout);
    let outputs = session.run_with_options(ort::inputs![input], &*run_options);
    if watchdog.timed_out() {
        return Err(ProviderInvokeError::timeout(
            "PPOCR detector inference exceeded the configured timeout",
        ));
    }
    let outputs = outputs.map_err(|err| {
        ProviderInvokeError::run(format!("PPOCR detector inference failed: {err}"), &err)
    })?;
    if outputs.len() == 0 {
        return Err("PPOCR detector returned no outputs".to_string().into());
    }
    let (output_shape, scores) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("PPOCR detector first output is not an f32 tensor: {err}"))?;
    let map = detector_probability_map(output_shape.as_ref(), scores)?;
    let boxes = detect_text_boxes_from_probability_map(&map, region, params)?;
    Ok(merge_detected_text_boxes(
        boxes,
        region,
        params,
        merge_gap_factor,
    ))
}

fn remaining_inference_budget(
    deadline: Instant,
    stage: &str,
) -> Result<Duration, ProviderInvokeError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            ProviderInvokeError::timeout(format!(
                "{stage} could not start before the configured timeout"
            ))
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecognitionInputShape {
    height: usize,
    width: usize,
}

impl RecognitionInputShape {
    fn to_ort_shape(self) -> Vec<i64> {
        vec![1, 3, self.height as i64, self.width as i64]
    }
}

fn select_recognition_input_shape(
    session: &Session,
    region: VisionRect,
    params: &PpocrCtcParams,
) -> Result<RecognitionInputShape, String> {
    let input = session
        .inputs()
        .first()
        .ok_or_else(|| "PPOCR recognizer model has no inputs".to_string())?;
    let ValueType::Tensor { ty, shape, .. } = input.dtype() else {
        return Err(format!(
            "PPOCR recognizer input {} is not a tensor",
            input.name()
        ));
    };
    if *ty != TensorElementType::Float32 {
        return Err(format!(
            "PPOCR recognizer input {} must be float32, got {ty:?}",
            input.name()
        ));
    }
    if shape.len() != 4 {
        return Err(format!(
            "PPOCR recognizer input {} must be rank 4 NCHW, got shape {shape}",
            input.name()
        ));
    }
    if !dimension_matches(shape[0], 1) || !dimension_matches(shape[1], 3) {
        return Err(format!(
            "PPOCR recognizer input {} must be NCHW with batch 1 and 3 channels, got shape {shape}",
            input.name()
        ));
    }
    let input_height = params.recognizer.input_height as usize;
    let height = positive_or_default(shape[2], input_height, "recognizer height")?;
    if height != input_height {
        return Err(format!(
            "PPOCR recognizer input {} has the static height {height}, but the model description's recognizer.input_height is {input_height}",
            input.name()
        ));
    }
    let width = if shape[3] > 0 {
        usize::try_from(shape[3]).map_err(|err| format!("invalid recognizer width: {err}"))?
    } else {
        dynamic_width_for_region(region, height, params)?
    };
    Ok(RecognitionInputShape { height, width })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DetectionInputShape {
    height: usize,
    width: usize,
}

impl DetectionInputShape {
    fn to_ort_shape(self) -> Vec<i64> {
        vec![1, 3, self.height as i64, self.width as i64]
    }
}

fn select_detection_input_shape(
    session: &Session,
    region: VisionRect,
    params: &PpocrCtcParams,
) -> Result<DetectionInputShape, String> {
    let input = session
        .inputs()
        .first()
        .ok_or_else(|| "PPOCR detector model has no inputs".to_string())?;
    let ValueType::Tensor { ty, shape, .. } = input.dtype() else {
        return Err(format!(
            "PPOCR detector input {} is not a tensor",
            input.name()
        ));
    };
    if *ty != TensorElementType::Float32 {
        return Err(format!(
            "PPOCR detector input {} must be float32, got {ty:?}",
            input.name()
        ));
    }
    if shape.len() != 4 {
        return Err(format!(
            "PPOCR detector input {} must be rank 4 NCHW, got shape {shape}",
            input.name()
        ));
    }
    if !dimension_matches(shape[0], 1) || !dimension_matches(shape[1], 3) {
        return Err(format!(
            "PPOCR detector input {} must be NCHW with batch 1 and 3 channels, got shape {shape}",
            input.name()
        ));
    }
    if shape[2] > 0 && shape[3] > 0 {
        let height =
            usize::try_from(shape[2]).map_err(|err| format!("invalid detector height: {err}"))?;
        let width =
            usize::try_from(shape[3]).map_err(|err| format!("invalid detector width: {err}"))?;
        if height == 0 || width == 0 {
            return Err("detector input dimensions must be non-zero".to_string());
        }
        return Ok(DetectionInputShape { height, width });
    }
    dynamic_detection_shape_for_region(region, params)
}

fn dimension_matches(expected: i64, actual: i64) -> bool {
    expected < 0 || expected == actual
}

fn positive_or_default(value: i64, default: usize, label: &str) -> Result<usize, String> {
    if value < 0 {
        return Ok(default);
    }
    let value = usize::try_from(value).map_err(|err| format!("invalid {label}: {err}"))?;
    if value == 0 {
        return Err(format!("{label} must be non-zero"));
    }
    Ok(value)
}

/// The tensor width of a dynamic-width recognizer: the region's aspect at the tensor height,
/// clamped to the description's `min_width..=max_width`; narrower content is zero-padded.
fn dynamic_width_for_region(
    region: VisionRect,
    height: usize,
    params: &PpocrCtcParams,
) -> Result<usize, String> {
    if region.height <= 0 {
        return Err("OCR region height must be non-zero".to_string());
    }
    let exact = (region.width as f32 / region.height as f32) * height as f32;
    // Up to the default 320 the content keeps its ceil width, exactly as before; the official
    // pipeline does the same inside its tensor of at least 320. Above 320 the official pipeline
    // sizes the tensor, and with it the content, as int(height * w / h), so a description that
    // raises max_width gets that floor width (review O3-1). Every default path is unchanged:
    // the 320 cap hides the difference.
    let ceil = exact.ceil() as usize;
    let scaled = if ceil > OFFICIAL_FLOOR_WIDTH_ABOVE {
        exact.floor() as usize
    } else {
        ceil
    };
    Ok(scaled.clamp(
        params.recognizer.min_width as usize,
        params.recognizer.max_width as usize,
    ))
}

fn dynamic_detection_shape_for_region(
    region: VisionRect,
    params: &PpocrCtcParams,
) -> Result<DetectionInputShape, String> {
    let detector = &params.detector;
    let max_side = detector.max_side as usize;
    let min_side = detector.min_side as usize;
    let multiple = detector.multiple as usize;
    let rect = RectUsize::from_vision_rect(region)?;
    let longest = rect.width.max(rect.height);
    let scale = if longest > max_side {
        max_side as f32 / longest as f32
    } else {
        1.0
    };
    let width = round_up_to_multiple(
        ((rect.width as f32 * scale).ceil() as usize).max(min_side),
        multiple,
    );
    let height = round_up_to_multiple(
        ((rect.height as f32 * scale).ceil() as usize).max(min_side),
        multiple,
    );
    Ok(DetectionInputShape { height, width })
}

fn round_up_to_multiple(value: usize, multiple: usize) -> usize {
    value.div_ceil(multiple) * multiple
}

fn frame_region_to_recognition_tensor(
    frame: &VisionFrameView<'_>,
    region: VisionRect,
    input_shape: &RecognitionInputShape,
    params: &PpocrCtcParams,
) -> Result<Vec<f32>, String> {
    let rect = RectUsize::from_vision_rect(region)?;
    let frame_width =
        usize::try_from(frame.width).map_err(|err| format!("invalid frame width: {err}"))?;
    let frame_height =
        usize::try_from(frame.height).map_err(|err| format!("invalid frame height: {err}"))?;
    if rect.x + rect.width > frame_width || rect.y + rect.height > frame_height {
        return Err("OCR region exceeds frame bounds".to_string());
    }
    let channels = frame_channels(frame.pixel_format);
    let resized_width = ((rect.width as f32 / rect.height as f32) * input_shape.height as f32)
        .ceil()
        .max(1.0) as usize;
    let resized_width = resized_width.min(input_shape.width);
    let plane_size = input_shape.height * input_shape.width;
    let mut tensor = vec![0.0_f32; plane_size * 3];

    for out_y in 0..input_shape.height {
        let (src_y, fy) = rec_resize_coordinate(out_y, rect.height, input_shape.height);
        let y_weights = rec_linear_weights(fy);
        // OpenCV retains the vertical weights when both rows clamp to an edge.
        let top = rect.y + src_y.clamp(0, rect.height as isize - 1) as usize;
        let bottom = rect.y + (src_y + 1).clamp(0, rect.height as isize - 1) as usize;
        for out_x in 0..resized_width {
            let (src_x, mut fx) = rec_resize_coordinate(out_x, rect.width, resized_width);
            if src_x < 0 || src_x >= rect.width as isize - 1 {
                fx = 0.0;
            }
            let left = src_x.clamp(0, rect.width as isize - 1) as usize;
            let right = (left + 1).min(rect.width - 1);
            let x_weights = rec_linear_weights(fx);
            let read = |y, x| {
                read_rgb_pixel(
                    frame.pixels,
                    frame.pixel_format,
                    (y * frame_width + rect.x + x) * channels,
                )
            };
            let tl = read(top, left)?;
            let tr = read(top, right)?;
            let bl = read(bottom, left)?;
            let br = read(bottom, right)?;
            let dst = out_y * input_shape.width + out_x;
            let red = [tl.0, tr.0, bl.0, br.0];
            let green = [tl.1, tr.1, bl.1, br.1];
            let blue = [tl.2, tr.2, bl.2, br.2];
            // The frame remains RGB; RecResizeImg's internal NCHW input is BGR by default.
            let planes = match params.recognizer.channel_order {
                ChannelOrder::Bgr => [blue, green, red],
                ChannelOrder::Rgb => [red, green, blue],
            };
            for (channel, samples) in planes.into_iter().enumerate() {
                tensor[channel * plane_size + dst] =
                    normalize_rec_pixel(rec_linear_pixel(samples, x_weights, y_weights));
            }
        }
    }
    Ok(tensor)
}

fn rec_resize_coordinate(output: usize, source_size: usize, output_size: usize) -> (isize, f32) {
    let scale = 1.0 / (output_size as f64 / source_size as f64);
    let coordinate = ((output as f64 + 0.5) * scale - 0.5) as f32;
    let base = coordinate.floor();
    (base as isize, coordinate - base)
}

fn rec_linear_weights(fraction: f32) -> [i32; 2] {
    // OpenCV INTER_LINEAR's uint8 path uses 11-bit coefficients, nearest-even.
    [
        ((1.0 - fraction) * 2048.0).round_ties_even() as i32,
        (fraction * 2048.0).round_ties_even() as i32,
    ]
}

fn rec_linear_pixel(samples: [u8; 4], x_weights: [i32; 2], y_weights: [i32; 2]) -> u8 {
    let top = i32::from(samples[0]) * x_weights[0] + i32::from(samples[1]) * x_weights[1];
    let bottom = i32::from(samples[2]) * x_weights[0] + i32::from(samples[3]) * x_weights[1];
    // Preserve uint8 INTER_LINEAR's staged integer quantization before normalization.
    // Reference: OpenCV 4.12.0 resize.cpp, VResizeLinear<uchar, int, short, ...>.
    ((((y_weights[0] * (top >> 4)) >> 16) + ((y_weights[1] * (bottom >> 4)) >> 16) + 2) >> 2) as u8
}

fn frame_region_to_detection_tensor(
    frame: &VisionFrameView<'_>,
    region: VisionRect,
    input_shape: &DetectionInputShape,
    params: &PpocrCtcParams,
) -> Result<Vec<f32>, String> {
    let rect = RectUsize::from_vision_rect(region)?;
    let frame_width =
        usize::try_from(frame.width).map_err(|err| format!("invalid frame width: {err}"))?;
    let frame_height =
        usize::try_from(frame.height).map_err(|err| format!("invalid frame height: {err}"))?;
    if rect.x + rect.width > frame_width || rect.y + rect.height > frame_height {
        return Err("OCR detector region exceeds frame bounds".to_string());
    }
    let channels = frame_channels(frame.pixel_format);
    let plane_size = input_shape.height * input_shape.width;
    let mut tensor = vec![0.0_f32; plane_size * 3];

    let read = |y: usize, x: usize| {
        read_rgb_pixel(
            frame.pixels,
            frame.pixel_format,
            (y * frame_width + x) * channels,
        )
    };
    for out_y in 0..input_shape.height {
        let rows = match params.detector.resize {
            DetectorResize::Nearest => {
                let src_y =
                    rect.y + (out_y * rect.height / input_shape.height).min(rect.height - 1);
                (src_y, src_y, [2048, 0])
            }
            DetectorResize::Linear => {
                let (src_y, fy) = rec_resize_coordinate(out_y, rect.height, input_shape.height);
                (
                    rect.y + src_y.clamp(0, rect.height as isize - 1) as usize,
                    rect.y + (src_y + 1).clamp(0, rect.height as isize - 1) as usize,
                    rec_linear_weights(fy),
                )
            }
        };
        for out_x in 0..input_shape.width {
            let (r, g, b) = match params.detector.resize {
                DetectorResize::Nearest => {
                    let src_x =
                        rect.x + (out_x * rect.width / input_shape.width).min(rect.width - 1);
                    read(rows.0, src_x)?
                }
                DetectorResize::Linear => {
                    let (src_x, mut fx) =
                        rec_resize_coordinate(out_x, rect.width, input_shape.width);
                    if src_x < 0 || src_x >= rect.width as isize - 1 {
                        fx = 0.0;
                    }
                    let left = src_x.clamp(0, rect.width as isize - 1) as usize;
                    let right = (left + 1).min(rect.width - 1);
                    let x_weights = rec_linear_weights(fx);
                    let tl = read(rows.0, rect.x + left)?;
                    let tr = read(rows.0, rect.x + right)?;
                    let bl = read(rows.1, rect.x + left)?;
                    let br = read(rows.1, rect.x + right)?;
                    let sample = |samples| rec_linear_pixel(samples, x_weights, rows.2);
                    (
                        sample([tl.0, tr.0, bl.0, br.0]),
                        sample([tl.1, tr.1, bl.1, br.1]),
                        sample([tl.2, tr.2, bl.2, br.2]),
                    )
                }
            };
            let dst = out_y * input_shape.width + out_x;
            // The ImageNet mean and deviation are indexed by plane, whatever its colour.
            let planes = match params.detector.channel_order {
                ChannelOrder::Rgb => [r, g, b],
                ChannelOrder::Bgr => [b, g, r],
            };
            for (plane, value) in planes.into_iter().enumerate() {
                tensor[plane * plane_size + dst] = normalize_det_pixel(value, plane);
            }
        }
    }
    Ok(tensor)
}

#[derive(Debug, Clone, Copy)]
struct RectUsize {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl RectUsize {
    fn from_vision_rect(rect: VisionRect) -> Result<Self, String> {
        if rect.x < 0 || rect.y < 0 || rect.width <= 0 || rect.height <= 0 {
            return Err("OCR region must have non-negative origin and positive size".to_string());
        }
        Ok(Self {
            x: rect.x as usize,
            y: rect.y as usize,
            width: rect.width as usize,
            height: rect.height as usize,
        })
    }
}

fn frame_channels(pixel_format: VisionPixelFormat) -> usize {
    match pixel_format {
        VisionPixelFormat::Rgb8 => 3,
        VisionPixelFormat::Rgba8 => 4,
        VisionPixelFormat::Gray8 => 1,
    }
}

fn read_rgb_pixel(
    pixels: &[u8],
    pixel_format: VisionPixelFormat,
    offset: usize,
) -> Result<(u8, u8, u8), String> {
    match pixel_format {
        VisionPixelFormat::Rgb8 => read_channels(pixels, offset)
            .map(|channels: [u8; 3]| (channels[0], channels[1], channels[2])),
        VisionPixelFormat::Rgba8 => read_channels(pixels, offset)
            .map(|channels: [u8; 4]| (channels[0], channels[1], channels[2])),
        VisionPixelFormat::Gray8 => pixels
            .get(offset)
            .copied()
            .map(|value| (value, value, value))
            .ok_or_else(|| "frame pixel buffer ended while reading gray pixel".to_string()),
    }
}

fn read_channels<const N: usize>(pixels: &[u8], offset: usize) -> Result<[u8; N], String> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| "frame pixel offset overflow".to_string())?;
    let slice = pixels
        .get(offset..end)
        .ok_or_else(|| "frame pixel buffer ended while reading color pixel".to_string())?;
    slice
        .try_into()
        .map_err(|_| "failed to read color pixel".to_string())
}

fn normalize_rec_pixel(value: u8) -> f32 {
    f32::from(value) / 127.5 - 1.0
}

fn normalize_det_pixel(value: u8, channel: usize) -> f32 {
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    (f32::from(value) / 255.0 - MEAN[channel]) / STD[channel]
}

#[derive(Debug, Clone, PartialEq)]
struct ProbabilityMap {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

fn detector_probability_map(shape: &[i64], scores: &[f32]) -> Result<ProbabilityMap, String> {
    let dims: Vec<_> = shape
        .iter()
        .map(|dim| {
            usize::try_from(*dim).map_err(|err| format!("invalid detector output dimension: {err}"))
        })
        .collect::<Result<_, _>>()?;
    let (height, width) = match dims.as_slice() {
        [1, 1, height, width] if height * width == scores.len() => (*height, *width),
        [1, height, width] if height * width == scores.len() => (*height, *width),
        [height, width] if height * width == scores.len() => (*height, *width),
        _ => {
            return Err(format!(
                "unsupported PPOCR detector output shape {shape:?} for {} scores",
                scores.len()
            ));
        }
    };
    if height == 0 || width == 0 {
        return Err("PPOCR detector output dimensions must be non-zero".to_string());
    }
    let mut values = Vec::with_capacity(scores.len());
    for score in scores {
        if !score.is_finite() {
            return Err("PPOCR detector output contains a non-finite score".to_string());
        }
        values.push(to_probability(*score));
    }
    Ok(ProbabilityMap {
        width,
        height,
        values,
    })
}

fn to_probability(value: f32) -> f32 {
    if (0.0..=1.0).contains(&value) {
        value
    } else {
        1.0 / (1.0 + (-value).exp())
    }
}

/// The padded box of every text component of the region's probability map. Padding is clipped
/// to the requested region (Workflow #396 REC-1); a full-frame region clips to the frame as
/// before.
fn detect_text_boxes_from_probability_map(
    map: &ProbabilityMap,
    region: VisionRect,
    params: &PpocrCtcParams,
) -> Result<Vec<DetectedTextBox>, String> {
    let threshold = params.detector.threshold;
    let padding = params.detector.box_padding as i32;
    let mut visited = vec![false; map.values.len()];
    let mut boxes = Vec::new();
    for index in 0..map.values.len() {
        if visited[index] || map.values[index] < threshold {
            continue;
        }
        let component = collect_component(map, index, &mut visited, threshold);
        if component.area < params.detector.min_area as usize {
            continue;
        }
        let rect = component_to_vision_rect(&component, map, region, padding)?;
        boxes.push(DetectedTextBox {
            rect,
            confidence: component.max_score,
        });
    }
    boxes.sort_by_key(|text_box| (text_box.rect.y, text_box.rect.x));
    Ok(boxes)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MapComponent {
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
    area: usize,
    max_score: f32,
}

fn collect_component(
    map: &ProbabilityMap,
    start_index: usize,
    visited: &mut [bool],
    threshold: f32,
) -> MapComponent {
    let mut queue = VecDeque::from([start_index]);
    visited[start_index] = true;
    let mut component = MapComponent {
        min_x: start_index % map.width,
        min_y: start_index / map.width,
        max_x: start_index % map.width,
        max_y: start_index / map.width,
        area: 0,
        max_score: map.values[start_index],
    };

    while let Some(index) = queue.pop_front() {
        let x = index % map.width;
        let y = index / map.width;
        component.min_x = component.min_x.min(x);
        component.min_y = component.min_y.min(y);
        component.max_x = component.max_x.max(x);
        component.max_y = component.max_y.max(y);
        component.area += 1;
        component.max_score = component.max_score.max(map.values[index]);

        for (next_x, next_y) in neighbors4(x, y, map.width, map.height) {
            let next_index = next_y * map.width + next_x;
            if visited[next_index] || map.values[next_index] < threshold {
                continue;
            }
            visited[next_index] = true;
            queue.push_back(next_index);
        }
    }
    component
}

fn neighbors4(x: usize, y: usize, width: usize, height: usize) -> Vec<(usize, usize)> {
    let mut neighbors = Vec::with_capacity(4);
    if x > 0 {
        neighbors.push((x - 1, y));
    }
    if x + 1 < width {
        neighbors.push((x + 1, y));
    }
    if y > 0 {
        neighbors.push((x, y - 1));
    }
    if y + 1 < height {
        neighbors.push((x, y + 1));
    }
    neighbors
}

fn component_to_vision_rect(
    component: &MapComponent,
    map: &ProbabilityMap,
    region: VisionRect,
    padding: i32,
) -> Result<VisionRect, String> {
    let scale_x = region.width as f32 / map.width as f32;
    let scale_y = region.height as f32 / map.height as f32;
    let left = region.x + (component.min_x as f32 * scale_x).floor() as i32;
    let top = region.y + (component.min_y as f32 * scale_y).floor() as i32;
    let right = region.x + ((component.max_x + 1) as f32 * scale_x).ceil() as i32;
    let bottom = region.y + ((component.max_y + 1) as f32 * scale_y).ceil() as i32;
    padded_rect(left, top, right, bottom, region, padding)
}

/// The box grown by `padding` on every side and clipped to `bounds`, the requested region.
fn padded_rect(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    bounds: VisionRect,
    padding: i32,
) -> Result<VisionRect, String> {
    let x = (left - padding).max(bounds.x);
    let y = (top - padding).max(bounds.y);
    let right = (right + padding).min(rect_right(bounds));
    let bottom = (bottom + padding).min(rect_bottom(bounds));
    if right <= x || bottom <= y {
        return Err("detected OCR box collapsed after clamping".to_string());
    }
    Ok(VisionRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

/// Boxes on one line merge under the folder's rule with `merge_gap_factor` in place of the
/// folder's factor; `None` merges no boxes. The result is in reading order `(y, x)`.
fn merge_detected_text_boxes(
    mut boxes: Vec<DetectedTextBox>,
    bounds: VisionRect,
    params: &PpocrCtcParams,
    merge_gap_factor: Option<u32>,
) -> Vec<DetectedTextBox> {
    let Some(merge_gap_factor) = merge_gap_factor else {
        boxes.sort_by_key(|text_box| (text_box.rect.y, text_box.rect.x));
        return boxes;
    };
    boxes.sort_by_key(|text_box| (rect_center_y(text_box.rect), text_box.rect.x));
    let mut merged: Vec<DetectedTextBox> = Vec::new();
    for text_box in boxes {
        if let Some(last) = merged.last_mut()
            && should_merge_text_boxes(last.rect, text_box.rect, params, merge_gap_factor)
            && let Ok(rect) = union_rect(
                last.rect,
                text_box.rect,
                bounds,
                params.detector.box_padding as i32,
            )
        {
            last.rect = rect;
            last.confidence = last.confidence.max(text_box.confidence);
            continue;
        }
        merged.push(text_box);
    }
    merged.sort_by_key(|text_box| (text_box.rect.y, text_box.rect.x));
    merged
}

fn should_merge_text_boxes(
    left: VisionRect,
    right: VisionRect,
    params: &PpocrCtcParams,
    merge_gap_factor: u32,
) -> bool {
    let detector = &params.detector;
    let vertical_overlap = (rect_bottom(left).min(rect_bottom(right)) - left.y.max(right.y)).max(0);
    let min_height = left.height.min(right.height).max(1);
    let horizontal_gap = if right.x >= rect_right(left) {
        right.x - rect_right(left)
    } else if left.x >= rect_right(right) {
        left.x - rect_right(right)
    } else {
        0
    };
    vertical_overlap * 100 >= min_height * detector.merge_min_overlap_percent as i32
        && horizontal_gap
            <= left
                .height
                .max(right.height)
                .max(detector.merge_min_height as i32)
                * merge_gap_factor as i32
}

fn union_rect(
    left: VisionRect,
    right: VisionRect,
    bounds: VisionRect,
    padding: i32,
) -> Result<VisionRect, String> {
    padded_rect(
        left.x.min(right.x),
        left.y.min(right.y),
        rect_right(left).max(rect_right(right)),
        rect_bottom(left).max(rect_bottom(right)),
        bounds,
        padding,
    )
}

fn rect_right(rect: VisionRect) -> i32 {
    rect.x + rect.width
}

fn rect_bottom(rect: VisionRect) -> i32 {
    rect.y + rect.height
}

fn rect_center_y(rect: VisionRect) -> i32 {
    rect.y + rect.height / 2
}

fn is_full_frame_region(frame: &VisionFrameView<'_>, region: VisionRect) -> bool {
    region.x == 0
        && region.y == 0
        && region.width == frame.width as i32
        && region.height == frame.height as i32
}

fn average_confidence(values: impl Iterator<Item = f32>) -> Option<f32> {
    let mut count = 0;
    let mut sum = 0.0;
    for value in values {
        count += 1;
        sum += value;
    }
    if count == 0 {
        None
    } else {
        Some(sum / count as f32)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct DecodedText {
    text: String,
    confidence: Option<f32>,
}

fn decode_ctc_output(
    shape: &[i64],
    scores: &[f32],
    dictionary: &[String],
    params: &PpocrCtcParams,
) -> Result<DecodedText, String> {
    let (steps, class_count) = output_layout(shape, scores.len())?;
    if class_count < 2 {
        return Err(
            "PPOCR recognizer output class count must include blank plus labels".to_string(),
        );
    }
    check_class_count(class_count, dictionary.len())?;
    let mut text = String::new();
    let mut previous_index = 0_usize;
    let mut confidences = Vec::new();
    for step in 0..steps {
        let row = &scores[step * class_count..(step + 1) * class_count];
        let (index, confidence) = argmax_with_softmax_confidence(row)?;
        // The class after the dictionary is the space class of a keys+2 recognizer.
        let space =
            index == dictionary.len() + 1 && params.decoder.space_class == SpaceClass::Space;
        if index == 0 || (index > dictionary.len() && !space) {
            previous_index = 0;
            continue;
        }
        if index != previous_index {
            if space {
                text.push(' ');
            } else {
                let label = dictionary.get(index - 1).ok_or_else(|| {
                    format!("PPOCR recognizer class {index} has no dictionary entry")
                })?;
                text.push_str(label);
            }
            confidences.push(confidence);
        }
        previous_index = index;
    }
    let confidence = if confidences.is_empty() {
        None
    } else {
        Some(confidences.iter().sum::<f32>() / confidences.len() as f32)
    };
    Ok(DecodedText { text, confidence })
}

fn output_layout(shape: &[i64], score_len: usize) -> Result<(usize, usize), String> {
    let dims: Vec<_> = shape
        .iter()
        .map(|dim| usize::try_from(*dim).map_err(|err| format!("invalid output dimension: {err}")))
        .collect::<Result<_, _>>()?;
    match dims.as_slice() {
        [1, steps, class_count] if steps * class_count == score_len => Ok((*steps, *class_count)),
        [steps, class_count] if steps * class_count == score_len => Ok((*steps, *class_count)),
        _ => Err(format!(
            "unsupported PPOCR recognizer output shape {shape:?} for {score_len} scores"
        )),
    }
}

fn argmax_with_softmax_confidence(row: &[f32]) -> Result<(usize, f32), String> {
    if row.is_empty() {
        return Err("empty PPOCR recognizer output row".to_string());
    }
    if row.iter().any(|score| !score.is_finite()) {
        return Err("PPOCR recognizer output contains a non-finite score".to_string());
    }
    let mut best_index = 0;
    let mut best_value = row[0];
    for (index, value) in row.iter().copied().enumerate().skip(1) {
        if value > best_value {
            best_index = index;
            best_value = value;
        }
    }
    let row_sum: f32 = row.iter().sum();
    if row_sum.is_finite()
        && (0.99..=1.01).contains(&row_sum)
        && row.iter().all(|value| (0.0..=1.0).contains(value))
    {
        return Ok((best_index, best_value));
    }

    let max_value = row
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, |left, right| left.max(right));
    let exp_sum: f32 = row.iter().map(|value| (*value - max_value).exp()).sum();
    if !exp_sum.is_finite() || exp_sum <= 0.0 {
        return Err("PPOCR recognizer output softmax sum is invalid".to_string());
    }
    Ok((best_index, (best_value - max_value).exp() / exp_sum))
}

#[derive(Debug)]
enum ProviderInvokeError {
    Failure(String),
    Timeout(String),
    /// Workflow #407 GPU-1 B1, B2: a session built on first use (the detector) that could not be
    /// built, with its own code and cause.
    Vision(VisionFfiError),
    /// Workflow #407 GPU-1 E: ONNX Runtime's run of a session failed (not the watchdog's
    /// timeout); on DirectML the model is built again ([`ModelSessions::invocation_failed`]).
    Run(SessionRunFailure),
}

/// Workflow #407 GPU-1 E: a session's failed run: the engine's message, the one a CPU or CUDA
/// session's failed run always had, and ONNX Runtime's status of it.
#[derive(Debug)]
struct SessionRunFailure {
    message: String,
    ort_status: Outcome,
}

impl ProviderInvokeError {
    fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
    }

    /// A session's run that failed with `error`, described by `message`.
    fn run<R>(message: String, error: &ort::Error<R>) -> Self {
        Self::Run(SessionRunFailure {
            message,
            ort_status: onnxruntime_error_status(error),
        })
    }

    fn into_ffi_error(self) -> VisionFfiError {
        match self {
            Self::Failure(message) | Self::Run(SessionRunFailure { message, .. }) => {
                failure(message)
            }
            Self::Timeout(message) => {
                VisionFfiError::fatal_with_code(VisionFfiErrorCode::Timeout, ENGINE_MODULE, message)
            }
            Self::Vision(error) => error,
        }
    }
}

impl From<String> for ProviderInvokeError {
    fn from(message: String) -> Self {
        Self::Failure(message)
    }
}

impl From<VisionFfiError> for ProviderInvokeError {
    fn from(error: VisionFfiError) -> Self {
        Self::Vision(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_vision_ffi::{CudaDeviceSelector, VisionFrame, ppocr_model_content_sha256};

    fn default_params() -> PpocrCtcParams {
        PpocrCtcParams::default()
    }

    #[test]
    fn session_plan_keeps_cpu_and_cuda_provider_registration_disjoint() {
        let cpu = ProviderSessionPlan::from_key(&test_session_key(OnnxExecutionProvider::Cpu))
            .expect("CPU plan");
        let cuda = ProviderSessionPlan::from_key(&test_session_key(OnnxExecutionProvider::Cuda))
            .expect("CUDA plan");

        assert_eq!(cpu.resolved_execution_provider, OnnxExecutionProvider::Cpu);
        assert_eq!(cpu.cuda_ordinal, None);
        assert_eq!(
            cpu.registered_execution_providers,
            [OnnxExecutionProvider::Cpu]
        );
        assert!(cpu.cpu_ep_registered);
        assert!(!cpu.cpu_fallback_disabled);

        assert_eq!(
            cuda.resolved_execution_provider,
            OnnxExecutionProvider::Cuda
        );
        assert_eq!(cuda.cuda_ordinal, Some(1));
        assert_eq!(
            cuda.registered_execution_providers,
            [OnnxExecutionProvider::Cuda]
        );
        assert!(!cuda.cpu_ep_registered);
        assert!(cuda.cpu_fallback_disabled);
    }

    #[test]
    fn node_placement_diagnostic_is_explicit_and_cuda_only() {
        let cpu = ProviderSessionPlan::from_key(&test_session_key(OnnxExecutionProvider::Cpu))
            .expect("CPU plan");
        let cuda = ProviderSessionPlan::from_key(&test_session_key(OnnxExecutionProvider::Cuda))
            .expect("CUDA plan");

        // Workflow #318 cfg3: the value is injected through the request, never read from
        // the environment.
        assert!(!node_placement_diagnostic_requested(&cuda, None).expect("disabled"));
        assert!(
            node_placement_diagnostic_requested(&cuda, Some("1"))
                .expect("explicit CUDA diagnostic")
        );
        let invalid = node_placement_diagnostic_requested(&cuda, Some("true"))
            .expect_err("non-canonical activation rejected");
        assert!(invalid.contains("must be exactly 1"));
        let cpu = node_placement_diagnostic_requested(&cpu, Some("1"))
            .expect_err("CPU diagnostic rejected");
        assert!(cpu.contains("only for an explicit CUDA OCR session"));
    }

    #[test]
    fn node_placement_diagnostic_records_exact_cpu_nodes_without_inference() {
        let mut capture = NodePlacementLogCapture::default();
        for message in [
            "Node placements",
            " Node(s) placed on [CUDAExecutionProvider]. Number of nodes: 2",
            "  Conv (encoder.conv)",
            "  Relu (encoder.relu)",
            " Node(s) placed on [CPUExecutionProvider]. Number of nodes: 2",
            "  Shape (shape_0)",
            "  Gather (gather_0)",
        ] {
            capture.record(message);
        }

        let diagnostic = capture
            .diagnostic(PpocrModelRole::Recognizer)
            .expect("complete diagnostic");
        assert_eq!(diagnostic.model_role, "recognizer");
        assert_eq!(diagnostic.diagnostic_scope, "session_initialization_only");
        assert!(!diagnostic.inference_executed);
        // Workflow #407 GPU-1 B1: the v2 report counts the accelerated provider's nodes too.
        assert_eq!(
            diagnostic.accelerated_execution_provider,
            "CUDAExecutionProvider"
        );
        assert_eq!(diagnostic.accelerated_node_count, 2);
        assert_eq!(diagnostic.device, None);
        assert_eq!(diagnostic.cpu_assigned_node_count, 2);
        assert_eq!(diagnostic.nodes[0].node_name, "gather_0");
        assert_eq!(diagnostic.nodes[0].operator_type, "Gather");
        assert_eq!(diagnostic.nodes[1].node_name, "shape_0");
        assert_eq!(diagnostic.nodes[1].operator_type, "Shape");
        assert!(diagnostic.nodes.iter().all(|node| {
            node.domain == "unavailable"
                && node.placement_reason == PpocrNodePlacementReason::Unavailable
                && node.assigned_execution_provider == CPU_EXECUTION_PROVIDER
        }));

        let json = serde_json::to_value(diagnostic).expect("diagnostic JSON");
        assert_eq!(
            json["record_type"],
            "actingcommand.ppocr_node_placement_diagnostic.v2"
        );
        assert_eq!(json["accelerated_node_count"], 2);
        assert_eq!(json["nodes"].as_array().expect("nodes").len(), 2);
    }

    #[test]
    fn node_placement_diagnostic_fails_closed_on_incomplete_cpu_group() {
        let mut capture = NodePlacementLogCapture::default();
        for message in [
            "Node placements",
            " Node(s) placed on [CPUExecutionProvider]. Number of nodes: 2",
            "  Shape (shape_0)",
        ] {
            capture.record(message);
        }

        let err = capture
            .diagnostic(PpocrModelRole::Recognizer)
            .expect_err("incomplete diagnostic rejected");
        assert!(err.contains("reported 2 CPU-assigned nodes but emitted details for 1"));
    }

    #[test]
    fn node_placement_diagnostic_reports_all_cuda_without_cpu_nodes() {
        let mut capture = NodePlacementLogCapture::default();
        capture.record("Node placements");
        capture.record(" All nodes placed on [CUDAExecutionProvider]. Number of nodes: 9");

        let diagnostic = capture
            .diagnostic(PpocrModelRole::Detector)
            .expect("all-CUDA diagnostic");
        assert_eq!(diagnostic.model_role, "detector");
        assert_eq!(diagnostic.diagnostic_scope, "session_initialization_only");
        assert!(!diagnostic.inference_executed);
        // Workflow #407 GPU-1 B1: the v2 report counts the accelerated provider's nodes too.
        assert_eq!(diagnostic.record_type, PPOCR_NODE_PLACEMENT_RECORD_TYPE);
        assert_eq!(
            diagnostic.accelerated_execution_provider,
            "CUDAExecutionProvider"
        );
        assert_eq!(diagnostic.accelerated_node_count, 9);
        assert_eq!(diagnostic.cpu_assigned_node_count, 0);
        assert!(diagnostic.nodes.is_empty());
    }

    #[test]
    fn decodes_ctc_output_with_blank_and_repeated_labels() {
        let dictionary = vec!["A".to_string(), "B".to_string()];
        let scores = vec![
            4.0, 0.0, 0.0, //
            0.0, 5.0, 0.0, //
            0.0, 6.0, 0.0, //
            5.0, 0.0, 0.0, //
            0.0, 0.0, 5.0, //
        ];

        let decoded =
            decode_ctc_output(&[1, 5, 3], &scores, &dictionary, &default_params()).expect("decode");

        assert_eq!(decoded.text, "AB");
        assert!(decoded.confidence.expect("confidence") > 0.9);
    }

    #[test]
    fn rejects_output_without_dictionary_entry() {
        let dictionary = vec!["A".to_string()];
        let scores = vec![0.0; 8];

        let err = decode_ctc_output(&[1, 2, 4], &scores, &dictionary, &default_params())
            .expect_err("too many classes");

        assert!(err.contains("dictionary"));
    }

    #[test]
    fn treats_extra_ppocr_class_as_special_token() {
        let dictionary = vec!["A".to_string()];
        let scores = vec![
            0.0, 5.0, 0.0, //
            0.0, 0.0, 5.0, //
            0.0, 5.0, 0.0, //
        ];

        let decoded =
            decode_ctc_output(&[1, 3, 3], &scores, &dictionary, &default_params()).expect("decode");

        assert_eq!(decoded.text, "AA");
    }

    #[test]
    fn confidence_uses_existing_probability_distribution() {
        let (index, confidence) =
            argmax_with_softmax_confidence(&[0.1, 0.8, 0.1]).expect("confidence");

        assert_eq!(index, 1);
        assert!((confidence - 0.8).abs() < 0.001);
    }

    #[test]
    fn converts_region_to_nchw_recognition_tensor() {
        let frame = VisionFrame {
            width: 2,
            height: 1,
            pixel_format: VisionPixelFormat::Rgb8,
            pixels: vec![0, 127, 255, 255, 127, 0],
        };
        let shape = RecognitionInputShape {
            height: 1,
            width: 2,
        };

        let tensor = frame_region_to_recognition_tensor(
            &frame.view(),
            VisionRect {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
            &shape,
            &default_params(),
        )
        .expect("tensor");

        assert_eq!(tensor.len(), 6);
        assert!((tensor[0] - 1.0).abs() < 0.001);
        assert!((tensor[1] + 1.0).abs() < 0.001);
        assert!((tensor[2] - (127.0 / 127.5 - 1.0)).abs() < 0.001);
        assert_eq!(tensor[2], tensor[3]);
        assert!((tensor[4] + 1.0).abs() < 0.001);
        assert!((tensor[5] - 1.0).abs() < 0.001);
    }

    #[test]
    fn recognition_resize_samples_linear_bgr_inside_region_and_zero_pads() {
        // Workflow #269 OCR-RECOGNIZER-PREPROCESS-v1: fixed model RecResizeImg spec.
        let mut frame = VisionFrame {
            width: 5,
            height: 4,
            pixel_format: VisionPixelFormat::Rgb8,
            pixels: vec![255; 5 * 4 * 3],
        };
        let source = [
            [[0, 30, 240], [60, 90, 180], [120, 150, 120]],
            [[120, 90, 120], [180, 150, 60], [240, 210, 0]],
        ];
        for (y, row) in source.iter().enumerate() {
            for (x, pixel) in row.iter().enumerate() {
                let offset = ((y + 1) * 5 + x + 1) * 3;
                frame.pixels[offset..offset + 3].copy_from_slice(pixel);
            }
        }
        let original_pixels = frame.pixels.clone();
        let region = VisionRect {
            x: 1,
            y: 1,
            width: 3,
            height: 2,
        };
        let shape = RecognitionInputShape {
            height: 3,
            width: 7,
        };
        let tensor =
            frame_region_to_recognition_tensor(&frame.view(), region, &shape, &default_params())
                .expect("tensor");
        // ceil(3 / 2 * 3) = 5 columns. Half-pixel positions are -0.2, 0.4,
        // 1, 1.6, 2.2 horizontally and -1/6, 1/2, 7/6 vertically.
        let expected_bgr: [[u8; 15]; 3] = [
            [
                240, 216, 180, 144, 120, 180, 156, 120, 84, 60, 120, 96, 60, 24, 0,
            ],
            [
                30, 54, 90, 126, 150, 60, 84, 120, 156, 180, 90, 114, 150, 186, 210,
            ],
            [
                0, 24, 60, 96, 120, 60, 84, 120, 156, 180, 120, 144, 180, 216, 240,
            ],
        ];
        assert_eq!(tensor.len(), 3 * 3 * 7);
        assert_eq!(shape.to_ort_shape(), [1, 3, 3, 7]);
        for (channel, expected) in expected_bgr.iter().enumerate() {
            for y in 0..3 {
                for x in 0..5 {
                    let value = f32::from(expected[y * 5 + x]) / 127.5 - 1.0;
                    assert!((tensor[channel * 21 + y * 7 + x] - value).abs() < 0.000001);
                }
                assert_eq!(
                    &tensor[channel * 21 + y * 7 + 5..channel * 21 + y * 7 + 7],
                    &[0.0, 0.0]
                );
            }
        }
        assert_eq!(frame.pixels, original_pixels);
        assert_eq!(
            dynamic_width_for_region(region, 48, &default_params()).expect("width"),
            72
        );
        assert_eq!(
            dynamic_width_for_region(VisionRect { width: 1, ..region }, 48, &default_params())
                .expect("minimum"),
            32
        );
        assert_eq!(
            dynamic_width_for_region(
                VisionRect {
                    width: 100,
                    ..region
                },
                48,
                &default_params()
            )
            .expect("maximum"),
            320
        );
        assert!(
            dynamic_width_for_region(
                VisionRect {
                    height: 0,
                    ..region
                },
                48,
                &default_params()
            )
            .is_err()
        );
        assert_eq!(
            positive_or_default(-1, 48, "recognizer height").expect("dynamic height"),
            48
        );
        assert!(positive_or_default(0, 48, "recognizer height").is_err());
        for invalid in [
            VisionRect { x: -1, ..region },
            VisionRect { width: 0, ..region },
            VisionRect {
                height: 4,
                ..region
            },
        ] {
            assert!(
                frame_region_to_recognition_tensor(
                    &frame.view(),
                    invalid,
                    &shape,
                    &default_params()
                )
                .is_err()
            );
        }
        frame.pixels.truncate(3);
        let error =
            frame_region_to_recognition_tensor(&frame.view(), region, &shape, &default_params())
                .expect_err("short pixels");
        assert!(error.contains("pixel buffer ended"));
    }

    #[test]
    fn recognition_resize_quantizes_uint8_and_caps_width_for_rgba_and_gray() {
        let region = VisionRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let shape = RecognitionInputShape {
            height: 2,
            width: 3,
        };
        for (pixel_format, pixels, expected) in [
            (
                VisionPixelFormat::Rgba8,
                vec![0, 1, 0, 255, 1, 0, 1, 0],
                [
                    [0_u8, 0, 1, 0, 0, 1],
                    [1, 0, 0, 1, 0, 0],
                    [0, 0, 1, 0, 0, 1],
                ],
            ),
            (
                VisionPixelFormat::Gray8,
                vec![0, 1],
                [[0, 0, 1, 0, 0, 1]; 3],
            ),
        ] {
            let frame = VisionFrame {
                width: 2,
                height: 1,
                pixel_format,
                pixels,
            };
            let tensor = frame_region_to_recognition_tensor(
                &frame.view(),
                region,
                &shape,
                &default_params(),
            )
            .expect("tensor");
            // Aspect width 4 caps to 3. The middle uint8 sample is 0: retaining
            // the clamped row weights 1/4 and 3/4 gives (0 + 1 + 2) >> 2.
            assert_eq!(tensor.len(), 18);
            for (actual, expected) in tensor.iter().zip(expected.into_iter().flatten()) {
                assert!((*actual - (f32::from(expected) / 127.5 - 1.0)).abs() < 0.000001);
            }
        }
    }

    #[test]
    fn parses_detector_probability_map() {
        let map = detector_probability_map(&[1, 1, 2, 3], &[0.0, 0.25, 0.5, 0.75, 1.0, 2.0])
            .expect("map");

        assert_eq!(map.width, 3);
        assert_eq!(map.height, 2);
        assert_eq!(map.values[4], 1.0);
        assert!(map.values[5] > 0.88);
    }

    #[test]
    fn detects_and_merges_nearby_text_components() {
        let mut values = vec![0.0; 8 * 3];
        for y in 1..3 {
            for x in 1..3 {
                values[y * 8 + x] = 0.9;
            }
            for x in 4..6 {
                values[y * 8 + x] = 0.8;
            }
        }
        let map = ProbabilityMap {
            width: 8,
            height: 3,
            values,
        };

        let frame_region = VisionRect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };
        let boxes = detect_text_boxes_from_probability_map(&map, frame_region, &default_params())
            .expect("boxes");
        let merged = merge_detected_text_boxes(
            boxes,
            frame_region,
            &default_params(),
            Some(default_params().detector.merge_gap_factor),
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].rect.x, 0);
        assert!(merged[0].rect.width >= 60);
        assert!(merged[0].confidence > 0.89);
    }

    // Workflow #396 REC-1: pure-function tests of the read route, the per-request line merge,
    // the clip to the requested region and the box count before truncation.

    /// A probability map whose cells inside each `(x, y, width, height)` box are text.
    fn text_map(
        width: usize,
        height: usize,
        boxes: &[(usize, usize, usize, usize)],
    ) -> ProbabilityMap {
        let mut values = vec![0.0; width * height];
        for (left, top, box_width, box_height) in boxes {
            for y in *top..top + box_height {
                for x in *left..left + box_width {
                    values[y * width + x] = 0.9;
                }
            }
        }
        ProbabilityMap {
            width,
            height,
            values,
        }
    }

    fn rect_within(inner: VisionRect, outer: VisionRect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.x + inner.width <= outer.x + outer.width
            && inner.y + inner.height <= outer.y + outer.height
    }

    fn rect_region() -> VisionRect {
        VisionRect {
            x: 40,
            y: 40,
            width: 400,
            height: 100,
        }
    }

    fn zero_frame(width: u32, height: u32) -> VisionFrame {
        VisionFrame {
            width,
            height,
            pixel_format: VisionPixelFormat::Rgb8,
            pixels: vec![0; (width * height * 3) as usize],
        }
    }

    #[test]
    fn read_route_follows_the_declared_blocks() {
        let frame = zero_frame(640, 360);
        let full = VisionRect {
            x: 0,
            y: 0,
            width: 640,
            height: 360,
        };
        let route = |region, blocks| read_route(&frame.view(), region, blocks, 3);

        assert_eq!(
            route(full, OcrBlockMode::Implicit),
            ReadRoute::Detect {
                merge_gap_factor: Some(3)
            }
        );
        assert_eq!(
            route(rect_region(), OcrBlockMode::Implicit),
            ReadRoute::Region
        );
        assert_eq!(
            route(rect_region(), OcrBlockMode::Region),
            ReadRoute::Region
        );
        assert_eq!(
            route(
                rect_region(),
                OcrBlockMode::Detect {
                    line_merge: OcrLineMerge::GapFactor(0)
                }
            ),
            ReadRoute::Detect {
                merge_gap_factor: Some(0)
            }
        );
        assert_eq!(
            route(
                rect_region(),
                OcrBlockMode::Detect {
                    line_merge: OcrLineMerge::Never
                }
            ),
            ReadRoute::Detect {
                merge_gap_factor: None
            }
        );
    }

    #[test]
    fn rect_detect_reads_each_box_and_region_reads_one_block() {
        let region = rect_region();
        let map = text_map(400, 100, &[(20, 30, 80, 20), (250, 30, 80, 20)]);

        let boxes =
            detect_text_boxes_from_probability_map(&map, region, &default_params()).expect("boxes");
        let detected = merge_detected_text_boxes(boxes, region, &default_params(), Some(0));

        assert_eq!(detected.len(), 2, "each box keeps its own rectangle");
        assert!(
            detected
                .iter()
                .all(|text_box| rect_within(text_box.rect, region))
        );
        assert!(rect_within(
            detected[0].rect,
            VisionRect {
                x: 40,
                y: 40,
                width: 200,
                height: 100
            }
        ));
        assert!(rect_within(
            detected[1].rect,
            VisionRect {
                x: 240,
                y: 40,
                width: 200,
                height: 100
            }
        ));

        let whole = canonical_roi_result(
            DecodedText {
                text: "ALPHA BETA".to_string(),
                confidence: Some(0.9),
            },
            region,
        );
        assert_eq!(whole.blocks.len(), 1, "a region read is one block");
        assert_eq!(whole.blocks[0].rect, region);
    }

    #[test]
    fn line_merge_follows_the_request() {
        let region = VisionRect {
            x: 0,
            y: 0,
            width: 400,
            height: 100,
        };
        let params = default_params();
        let merged = |gap: usize, factor: Option<u32>| {
            let map = text_map(400, 100, &[(50, 40, 100, 20), (150 + gap, 40, 100, 20)]);
            let boxes =
                detect_text_boxes_from_probability_map(&map, region, &params).expect("boxes");
            assert_eq!(boxes.len(), 2);
            merge_detected_text_boxes(boxes, region, &params, factor).len()
        };

        // Two names 40 px apart: 8 px apart after the 16 px padding.
        assert_eq!(
            merged(40, Some(params.detector.merge_gap_factor)),
            1,
            "the folder's factor merges"
        );
        assert_eq!(merged(40, Some(0)), 2, "gap_factor 0 keeps them apart");
        // 20 px apart: the padded boxes overlap.
        assert_eq!(
            merged(20, Some(0)),
            1,
            "gap_factor 0 merges overlapping boxes"
        );
        assert_eq!(merged(20, None), 2, "none merges nothing");
    }

    #[test]
    fn padded_boxes_clip_to_the_requested_region() {
        let region = rect_region();
        let map = text_map(400, 100, &[(0, 0, 10, 10), (390, 90, 10, 10)]);

        let boxes =
            detect_text_boxes_from_probability_map(&map, region, &default_params()).expect("boxes");

        assert_eq!(boxes.len(), 2);
        assert_eq!(
            boxes[0].rect,
            VisionRect {
                x: 40,
                y: 40,
                width: 26,
                height: 26
            }
        );
        assert_eq!(
            boxes[1].rect,
            VisionRect {
                x: 414,
                y: 114,
                width: 26,
                height: 26
            }
        );
        assert!(
            boxes
                .iter()
                .all(|text_box| rect_within(text_box.rect, region))
        );
    }

    #[test]
    fn detection_count_reports_boxes_beyond_max_boxes() {
        let boxes = |count: i32| {
            (0..count)
                .map(|index| DetectedTextBox {
                    rect: VisionRect {
                        x: index * 4,
                        y: 0,
                        width: 2,
                        height: 2,
                    },
                    confidence: 0.9,
                })
                .collect::<Vec<_>>()
        };

        let (kept, count) = keep_detected_boxes(boxes(65), 64);
        assert_eq!(kept.len(), 64);
        assert_eq!(
            count,
            OcrDetectionCount {
                detected: 65,
                limit: 64
            }
        );

        let (kept, count) = keep_detected_boxes(boxes(64), 64);
        assert_eq!(kept.len(), 64);
        assert_eq!(
            count,
            OcrDetectionCount {
                detected: 64,
                limit: 64
            }
        );
    }

    #[test]
    fn distinguishes_full_frame_from_sub_frame_region() {
        let frame = VisionFrame {
            width: 320,
            height: 80,
            pixel_format: VisionPixelFormat::Rgb8,
            pixels: vec![0; 320 * 80 * 3],
        };

        assert!(is_full_frame_region(
            &frame.view(),
            VisionRect {
                x: 0,
                y: 0,
                width: 320,
                height: 80,
            }
        ));
        assert!(!is_full_frame_region(
            &frame.view(),
            VisionRect {
                x: 1,
                y: 0,
                width: 319,
                height: 80,
            }
        ));
    }

    #[test]
    fn canonical_sub_frame_roi_result_preserves_region_without_warning() {
        let frame = VisionFrame {
            width: 320,
            height: 80,
            pixel_format: VisionPixelFormat::Rgb8,
            pixels: vec![0; 320 * 80 * 3],
        };
        let region = VisionRect {
            x: 16,
            y: 8,
            width: 160,
            height: 32,
        };
        assert!(!is_full_frame_region(&frame.view(), region));

        let result = canonical_roi_result(
            DecodedText {
                text: "home".to_string(),
                confidence: Some(0.99),
            },
            region,
        );

        assert_eq!(result.text, "home");
        assert_eq!(result.confidence, Some(0.99));
        assert_eq!(result.backend, VisionBackendKind::FastDeployPpocr);
        assert!(result.warnings.is_empty());
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].text, "home");
        assert_eq!(result.blocks[0].rect, region);
        assert_eq!(result.blocks[0].confidence, Some(0.99));
    }

    fn test_session_key(backend: OnnxExecutionProvider) -> OcrSessionKey {
        let detector_hash = "a".repeat(64);
        let recognizer_hash = "b".repeat(64);
        let dictionary_hash = "c".repeat(64);
        let model_hash =
            ppocr_model_content_sha256(&detector_hash, &recognizer_hash, &dictionary_hash, None)
                .expect("model hash");
        let selector = CudaDeviceSelector {
            ordinal: 1,
            expected_stable_identity: "cuda-uuid:11111111111111111111111111111111".to_string(),
        };
        let resolved = (backend == OnnxExecutionProvider::Cuda).then_some(CudaDeviceIdentity {
            ordinal: 1,
            stable_identity: "cuda-uuid:11111111111111111111111111111111".to_string(),
            pci_bus_id: Some("0000:02:00.0".to_string()),
        });
        OcrSessionKey::from_parts(OcrSessionKeyParts {
            engine_binding_sha256: "e".repeat(64),
            runtime_library_path: "onnxruntime.dll".to_string(),
            runtime_library_sha256: "d".repeat(64),
            onnxruntime_version: "1.24.0-test".to_string(),
            model_ref: "PP-OCRv6_medium".to_string(),
            model_sha256: model_hash,
            requested_backend: backend,
            requested_cuda_device: (backend == OnnxExecutionProvider::Cuda).then_some(selector),
            resolved_cuda_device: resolved,
            requested_directml_device: None,
            resolved_directml_device: None,
        })
        .expect("session key")
    }
}
