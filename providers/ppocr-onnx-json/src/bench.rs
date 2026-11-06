// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-0a (experiment branch `exp/407-gpu0-bench` only, never merged): what the
//! vision EP bench `ac-vision-ep-bench` needs from the engine to time a raw ONNX Runtime
//! session with exactly the engine's session options and the engine's input tensors. Nothing
//! here changes what the engine does; actingd never calls it.

use super::{
    CPU_EXECUTION_PROVIDER, ENGINE_MODULE, OrtStepFailure, ProviderSessionPlan, commit_ort_session,
    failure, frame_region_to_detection_tensor, frame_region_to_recognition_tensor, runtime_facts,
    select_detection_input_shape, select_recognition_input_shape,
};
use actingcommand_vision_ffi::{
    CudaDeviceIdentity, OnnxExecutionProvider, PpocrCtcParams, VisionFfiError, VisionFfiErrorCode,
    VisionFfiResult, VisionFrameView, VisionRect, VisionRuntimeSpec,
};
use ort::logging::{LogLevel, LoggerFunction};
use ort::session::Session;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

/// Workflow #407 GPU-0c: a failed session build, with ONNX Runtime's own status kept whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchOrtFailure {
    /// What the engine was doing, as its own error text words it.
    pub step: String,
    /// ONNX Runtime's error code; none for a step that is no ONNX Runtime call.
    pub ort_code: Option<ort::ErrorCode>,
    /// ONNX Runtime's status message, verbatim, with its causes.
    pub ort_message: Option<String>,
}

fn bench_failure(failure: OrtStepFailure) -> BenchOrtFailure {
    BenchOrtFailure {
        step: failure.step,
        ort_code: failure.ort_code,
        ort_message: failure.ort_message,
    }
}

impl std::fmt::Display for BenchOrtFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.step)?;
        if let Some(message) = &self.ort_message {
            write!(f, ": {message}")?;
        }
        if let Some(code) = &self.ort_code {
            write!(f, " (ONNX Runtime error code {code:?})")?;
        }
        Ok(())
    }
}

/// Builds one session from `model` with the engine's own session builder for `plan` (no
/// node-placement diagnostic); a failure keeps ONNX Runtime's status whole.
pub fn build_session(
    model: &[u8],
    path: &Path,
    plan: &BenchSessionPlan,
) -> Result<Session, BenchOrtFailure> {
    commit_ort_session(model, path, &plan.0, None).map_err(bench_failure)
}

/// Workflow #407 GPU-0c: where ONNX Runtime placed one model's nodes, read from ONNX Runtime's
/// own verbose session log (the `Node placements` section it logs once the graph is
/// partitioned, before its CPU-fallback check) of a session built with the plan's exact options
/// plus a verbose logger. No inference runs; the session is dropped at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchPlacement {
    /// `same_options`, or `fallback_allowed_twin` when the same-options session failed without
    /// logging a whole placement and a twin with CPU fallback allowed was read instead.
    pub read_from: &'static str,
    /// Every execution provider ONNX Runtime named, with its node count, in log order.
    pub providers: Vec<(String, usize)>,
    /// The nodes ONNX Runtime placed on its CPU execution provider: (operator type, node name).
    pub cpu_nodes: Vec<(String, String)>,
    /// The same-options session's failure, when it did not commit.
    pub session_failure: Option<BenchOrtFailure>,
    /// Why no whole placement could be read; none when it was.
    pub log_error: Option<String>,
}

/// The most per-node lines the placement read keeps.
const PLACEMENT_MAX_NODES: usize = 65_536;

/// Reads the node placement of `model` under `plan` (see [`BenchPlacement`]).
pub fn node_placement(model: &[u8], path: &Path, plan: &BenchSessionPlan) -> BenchPlacement {
    let (read, session_failure) = placement_session(model, path, &plan.0);
    match read {
        Ok((providers, cpu_nodes)) => BenchPlacement {
            read_from: "same_options",
            providers,
            cpu_nodes,
            session_failure,
            log_error: None,
        },
        Err(log_error) if session_failure.is_some() && plan.0.cpu_fallback_disabled => {
            let mut twin = plan.0.clone();
            twin.cpu_fallback_disabled = false;
            let (twin_read, twin_failure) = placement_session(model, path, &twin);
            match (twin_read, twin_failure) {
                (Ok((providers, cpu_nodes)), _) => BenchPlacement {
                    read_from: "fallback_allowed_twin",
                    providers,
                    cpu_nodes,
                    session_failure,
                    log_error: None,
                },
                (Err(twin_error), twin_failure) => BenchPlacement {
                    read_from: "same_options",
                    providers: Vec::new(),
                    cpu_nodes: Vec::new(),
                    session_failure,
                    log_error: Some(format!(
                        "{log_error}; the fallback-allowed twin: {twin_error}{}",
                        twin_failure
                            .map(|failure| format!(" (its session: {failure})"))
                            .unwrap_or_default()
                    )),
                },
            }
        }
        Err(log_error) => BenchPlacement {
            read_from: "same_options",
            providers: Vec::new(),
            cpu_nodes: Vec::new(),
            session_failure,
            log_error: Some(log_error.to_string()),
        },
    }
}

/// Workflow #407 GPU-0f: why no whole node placement could be read from ONNX Runtime's log.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PlacementLogError {
    LogPoisoned,
    NoSection,
    NoProvider,
    /// A provider summary that does not read as `[<EP>]. Number of nodes: <n>`.
    UnreadableSummary {
        line: String,
    },
    /// A per-node line that does not read as `<op type> (<node name>)`.
    UnreadableNode {
        provider: String,
        line: String,
    },
    /// The next provider summary came while per-node lines of this provider were still due.
    GroupEndedEarly {
        provider: String,
        due: usize,
    },
    /// The log ended while per-node lines of this provider were still due.
    GroupIncomplete {
        provider: String,
        due: usize,
    },
    TooManyCpuNodes,
}

impl std::fmt::Display for PlacementLogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LogPoisoned => f.write_str("the node-placement log mutex is poisoned"),
            Self::NoSection => f.write_str("ONNX Runtime logged no `Node placements` section"),
            Self::NoProvider => {
                f.write_str("ONNX Runtime's `Node placements` section named no execution provider")
            }
            Self::UnreadableSummary { line } => write!(
                f,
                "a provider summary does not read as `[<EP>]. Number of nodes: <n>`: {line}"
            ),
            Self::UnreadableNode { provider, line } => write!(
                f,
                "a per-node line of {provider} does not read as `<op type> (<node name>)`: {line}"
            ),
            Self::GroupEndedEarly { provider, due } => write!(
                f,
                "the placement group of {provider} ended {due} per-node lines early"
            ),
            Self::GroupIncomplete { provider, due } => write!(
                f,
                "the placement group of {provider} is missing {due} per-node lines"
            ),
            Self::TooManyCpuNodes => {
                f.write_str("more CPU-placed nodes than the placement read keeps")
            }
        }
    }
}

type PlacementRead = Result<(Vec<(String, usize)>, Vec<(String, String)>), PlacementLogError>;

fn placement_session(
    model: &[u8],
    path: &Path,
    plan: &ProviderSessionPlan,
) -> (PlacementRead, Option<BenchOrtFailure>) {
    let log = Arc::new(Mutex::new(PlacementLog::default()));
    let sink = Arc::clone(&log);
    let logger: LoggerFunction = Arc::new(
        move |_: LogLevel, _: &str, _: &str, _: &str, message: &str| match sink.lock() {
            Ok(mut log) => log.record(message),
            Err(poisoned) => poisoned.into_inner().fail(PlacementLogError::LogPoisoned),
        },
    );
    let session = commit_ort_session(model, path, plan, Some(logger));
    let session_failure = session.as_ref().err().cloned().map(bench_failure);
    drop(session);
    let read = match log.lock() {
        Ok(mut log) => std::mem::take(&mut *log).finish(),
        Err(_) => Err(PlacementLogError::LogPoisoned),
    };
    (read, session_failure)
}

/// ONNX Runtime's verbose `Node placements` section: `All nodes placed on [<EP>]. Number of
/// nodes: <n>` for one provider, or per provider `Node(s) placed on [<EP>]. Number of nodes: <n>`
/// followed by one `<op type> (<node name>)` line per node.
#[derive(Debug, Default)]
struct PlacementLog {
    saw_header: bool,
    providers: Vec<(String, usize)>,
    /// The provider whose per-node lines follow, and how many are still due.
    open: Option<(usize, usize)>,
    cpu_nodes: Vec<(String, String)>,
    failure: Option<PlacementLogError>,
}

impl PlacementLog {
    fn record(&mut self, message: &str) {
        if self.failure.is_some() {
            return;
        }
        let message = message.trim();
        if message == "Node placements" {
            *self = Self {
                saw_header: true,
                ..Self::default()
            };
            return;
        }
        if !self.saw_header {
            return;
        }
        for (prefix, detailed) in [
            ("All nodes placed on [", false),
            ("Node(s) placed on [", true),
        ] {
            let Some(rest) = message.strip_prefix(prefix) else {
                continue;
            };
            if let Some((index, due)) = self.open {
                self.fail(PlacementLogError::GroupEndedEarly {
                    provider: self.providers[index].0.clone(),
                    due,
                });
                return;
            }
            let parsed = rest
                .split_once("]. Number of nodes: ")
                .filter(|(provider, _)| !provider.is_empty())
                .and_then(|(provider, count)| {
                    count
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .map(|count| (provider, count))
                });
            let Some((provider, count)) = parsed else {
                self.fail(PlacementLogError::UnreadableSummary {
                    line: bounded(message),
                });
                return;
            };
            self.providers.push((provider.to_owned(), count));
            if detailed && count > 0 {
                self.open = Some((self.providers.len() - 1, count));
            }
            return;
        }
        let Some((index, due)) = self.open else {
            return;
        };
        // A node ONNX Runtime inserted may have an empty name; its operator type is never empty.
        let node = message
            .split_once(" (")
            .and_then(|(op, rest)| rest.strip_suffix(')').map(|name| (op, name)))
            .filter(|(op, _)| !op.is_empty());
        let Some((op, name)) = node else {
            self.fail(PlacementLogError::UnreadableNode {
                provider: self.providers[index].0.clone(),
                line: bounded(message),
            });
            return;
        };
        if self.providers[index].0 == CPU_EXECUTION_PROVIDER {
            if self.cpu_nodes.len() >= PLACEMENT_MAX_NODES {
                self.fail(PlacementLogError::TooManyCpuNodes);
                return;
            }
            self.cpu_nodes.push((op.to_owned(), name.to_owned()));
        }
        self.open = (due > 1).then_some((index, due - 1));
    }

    fn fail(&mut self, error: PlacementLogError) {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        self.open = None;
    }

    fn finish(self) -> PlacementRead {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if !self.saw_header {
            return Err(PlacementLogError::NoSection);
        }
        if self.providers.is_empty() {
            return Err(PlacementLogError::NoProvider);
        }
        if let Some((index, due)) = self.open {
            return Err(PlacementLogError::GroupIncomplete {
                provider: self.providers[index].0.clone(),
                due,
            });
        }
        Ok((self.providers, self.cpu_nodes))
    }
}

fn bounded(message: &str) -> String {
    const LIMIT: usize = 256;
    if message.len() <= LIMIT {
        return format!("{message:?}");
    }
    let mut end = LIMIT;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{:?}…", &message[..end])
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
