// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use crate::{ExecutionKernelError, ExecutionKernelResult};
pub use actingcommand_contract::ExecutionBackendProvenance;
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ApplicationLifecycleAction, EmulatorInstanceAction, FencedWrite, InstanceId,
    MonitorObservation, VisionModelKind, VisionModelMemoryStatus, VisionModelTimingStatus,
};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceErrorCategory, DeviceErrorSensitivity, DeviceResult, Frame,
    InputBackend,
};
pub use actingcommand_device::{
    DeviceProcessPresence, EmulatorControlFailure, EmulatorControlOutcome, EmulatorControlResult,
    InstanceState,
};
pub use actingcommand_recognition_pack::VisionProvider as RecognitionVisionProvider;
use actingcommand_recognition_pack::{
    NnProviderLabel, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderBlocks, OcrProviderDetection, OcrProviderExecutionEvidence, OcrProviderObservation,
    OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock, PackRect, VisionProviderError,
    VisionProviderErrorCode, VisionProviderFrame,
};
use actingcommand_vision_ffi::{
    NnClassificationResult, NnEngine, NnInferenceRequest, NnModelLoad, NnModelSpec, OcrBlockMode,
    OcrEngine, OcrExecutionAttestation, OcrExecutionStatus, OcrFallbackPolicy,
    OcrInferenceRequestView, OcrInferenceResult, OcrLineMerge, OcrModelLoad, OcrModelSpec,
    OcrSessionId, OnnxExecutionProvider, VisionBackendKind, VisionFfiError, VisionFfiErrorCode,
    VisionFfiResult, VisionFrame, VisionFrameView, VisionModelListing, VisionModelLoader,
    VisionPixelFormat, VisionRect, next_ocr_session_id,
};
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::time::{Duration, Instant};

const MAX_MODEL_REF_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionModelIdentity {
    model_ref: String,
    model_sha256: String,
}

impl VisionModelIdentity {
    pub fn new(
        model_ref: impl Into<String>,
        model_sha256: impl Into<String>,
    ) -> Result<Self, VisionProviderError> {
        let model_ref = model_ref.into();
        let model_sha256 = model_sha256.into();
        if model_ref.trim().is_empty()
            || model_ref.len() > MAX_MODEL_REF_BYTES
            || model_ref.contains(['/', '\\', ':'])
            || model_ref.chars().any(char::is_control)
        {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::ModelMismatch,
                "vision model_ref must be a bounded logical identifier, not a host path",
            ));
        }
        if model_sha256.len() != 64
            || !model_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::ModelMismatch,
                "vision model_sha256 must be exactly 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self {
            model_ref,
            model_sha256,
        })
    }

    pub fn model_ref(&self) -> &str {
        &self.model_ref
    }

    pub fn model_sha256(&self) -> &str {
        &self.model_sha256
    }
}

/// The bound on lazily loaded vision models kept in memory at once (Workflow #360); the least
/// recently used idle model is unloaded to make room.
pub const MAX_LOADED_VISION_MODELS: usize = 4;

static NEXT_MODEL_USE: AtomicU64 = AtomicU64::new(1);

struct SlotCell<E: ?Sized> {
    engine: Option<Box<E>>,
    retired: Option<String>,
    session_id: Option<OcrSessionId>,
    /// The engine was built by the loader and counts toward `MAX_LOADED_VISION_MODELS`.
    counted: bool,
}

/// #407 PERF-READ: a monotonic clock in microseconds, which a request's lock wait and inference
/// are timed on.
pub type VisionTimingClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// #407 PERF-READ: the process's private bytes now, `None` when the sample failed; read before
/// and after each model load.
pub type PrivateBytesSampler = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

/// The provider's clock: microseconds since the provider was made.
fn monotonic_clock() -> VisionTimingClock {
    let origin = Instant::now();
    Arc::new(move || u64::try_from(origin.elapsed().as_micros()).unwrap_or(u64::MAX))
}

/// #407 PERF-READ: one model's account, beside its lock so the status read never waits for an
/// inference: whether an engine is in the slot (with its load's private-bytes growth when it was
/// measured), and its requests since the start.
#[derive(Debug, Default)]
struct ModelStats {
    loaded: Option<Option<u64>>,
    /// Workflow #407 GPU-1 C fix 2 (design §2.5): where the slot's loaded OCR engine runs, as
    /// its load handed it over (slice B2's `OcrModelLoad::Loaded.execution`), live while the
    /// engine is in the slot; none for an NN model, an engine built before the provider, or no
    /// engine.
    execution: Option<OcrExecutionStatus>,
    requests: u64,
    inferences: u64,
    lock_wait_total_us: u64,
    lock_wait_longest_us: u64,
    inference_total_us: u64,
    inference_longest_us: u64,
}

/// #407 PERF-READ: one request's times on one model. Started before the request waits for the
/// model's lock, `locked` once it holds it, `inference` around the engine's call; a load between
/// the two is in neither time. Dropped, it adds the request to the model's account once, however
/// the request ended (a lock wait that timed out counts until the drop).
struct RequestTiming<'a> {
    stats: &'a Mutex<ModelStats>,
    clock: &'a VisionTimingClock,
    started_us: u64,
    lock_wait_us: Option<u64>,
    inference_us: Option<u64>,
}

impl<'a> RequestTiming<'a> {
    fn start(stats: &'a Mutex<ModelStats>, clock: &'a VisionTimingClock) -> Self {
        Self {
            stats,
            clock,
            started_us: clock(),
            lock_wait_us: None,
            inference_us: None,
        }
    }

    fn locked(&mut self) {
        self.lock_wait_us = Some((self.clock)().saturating_sub(self.started_us));
    }

    fn inference<R>(&mut self, call: impl FnOnce() -> R) -> R {
        let started_us = (self.clock)();
        let result = call();
        self.inference_us = Some((self.clock)().saturating_sub(started_us));
        result
    }
}

impl Drop for RequestTiming<'_> {
    fn drop(&mut self) {
        let lock_wait_us = self
            .lock_wait_us
            .unwrap_or_else(|| (self.clock)().saturating_sub(self.started_us));
        let mut stats = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        stats.requests = stats.requests.saturating_add(1);
        stats.lock_wait_total_us = stats.lock_wait_total_us.saturating_add(lock_wait_us);
        stats.lock_wait_longest_us = stats.lock_wait_longest_us.max(lock_wait_us);
        if let Some(inference_us) = self.inference_us {
            stats.inferences = stats.inferences.saturating_add(1);
            stats.inference_total_us = stats.inference_total_us.saturating_add(inference_us);
            stats.inference_longest_us = stats.inference_longest_us.max(inference_us);
        }
    }
}

/// One model: its own lock serialises its load and its inferences, never another model's.
struct ModelSlot<E: ?Sized, S> {
    model_ref: String,
    spec: Option<S>,
    admitted_sha256: Option<String>,
    /// The content identity established by the first hash of the model's files.
    verified_sha256: OnceLock<String>,
    cell: Mutex<SlotCell<E>>,
    last_used: AtomicU64,
    /// #407 PERF-READ: a leaf lock, never held across a load or an inference.
    stats: Mutex<ModelStats>,
}

type OcrSlot = ModelSlot<dyn OcrEngine + Send, OcrModelSpec>;
type NnSlot = ModelSlot<dyn NnEngine + Send, NnModelSpec>;

impl<E: ?Sized, S> ModelSlot<E, S> {
    fn built(engine: Box<E>, identity: VisionModelIdentity) -> Self {
        let verified_sha256 = OnceLock::new();
        let _ = verified_sha256.set(identity.model_sha256().to_string());
        Self {
            model_ref: identity.model_ref().to_string(),
            spec: None,
            admitted_sha256: Some(identity.model_sha256().to_string()),
            verified_sha256,
            cell: Mutex::new(SlotCell {
                engine: Some(engine),
                retired: None,
                session_id: None,
                counted: false,
            }),
            last_used: AtomicU64::new(0),
            // Built before the provider: in memory, its load not measured here.
            stats: Mutex::new(ModelStats {
                loaded: Some(None),
                ..ModelStats::default()
            }),
        }
    }

    fn lazy(model_ref: String, spec: S) -> Self {
        Self {
            model_ref,
            spec: Some(spec),
            admitted_sha256: None,
            verified_sha256: OnceLock::new(),
            cell: Mutex::new(SlotCell {
                engine: None,
                retired: None,
                session_id: None,
                counted: false,
            }),
            last_used: AtomicU64::new(0),
            stats: Mutex::new(ModelStats::default()),
        }
    }

    /// #407 PERF-READ: an engine is in the slot now, its load's private-bytes growth `growth`.
    fn note_loaded(&self, growth: Option<u64>) {
        self.stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .loaded = Some(growth);
    }

    /// Workflow #407 GPU-1 C fix 2 (design §2.5): the loaded engine runs where `execution` says,
    /// read live until the engine leaves the slot.
    fn note_execution(&self, execution: OcrExecutionStatus) {
        self.stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .execution = Some(execution);
    }

    /// #407 PERF-READ: the slot's engine left memory (unloaded or retired); its execution status
    /// leaves with it (Workflow #407 GPU-1 C fix 2).
    fn note_unloaded(&self) {
        let mut stats = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        stats.loaded = None;
        stats.execution = None;
    }

    /// Workflow #407 GPU-1 C fix 2 (design §2.5): the warning of the fallback the loaded engine
    /// runs after, now (slice B2's `OcrModelExecution::warning`); none when it runs as requested
    /// or no engine with an execution status is in the slot. The execution status's own lock is
    /// taken after this account's is released.
    fn execution_warning(&self) -> Option<Outcome> {
        let execution = self
            .stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .execution
            .clone()?;
        execution.current().warning
    }

    /// Workflow #407 GPU-1 E: whether the loaded engine runs on a GPU now, as the execution status
    /// its load handed over says (a provider other than the CPU); false for an engine without
    /// one (an NN model, an engine built before the provider). The execution status's own lock
    /// is taken after this account's is released.
    fn runs_on_gpu(&self) -> bool {
        let execution = self
            .stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .execution
            .clone();
        execution.is_some_and(|execution| execution.current().actual != OnnxExecutionProvider::Cpu)
    }

    /// #407 PERF-READ: the slot's loaded engine as the status shows it, when one is in memory.
    fn loaded_status(&self, kind: VisionModelKind) -> Option<VisionModelMemoryStatus> {
        let growth = self
            .stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .loaded?;
        Some(VisionModelMemoryStatus {
            model_ref: self.model_ref.clone(),
            kind,
            load_private_bytes: growth,
        })
    }

    /// #407 PERF-READ: the slot's requests as the status shows them, once it had one.
    fn timing_status(&self, kind: VisionModelKind) -> Option<VisionModelTimingStatus> {
        let stats = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        (stats.requests > 0).then(|| VisionModelTimingStatus {
            model_ref: self.model_ref.clone(),
            kind,
            requests: stats.requests,
            inferences: stats.inferences,
            lock_wait_total_us: stats.lock_wait_total_us,
            lock_wait_longest_us: stats.lock_wait_longest_us,
            inference_total_us: stats.inference_total_us,
            inference_longest_us: stats.inference_longest_us,
        })
    }

    /// Refuses a request whose identity differs from the admitted or the verified one.
    fn require(&self, model_sha256: &str, capability: &str) -> Result<(), VisionProviderError> {
        for (source, known) in [
            ("admitted", self.admitted_sha256.as_deref()),
            ("installed", self.verified_sha256.get().map(String::as_str)),
        ] {
            if let Some(known) = known
                && known != model_sha256
            {
                return Err(VisionProviderError::new(
                    VisionProviderErrorCode::ModelMismatch,
                    format!(
                        "{capability} model '{}' {source} content is {known}; the target requires {model_sha256}",
                        self.model_ref
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Records the content identity of a fresh hash; false when it differs from the first one.
    fn verify(&self, model_sha256: &str) -> bool {
        self.verified_sha256
            .get_or_init(|| model_sha256.to_string())
            .as_str()
            == model_sha256
    }

    fn identity(&self) -> Result<VisionModelIdentity, VisionProviderError> {
        let model_sha256 = self.verified_sha256.get().ok_or_else(|| {
            VisionProviderError::new(
                VisionProviderErrorCode::Internal,
                format!("vision model '{}' has no verified identity", self.model_ref),
            )
        })?;
        VisionModelIdentity::new(self.model_ref.clone(), model_sha256.clone())
    }

    fn touch(&self) {
        self.last_used.store(
            NEXT_MODEL_USE.fetch_add(1, Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
}

/// Thread-safe Runtime adapter over the vision engines: one slot per model, each with its own
/// lock, built on first use through the injected loader and kept within
/// `MAX_LOADED_VISION_MODELS` by unloading the least recently used idle model.
pub struct VisionFfiProvider {
    ocr: BTreeMap<String, OcrSlot>,
    nn: BTreeMap<String, NnSlot>,
    /// Model folders that break the folder rule, with the reason; only requests naming them
    /// fail.
    invalid: BTreeMap<String, String>,
    loader: Option<Arc<dyn VisionModelLoader>>,
    loaded: Mutex<usize>,
    /// #407 PERF-READ: the clock a request's lock wait and inference are timed on.
    clock: VisionTimingClock,
    /// #407 PERF-READ: the process's private bytes, sampled around each load; none, no growth.
    private_bytes: Option<PrivateBytesSampler>,
    /// Workflow #407 GPU-1 C (design §1.4, §2.5): the warning of a start that found the
    /// configured GPU execution provider unusable, shown until the first OCR model load
    /// completes. A leaf lock, never held across a load or an inference.
    startup_warning: Mutex<Option<Outcome>>,
}

impl VisionFfiProvider {
    /// Workflow #407 GPU-1 C (design §1.4, §2.5): the start found the configured GPU execution
    /// provider unusable (`vision_gpu_unavailable`, as actingd's startup record wrote it). The
    /// status shows `warning` until the first OCR model load completes: that load initialises
    /// the runtime and decides again, and its decision is the one in effect. A load that fails
    /// leaves the warning shown.
    pub fn with_startup_warning(mut self, warning: Outcome) -> Self {
        self.startup_warning = Mutex::new(Some(warning));
        self
    }

    /// #407 PERF-READ: each model load is measured by the process's private bytes before and
    /// after it (`sampler`); the status shows the growth beside the loaded model.
    pub fn with_private_bytes_sampler(mut self, sampler: PrivateBytesSampler) -> Self {
        self.private_bytes = Some(sampler);
        self
    }

    /// The process's private bytes now, when a sampler is set and its sample succeeded.
    fn sample_private_bytes(&self) -> Option<u64> {
        self.private_bytes.as_ref().and_then(|sample| sample())
    }

    /// A provider over already-built engines, one per capability.
    pub fn new(
        ocr: Option<(Box<dyn OcrEngine + Send>, VisionModelIdentity)>,
        nn: Option<(Box<dyn NnEngine + Send>, VisionModelIdentity)>,
    ) -> Result<Self, VisionProviderError> {
        if ocr.is_none() && nn.is_none() {
            return Err(no_capability());
        }
        Ok(Self {
            ocr: ocr
                .into_iter()
                .map(|(engine, identity)| {
                    (
                        identity.model_ref().to_string(),
                        ModelSlot::built(engine, identity),
                    )
                })
                .collect(),
            nn: nn
                .into_iter()
                .map(|(engine, identity)| {
                    (
                        identity.model_ref().to_string(),
                        ModelSlot::built(engine, identity),
                    )
                })
                .collect(),
            invalid: BTreeMap::new(),
            loader: None,
            loaded: Mutex::new(0),
            clock: monotonic_clock(),
            private_bytes: None,
            startup_warning: Mutex::new(None),
        })
    }

    /// A provider over the model folders of one vision root, each built through `loader` on
    /// first use. Invalid folders are kept with their reason: only requests naming them fail.
    pub fn from_listing(
        listing: VisionModelListing,
        loader: Arc<dyn VisionModelLoader>,
    ) -> Result<Self, VisionProviderError> {
        if listing.ocr.is_empty() && listing.nn.is_empty() && listing.invalid.is_empty() {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                "the vision models folder holds no model folder",
            ));
        }
        Ok(Self {
            ocr: listing
                .ocr
                .into_iter()
                .map(|spec| {
                    (
                        spec.model_ref.clone(),
                        ModelSlot::lazy(spec.model_ref.clone(), spec),
                    )
                })
                .collect(),
            nn: listing
                .nn
                .into_iter()
                .map(|spec| {
                    (
                        spec.model_ref.clone(),
                        ModelSlot::lazy(spec.model_ref.clone(), spec),
                    )
                })
                .collect(),
            invalid: listing
                .invalid
                .into_iter()
                .map(|folder| (folder.name, folder.reason))
                .collect(),
            loader: Some(loader),
            loaded: Mutex::new(0),
            clock: monotonic_clock(),
            private_bytes: None,
            startup_warning: Mutex::new(None),
        })
    }

    fn ocr_slot(&self, model_ref: &str) -> Result<&OcrSlot, VisionProviderError> {
        if let Some(slot) = self.ocr.get(model_ref) {
            return Ok(slot);
        }
        if let Some(reason) = self.invalid.get(model_ref) {
            return Err(invalid_folder(model_ref, reason));
        }
        if self.nn.contains_key(model_ref) {
            return Err(wrong_kind(model_ref, "an NN classification", "OCR"));
        }
        if self.ocr.is_empty() && self.loader.is_none() {
            return Err(unavailable("OCR"));
        }
        Err(not_installed("OCR", model_ref, self.ocr.keys()))
    }

    fn nn_slot(&self, model_ref: &str) -> Result<&NnSlot, VisionProviderError> {
        if let Some(slot) = self.nn.get(model_ref) {
            return Ok(slot);
        }
        if let Some(reason) = self.invalid.get(model_ref) {
            return Err(invalid_folder(model_ref, reason));
        }
        if self.ocr.contains_key(model_ref) {
            return Err(wrong_kind(model_ref, "an OCR", "NN"));
        }
        if self.nn.is_empty() && self.loader.is_none() {
            return Err(unavailable("NN"));
        }
        Err(not_installed("NN", model_ref, self.nn.keys()))
    }

    /// Runs `call` on the model's engine, building it first when needed. Waiting for the
    /// model's lock, or inside the load for another request's runtime initialisation,
    /// consumes the request's `timeout_ms`; the request's own model load does not; the engine
    /// receives what remains.
    fn call_ocr<R>(
        &self,
        model_ref: &str,
        model_sha256: &str,
        timeout_ms: u64,
        call: impl FnOnce(&mut (dyn OcrEngine + Send), u64) -> VisionFfiResult<R>,
    ) -> Result<(R, VisionModelIdentity), VisionProviderError> {
        let slot = self.ocr_slot(model_ref)?;
        slot.require(model_sha256, "OCR")?;
        let started = Instant::now();
        let deadline = deadline_after(started, timeout_ms, "OCR")?;
        // #407 PERF-READ: declared before the lock's guard, so it records after the lock is free.
        let mut timing = RequestTiming::start(&slot.stats, &self.clock);
        let mut cell = lock_until(&slot.cell, deadline, "OCR", model_ref)?;
        timing.locked();
        let mut waited = started.elapsed();
        if let Some(reason) = &cell.retired {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                reason.clone(),
            ));
        }
        if cell.engine.is_none() {
            waited += self.load_ocr(slot, &mut cell, model_sha256, deadline)?;
        }
        let waited_ms = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
        slot.require(model_sha256, "OCR")?;
        let identity = slot.identity()?;
        let remaining = timeout_ms.saturating_sub(waited_ms);
        if remaining == 0 {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Timeout,
                "OCR request deadline exhausted before inference",
            ));
        }
        slot.touch();
        let outcome = {
            let engine = cell
                .engine
                .as_deref_mut()
                .ok_or_else(|| unavailable("OCR"))?;
            timing.inference(|| catch_unwind(AssertUnwindSafe(|| call(engine, remaining))))
        };
        match outcome {
            Ok(Ok(value)) => Ok((value, identity)),
            Ok(Err(error)) => {
                // Workflow #407 GPU-1 E (design §7 row E, §9 Q8): an engine on a GPU whose
                // provider became unavailable while it ran (the device was removed or reset) is
                // unloaded, not kept: the next request loads the model again, which resolves its
                // adapter again and falls back as its configuration says. Every other failure,
                // and every failure of an engine on the CPU, keeps the engine as before.
                if error.code() == VisionFfiErrorCode::ProviderUnavailable && slot.runs_on_gpu() {
                    self.unload(slot, &mut *cell);
                }
                Err(map_ffi_error(error))
            }
            Err(_) => {
                self.retire(
                    slot,
                    &mut *cell,
                    "OCR engine was retired after a provider panic",
                );
                Err(VisionProviderError::new(
                    VisionProviderErrorCode::Internal,
                    "OCR engine panicked and was retired",
                ))
            }
        }
    }

    fn load_ocr(
        &self,
        slot: &OcrSlot,
        cell: &mut SlotCell<dyn OcrEngine + Send>,
        model_sha256: &str,
        deadline: Instant,
    ) -> Result<Duration, VisionProviderError> {
        let (Some(spec), Some(loader)) = (slot.spec.as_ref(), self.loader.as_ref()) else {
            return Err(unavailable("OCR"));
        };
        let session_id = match &cell.session_id {
            Some(session_id) => session_id.clone(),
            None => {
                let session_id = next_ocr_session_id().map_err(map_ffi_error)?;
                cell.session_id = Some(session_id.clone());
                session_id
            }
        };
        self.reserve_loaded_model()?;
        let before = self.sample_private_bytes();
        match loader.load_ocr(spec, model_sha256, &session_id, deadline) {
            Ok(OcrModelLoad::Loaded {
                engine,
                model_sha256: actual,
                waited_on_others,
                execution,
            }) => {
                let after = self.sample_private_bytes();
                if !slot.verify(&actual) {
                    self.release_loaded_model();
                    return Err(self.changed_on_disk(slot, cell, &actual));
                }
                cell.engine = Some(engine);
                cell.counted = true;
                slot.note_loaded(load_growth(before, after));
                // Workflow #407 GPU-1 C fix 2 (design §2.5): the status reads where the model runs
                // from the execution status its load handed over (slice B2), live: the engine
                // replaces it when it moves the model to the CPU after the load.
                slot.note_execution(execution);
                // Workflow #407 GPU-1 C (design §1.4): a completed OCR load initialised the
                // runtime (or found it initialised) and decided again; the start's warning is
                // no longer the decision in effect.
                self.startup_warning
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                Ok(waited_on_others)
            }
            Ok(OcrModelLoad::Mismatch {
                model_sha256: actual,
            }) => {
                self.release_loaded_model();
                if !slot.verify(&actual) {
                    return Err(self.changed_on_disk(slot, cell, &actual));
                }
                Err(VisionProviderError::new(
                    VisionProviderErrorCode::ModelMismatch,
                    format!(
                        "OCR model '{}' content is {actual}; the target requires {model_sha256}",
                        slot.model_ref
                    ),
                ))
            }
            Err(error) => {
                self.release_loaded_model();
                Err(map_ffi_error(error))
            }
        }
    }

    fn load_nn(
        &self,
        slot: &NnSlot,
        cell: &mut SlotCell<dyn NnEngine + Send>,
        model_sha256: &str,
        deadline: Instant,
    ) -> Result<Duration, VisionProviderError> {
        let (Some(spec), Some(loader)) = (slot.spec.as_ref(), self.loader.as_ref()) else {
            return Err(unavailable("NN"));
        };
        self.reserve_loaded_model()?;
        let before = self.sample_private_bytes();
        match loader.load_nn(spec, model_sha256, deadline) {
            Ok(NnModelLoad::Loaded {
                engine,
                model_sha256: actual,
                waited_on_others,
            }) => {
                let after = self.sample_private_bytes();
                if !slot.verify(&actual) {
                    self.release_loaded_model();
                    return Err(self.changed_on_disk(slot, cell, &actual));
                }
                cell.engine = Some(engine);
                cell.counted = true;
                slot.note_loaded(load_growth(before, after));
                Ok(waited_on_others)
            }
            Ok(NnModelLoad::Mismatch {
                model_sha256: actual,
            }) => {
                self.release_loaded_model();
                if !slot.verify(&actual) {
                    return Err(self.changed_on_disk(slot, cell, &actual));
                }
                Err(VisionProviderError::new(
                    VisionProviderErrorCode::ModelMismatch,
                    format!(
                        "NN model '{}' content is {actual}; the target requires {model_sha256}",
                        slot.model_ref
                    ),
                ))
            }
            Err(error) => {
                self.release_loaded_model();
                Err(map_ffi_error(error))
            }
        }
    }

    fn changed_on_disk<E: ?Sized, S>(
        &self,
        slot: &ModelSlot<E, S>,
        cell: &mut SlotCell<E>,
        actual: &str,
    ) -> VisionProviderError {
        let reason = format!(
            "vision model '{}' files changed from {} to {actual} while the Runtime was running; restart is required",
            slot.model_ref,
            slot.verified_sha256
                .get()
                .map_or("an unverified identity", String::as_str)
        );
        self.retire(slot, cell, &reason);
        VisionProviderError::new(VisionProviderErrorCode::Unavailable, reason)
    }

    fn retire<E: ?Sized, S>(&self, slot: &ModelSlot<E, S>, cell: &mut SlotCell<E>, reason: &str) {
        self.unload(slot, cell);
        cell.retired = Some(reason.to_string());
    }

    /// The slot's engine leaves memory and its place among the loaded models is free; unlike
    /// [`Self::retire`], the slot stays usable and its next request loads the model again
    /// (Workflow #407 GPU-1 E).
    fn unload<E: ?Sized, S>(&self, slot: &ModelSlot<E, S>, cell: &mut SlotCell<E>) {
        cell.engine = None;
        slot.note_unloaded();
        if cell.counted {
            cell.counted = false;
            self.release_loaded_model();
        }
    }

    /// Takes one place among the loaded models, unloading the least recently used idle model
    /// when the bound is reached.
    fn reserve_loaded_model(&self) -> Result<(), VisionProviderError> {
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if *loaded < MAX_LOADED_VISION_MODELS {
            *loaded += 1;
            return Ok(());
        }
        if self.evict_idle_model() {
            // One model left and one takes its place: the count is unchanged.
            return Ok(());
        }
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            format!("all {MAX_LOADED_VISION_MODELS} loaded vision models are in use"),
        ))
    }

    fn release_loaded_model(&self) {
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        *loaded = loaded.saturating_sub(1);
    }

    /// Unloads the idle loaded model used least recently; a model whose lock is held is in use
    /// and never chosen.
    fn evict_idle_model(&self) -> bool {
        let mut victim: Option<(u64, bool, &str)> = None;
        for (name, slot) in &self.ocr {
            if let Some(used) = idle_since(slot)
                && victim.is_none_or(|(best, _, _)| used < best)
            {
                victim = Some((used, true, name));
            }
        }
        for (name, slot) in &self.nn {
            if let Some(used) = idle_since(slot)
                && victim.is_none_or(|(best, _, _)| used < best)
            {
                victim = Some((used, false, name));
            }
        }
        match victim {
            Some((_, true, name)) => self.ocr.get(name).is_some_and(unload_if_idle),
            Some((_, false, name)) => self.nn.get(name).is_some_and(unload_if_idle),
            None => false,
        }
    }
}

/// When the slot holds a loaded, idle engine: its last use.
fn idle_since<E: ?Sized, S>(slot: &ModelSlot<E, S>) -> Option<u64> {
    let cell = slot.cell.try_lock().ok()?;
    (cell.counted && cell.engine.is_some()).then(|| slot.last_used.load(Ordering::Relaxed))
}

fn unload_if_idle<E: ?Sized, S>(slot: &ModelSlot<E, S>) -> bool {
    match slot.cell.try_lock() {
        Ok(mut cell) if cell.counted && cell.engine.is_some() => {
            cell.engine = None;
            cell.counted = false;
            slot.note_unloaded();
            true
        }
        _ => false,
    }
}

/// #407 PERF-READ: the growth of the process's private bytes across a load, `after` less
/// `before` (never below zero: memory another thread released meanwhile hides growth, never
/// makes it negative); unknown when either sample failed.
fn load_growth(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    Some(after?.saturating_sub(before?))
}

impl fmt::Debug for VisionFfiProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VisionFfiProvider")
            .field("ocr_models", &self.ocr.keys().collect::<Vec<_>>())
            .field("nn_models", &self.nn.keys().collect::<Vec<_>>())
            .field("invalid_models", &self.invalid.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RecognitionVisionProvider for VisionFfiProvider {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        self.ocr_slot(model_ref)?.require(model_sha256, "OCR")
    }

    fn require_nn_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        self.nn_slot(model_ref)?.require(model_sha256, "NN")
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let frame = borrow_frame(request.frame)?;
        let region = vision_rect(request.region);
        let (result, _) = self.call_ocr(
            request.model_ref,
            request.model_sha256,
            request.timeout_ms,
            |engine, timeout_ms| {
                engine.read_text_view(OcrInferenceRequestView {
                    frame,
                    region,
                    languages: request.languages,
                    timeout_ms,
                    blocks: ocr_block_mode(request.blocks),
                })
            },
        )?;
        validate_ocr_backend(&result)
            .map_err(|error| error.with_ppocr_diagnostics(result.ppocr_diagnostics.clone()))?;
        Ok(map_ocr_result(result))
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let frame = borrow_frame(request.frame)?;
        let region = vision_rect(request.region);
        let (output, identity) = self.call_ocr(
            request.model_ref,
            request.model_sha256,
            request.timeout_ms,
            |engine, timeout_ms| {
                engine.read_text_with_attestation_view(OcrInferenceRequestView {
                    frame,
                    region,
                    languages: request.languages,
                    timeout_ms,
                    blocks: ocr_block_mode(request.blocks),
                })
            },
        )?;
        validate_ocr_backend(&output.result).map_err(|error| {
            error.with_ppocr_diagnostics(output.result.ppocr_diagnostics.clone())
        })?;
        let attestation = output
            .execution_attestation
            .ok_or_else(|| {
                VisionProviderError::new(
                    VisionProviderErrorCode::InvalidResponse,
                    "OCR engine did not return execution attestation",
                )
            })
            .map_err(|error| {
                error.with_ppocr_diagnostics(output.result.ppocr_diagnostics.clone())
            })?;
        let execution = map_ocr_execution_evidence(&attestation, &identity).map_err(|error| {
            error.with_ppocr_diagnostics(output.result.ppocr_diagnostics.clone())
        })?;
        Ok(OcrProviderObservation {
            result: map_ocr_result(output.result),
            execution: Some(execution),
        })
    }

    fn classify(
        &self,
        request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        let slot = self.nn_slot(request.model_ref)?;
        slot.require(request.model_sha256, "NN")?;
        let frame = crop_frame(request.frame, request.region)?;
        let started = Instant::now();
        let deadline = deadline_after(started, request.timeout_ms, "NN")?;
        // #407 PERF-READ: declared before the lock's guard, so it records after the lock is free.
        let mut timing = RequestTiming::start(&slot.stats, &self.clock);
        let mut cell = lock_until(&slot.cell, deadline, "NN", request.model_ref)?;
        timing.locked();
        let mut waited = started.elapsed();
        if let Some(reason) = &cell.retired {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                reason.clone(),
            ));
        }
        if cell.engine.is_none() {
            waited += self.load_nn(slot, &mut cell, request.model_sha256, deadline)?;
        }
        let waited_ms = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
        slot.require(request.model_sha256, "NN")?;
        let timeout_ms = request.timeout_ms.saturating_sub(waited_ms);
        if timeout_ms == 0 {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Timeout,
                "NN request deadline exhausted before inference",
            ));
        }
        let ffi_request = NnInferenceRequest {
            frame,
            model_id: request.model_ref.to_string(),
            labels: request.candidate_labels.to_vec(),
            timeout_ms,
        };
        slot.touch();
        let outcome = {
            let engine = cell
                .engine
                .as_deref_mut()
                .ok_or_else(|| unavailable("NN"))?;
            timing.inference(|| catch_unwind(AssertUnwindSafe(|| engine.classify(ffi_request))))
        };
        let result = match outcome {
            Ok(result) => result.map_err(map_ffi_error)?,
            Err(_) => {
                self.retire(
                    slot,
                    &mut *cell,
                    "NN engine was retired after a provider panic",
                );
                return Err(VisionProviderError::new(
                    VisionProviderErrorCode::Internal,
                    "NN engine panicked and was retired",
                ));
            }
        };
        validate_nn_backend(&result)?;
        Ok(NnProviderResult {
            labels: result
                .labels
                .into_iter()
                .map(|label| NnProviderLabel {
                    label: label.label,
                    score: label.score,
                })
                .collect(),
        })
    }

    /// #407 PERF-READ: the OCR models, then the NN models, whose engine is in memory now.
    fn loaded_models(&self) -> Vec<VisionModelMemoryStatus> {
        let ocr = self
            .ocr
            .values()
            .filter_map(|slot| slot.loaded_status(VisionModelKind::Ocr));
        let nn = self
            .nn
            .values()
            .filter_map(|slot| slot.loaded_status(VisionModelKind::Nn));
        ocr.chain(nn).collect()
    }

    /// #407 PERF-READ: the OCR models, then the NN models, that had a request since the start.
    fn model_timings(&self) -> Vec<VisionModelTimingStatus> {
        let ocr = self
            .ocr
            .values()
            .filter_map(|slot| slot.timing_status(VisionModelKind::Ocr));
        let nn = self
            .nn
            .values()
            .filter_map(|slot| slot.timing_status(VisionModelKind::Nn));
        ocr.chain(nn).collect()
    }

    /// Workflow #407 GPU-1 C (design §2.5): the start's GPU warning, until the first OCR model
    /// load completes, then (fix 2) each distinct warning of the loaded OCR models' execution, as
    /// their loads handed it over (`OcrModelLoad::Loaded.execution`, slice GPU-1 B2): one for a
    /// model on the CPU after its own fallback (attested `directml` requested, `cpu` resolved,
    /// which its evidence carries), one for the process's fallback at its runtime initialisation
    /// however many models carry it, none for a model on the provider it requested. Each
    /// model's execution is read live: a model the engine moves to the CPU after its load shows
    /// its warning from then on, and an unloaded model's warning leaves with its engine.
    fn execution_warnings(&self) -> Vec<Outcome> {
        let mut warnings: Vec<Outcome> = self
            .startup_warning
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect();
        for warning in self
            .ocr
            .values()
            .filter_map(|slot| slot.execution_warning())
        {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
        warnings
    }
}

fn vision_rect(region: PackRect) -> VisionRect {
    VisionRect {
        x: region.x,
        y: region.y,
        width: region.width,
        height: region.height,
    }
}

fn deadline_after(
    started: Instant,
    timeout_ms: u64,
    capability: &str,
) -> Result<Instant, VisionProviderError> {
    started
        .checked_add(Duration::from_millis(timeout_ms))
        .ok_or_else(|| {
            VisionProviderError::new(
                VisionProviderErrorCode::Timeout,
                format!("{capability} deadline overflow"),
            )
        })
}

/// Takes one model's lock, waiting at most until the request's deadline (Workflow #360: every
/// path, page gates included, waits under the target's own `timeout_ms`).
fn lock_until<'a, T>(
    mutex: &'a Mutex<T>,
    deadline: Instant,
    capability: &str,
    model_ref: &str,
) -> Result<MutexGuard<'a, T>, VisionProviderError> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(_)) => {
                return Err(VisionProviderError::new(
                    VisionProviderErrorCode::Internal,
                    format!("{capability} engine mutex is poisoned"),
                ));
            }
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(VisionProviderError::new(
                        VisionProviderErrorCode::Timeout,
                        format!(
                            "{capability} model '{model_ref}' was held by another request beyond this target's timeout_ms"
                        ),
                    ));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn invalid_folder(model_ref: &str, reason: &str) -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::Unavailable,
        format!("vision model folder '{model_ref}' is invalid: {reason}"),
    )
}

fn wrong_kind(model_ref: &str, actual: &str, requested: &str) -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::ModelMismatch,
        format!("vision model '{model_ref}' is {actual} model, not an {requested} model"),
    )
}

fn no_capability() -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::Unavailable,
        "vision provider must expose at least one production capability",
    )
}

fn not_installed<'a>(
    capability: &str,
    model_ref: &str,
    installed: impl Iterator<Item = &'a String>,
) -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::ModelMismatch,
        format!(
            "{capability} model '{model_ref}' is not installed (installed {capability} models: [{}])",
            installed.map(String::as_str).collect::<Vec<_>>().join(", ")
        ),
    )
}

/// The vision boundary's block mode of a recognition request (Workflow #396 REC-1).
fn ocr_block_mode(blocks: OcrProviderBlocks) -> OcrBlockMode {
    match blocks {
        OcrProviderBlocks::Implicit => OcrBlockMode::Implicit,
        OcrProviderBlocks::Region => OcrBlockMode::Region,
        OcrProviderBlocks::Detect {
            line_merge_gap_factor: None,
        } => OcrBlockMode::Detect {
            line_merge: OcrLineMerge::Never,
        },
        OcrProviderBlocks::Detect {
            line_merge_gap_factor: Some(factor),
        } => OcrBlockMode::Detect {
            line_merge: OcrLineMerge::GapFactor(factor),
        },
    }
}

fn map_ocr_result(result: OcrInferenceResult) -> OcrProviderResult {
    OcrProviderResult {
        detection: result.detection.map(|count| OcrProviderDetection {
            detected: count.detected,
            limit: count.limit,
        }),
        ppocr_diagnostics: result.ppocr_diagnostics,
        text: result.text,
        blocks: result
            .blocks
            .into_iter()
            .map(|block| OcrProviderTextBlock {
                text: block.text,
                rect: PackRect {
                    x: block.rect.x,
                    y: block.rect.y,
                    width: block.rect.width,
                    height: block.rect.height,
                },
                confidence: block.confidence,
            })
            .collect(),
        confidence: result.confidence,
    }
}

fn map_ocr_execution_evidence(
    attestation: &OcrExecutionAttestation,
    identity: &VisionModelIdentity,
) -> Result<OcrProviderExecutionEvidence, VisionProviderError> {
    let key = attestation.session.key();
    if key.model_ref() != identity.model_ref() || key.model_sha256() != identity.model_sha256() {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::ModelMismatch,
            "OCR execution attestation model identity does not match the admitted capability",
        ));
    }
    // Workflow #407 GPU-1 C (design §2.4): DirectML maps to its own kind; which bindings of it
    // the evidence may carry is the recognition pack's rule (and runtime-client's).
    let provider_kind = |provider| match provider {
        OnnxExecutionProvider::Cpu => OcrExecutionProviderKind::Cpu,
        OnnxExecutionProvider::Cuda => OcrExecutionProviderKind::Cuda,
        OnnxExecutionProvider::Directml => OcrExecutionProviderKind::Directml,
    };
    Ok(OcrProviderExecutionEvidence {
        invocation_id: attestation.invocation_id.as_str().to_string(),
        session_id: attestation.session.session_id().as_str().to_string(),
        session_generation: attestation.session.generation(),
        requested_provider: provider_kind(key.requested_backend()),
        resolved_provider: provider_kind(attestation.resolved_execution_provider),
        requested_cuda_ordinal: key.requested_cuda_device().map(|device| device.ordinal),
        requested_cuda_identity: key
            .requested_cuda_device()
            .map(|device| device.expected_stable_identity.clone()),
        resolved_cuda_ordinal: key.resolved_cuda_device().map(|device| device.ordinal),
        resolved_cuda_identity: key
            .resolved_cuda_device()
            .map(|device| device.stable_identity.clone()),
        provider_implementation: attestation.provider.implementation.clone(),
        provider_binary_sha256: attestation.provider.binary_sha256.clone(),
        runtime_version: attestation.runtime.onnxruntime_version.clone(),
        model_ref: key.model_ref().to_string(),
        model_sha256: key.model_sha256().to_string(),
        cpu_ep_registered: attestation.cpu_ep_registered,
        cpu_fallback_disabled: attestation.cpu_fallback_disabled,
        fallback_forbidden: attestation.fallback_policy == OcrFallbackPolicy::Forbidden,
        fallback_observed: attestation.fallback_observed,
        complete: attestation.complete,
    })
}

fn borrow_frame(
    frame: VisionProviderFrame<'_>,
) -> Result<VisionFrameView<'_>, VisionProviderError> {
    VisionFrameView::new(
        frame.width,
        frame.height,
        VisionPixelFormat::Rgb8,
        frame.rgb8_pixels,
    )
    .map_err(map_ffi_error)
}

fn crop_frame(
    frame: VisionProviderFrame<'_>,
    region: PackRect,
) -> Result<VisionFrame, VisionProviderError> {
    let x = usize::try_from(region.x).map_err(|_| invalid_region())?;
    let y = usize::try_from(region.y).map_err(|_| invalid_region())?;
    let width = usize::try_from(region.width).map_err(|_| invalid_region())?;
    let height = usize::try_from(region.height).map_err(|_| invalid_region())?;
    let frame_width = usize::try_from(frame.width).map_err(|_| invalid_region())?;
    let frame_height = usize::try_from(frame.height).map_err(|_| invalid_region())?;
    let end_x = x.checked_add(width).ok_or_else(invalid_region)?;
    let end_y = y.checked_add(height).ok_or_else(invalid_region)?;
    if width == 0 || height == 0 || end_x > frame_width || end_y > frame_height {
        return Err(invalid_region());
    }
    let row_bytes = width.checked_mul(3).ok_or_else(invalid_region)?;
    let capacity = row_bytes.checked_mul(height).ok_or_else(invalid_region)?;
    let frame_row_bytes = frame_width.checked_mul(3).ok_or_else(invalid_region)?;
    let mut pixels = Vec::with_capacity(capacity);
    for row in y..end_y {
        let start = row
            .checked_mul(frame_row_bytes)
            .and_then(|offset| x.checked_mul(3).and_then(|x| offset.checked_add(x)))
            .ok_or_else(invalid_region)?;
        let end = start.checked_add(row_bytes).ok_or_else(invalid_region)?;
        let source = frame
            .rgb8_pixels
            .get(start..end)
            .ok_or_else(invalid_region)?;
        pixels.extend_from_slice(source);
    }
    VisionFrame::new(
        u32::try_from(width).map_err(|_| invalid_region())?,
        u32::try_from(height).map_err(|_| invalid_region())?,
        VisionPixelFormat::Rgb8,
        pixels,
    )
    .map_err(map_ffi_error)
}

fn validate_ocr_backend(result: &OcrInferenceResult) -> Result<(), VisionProviderError> {
    if result.backend != VisionBackendKind::FastDeployPpocr {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::InvalidResponse,
            "production OCR result did not attest the fastdeploy_ppocr backend",
        ));
    }
    if !result.warnings.is_empty() {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::InvalidResponse,
            "production OCR result reported unhandled provider warnings",
        ));
    }
    Ok(())
}

fn validate_nn_backend(result: &NnClassificationResult) -> Result<(), VisionProviderError> {
    if result.backend != VisionBackendKind::OnnxRuntime {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::InvalidResponse,
            "production NN result did not attest the onnxruntime backend",
        ));
    }
    Ok(())
}

fn map_ffi_error(error: VisionFfiError) -> VisionProviderError {
    let code = match error.code() {
        VisionFfiErrorCode::ProviderUnavailable => VisionProviderErrorCode::Unavailable,
        VisionFfiErrorCode::Timeout => VisionProviderErrorCode::Timeout,
        VisionFfiErrorCode::ModelMismatch => VisionProviderErrorCode::ModelMismatch,
        VisionFfiErrorCode::InvalidRequest | VisionFfiErrorCode::InvalidResponse => {
            VisionProviderErrorCode::InvalidResponse
        }
        VisionFfiErrorCode::ProviderFailure
        | VisionFfiErrorCode::ProviderPanic
        | VisionFfiErrorCode::Internal => VisionProviderErrorCode::Internal,
    };
    VisionProviderError::new(code, error.to_string())
        .with_ppocr_diagnostics(error.ppocr_diagnostics().clone())
}

fn unavailable(capability: &str) -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::Unavailable,
        format!("{capability} production capability is unavailable"),
    )
}

fn invalid_region() -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::InvalidResponse,
        "vision ROI is outside the RGB8 frame",
    )
}

/// Facts of a MuMu instance the endpoint was discovered from (`MuMuManager info -v all`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredInstanceBinding {
    instance_index: u16,
    instance_name: String,
    provider_version: String,
    mumu_manager_path: PathBuf,
}

impl DiscoveredInstanceBinding {
    pub fn new(
        instance_index: u16,
        instance_name: impl Into<String>,
        provider_version: impl Into<String>,
        mumu_manager_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            instance_index,
            instance_name: instance_name.into(),
            provider_version: provider_version.into(),
            mumu_manager_path: mumu_manager_path.into(),
        }
    }

    pub const fn instance_index(&self) -> u16 {
        self.instance_index
    }

    pub fn instance_name(&self) -> &str {
        &self.instance_name
    }

    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }

    /// The `MuMuManager.exe` the instance was discovered through; the emulator control
    /// operation dispatches `control` against exactly this executable.
    pub fn mumu_manager_path(&self) -> &Path {
        &self.mumu_manager_path
    }
}

/// Workflow #398 RD-1b (model-398 v1.1 §3.2, §3.4): the binding of a handset instance as the
/// private binding file gave it: the device's serial (shown only as its label), when it was
/// bound and by which channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandsetBinding {
    serial: actingcommand_device::DeviceSerial,
    bound_at_unix_ms: u64,
    bound_via: actingcommand_contract::DeviceBoundVia,
}

impl HandsetBinding {
    pub fn new(
        serial: actingcommand_device::DeviceSerial,
        bound_at_unix_ms: u64,
        bound_via: actingcommand_contract::DeviceBoundVia,
    ) -> Self {
        Self {
            serial,
            bound_at_unix_ms,
            bound_via,
        }
    }

    pub const fn serial(&self) -> &actingcommand_device::DeviceSerial {
        &self.serial
    }

    pub const fn bound_at_unix_ms(&self) -> u64 {
        self.bound_at_unix_ms
    }

    pub const fn bound_via(&self) -> actingcommand_contract::DeviceBoundVia {
        self.bound_via
    }
}

/// Workflow #398 RD-1b (model-398 v1.1 §3.1, §3.6): the device of a handset instance, bound or
/// not yet bound. An unbound handset is registered and refuses every session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandsetEndpoint {
    Bound(HandsetBinding),
    Unbound,
}

impl HandsetEndpoint {
    /// The binding, `None` while the handset is unbound.
    pub const fn binding(&self) -> Option<&HandsetBinding> {
        match self {
            Self::Bound(binding) => Some(binding),
            Self::Unbound => None,
        }
    }
}

/// The structured ADB target a registered instance was configured with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAdbEndpoint {
    host: String,
    port: u16,
    serial_configured: bool,
    discovered: Option<DiscoveredInstanceBinding>,
    handset: Option<HandsetEndpoint>,
}

impl ResolvedAdbEndpoint {
    pub fn new(host: impl Into<String>, port: u16, serial_configured: bool) -> Self {
        Self {
            host: host.into(),
            port,
            serial_configured,
            discovered: None,
            handset: None,
        }
    }

    /// Workflow #398 RD-1b: marks the endpoint as a handset's, addressed by its serial.
    pub fn with_handset(mut self, handset: HandsetEndpoint) -> Self {
        self.handset = Some(handset);
        self
    }

    /// Workflow #398 RD-1b: the handset behind the endpoint; `None` for an emulator.
    pub const fn handset(&self) -> Option<&HandsetEndpoint> {
        self.handset.as_ref()
    }

    pub fn with_discovered_binding(mut self, discovered: DiscoveredInstanceBinding) -> Self {
        self.discovered = Some(discovered);
        self
    }

    /// Present only when the endpoint was bound through instance discovery.
    pub const fn discovered_binding(&self) -> Option<&DiscoveredInstanceBinding> {
        self.discovered.as_ref()
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    /// True when the registration carried an explicit serial instead of HOST:PORT.
    pub const fn serial_configured(&self) -> bool {
        self.serial_configured
    }
}

/// A discovery binding whose ADB port is not known yet: discovery reported the instance
/// stopped. Emulator control resolves the port when it starts the instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAdbEndpoint {
    host: String,
    discovered: DiscoveredInstanceBinding,
}

impl PendingAdbEndpoint {
    pub fn new(host: impl Into<String>, discovered: DiscoveredInstanceBinding) -> Self {
        Self {
            host: host.into(),
            discovered,
        }
    }

    /// The host the instance is bound with once its port is reported.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn discovered_binding(&self) -> &DiscoveredInstanceBinding {
        &self.discovered
    }
}

/// The ADB endpoint state of a registered instance: bound to a HOST:PORT target, or pending
/// until emulator control starts the discovered instance and reports its port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedInstanceEndpoint {
    Bound(ResolvedAdbEndpoint),
    Pending(PendingAdbEndpoint),
}

impl ResolvedInstanceEndpoint {
    pub const fn bound(&self) -> Option<&ResolvedAdbEndpoint> {
        match self {
            Self::Bound(endpoint) => Some(endpoint),
            Self::Pending(_) => None,
        }
    }

    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }

    /// Present for both states of a discovery-bound instance, absent for an explicit one.
    pub const fn discovered_binding(&self) -> Option<&DiscoveredInstanceBinding> {
        match self {
            Self::Bound(endpoint) => endpoint.discovered_binding(),
            Self::Pending(pending) => Some(pending.discovered_binding()),
        }
    }
}

#[derive(Clone)]
pub struct ResolvedExecutionInstance {
    instance_id: InstanceId,
    audit_endpoint: String,
    provenance: ExecutionBackendProvenance,
    adb_endpoint: Option<ResolvedInstanceEndpoint>,
    configuration: Option<actingcommand_contract::EffectiveDeviceConfiguration>,
    capabilities: Option<actingcommand_contract::EmulatorCapabilityProfile>,
}

impl PartialEq for ResolvedExecutionInstance {
    fn eq(&self, other: &Self) -> bool {
        self.instance_id == other.instance_id
            && self.audit_endpoint == other.audit_endpoint
            && self.provenance == other.provenance
    }
}

impl Eq for ResolvedExecutionInstance {}

impl ResolvedExecutionInstance {
    pub fn new(instance_id: InstanceId, audit_endpoint: impl Into<String>) -> Self {
        Self {
            instance_id,
            audit_endpoint: audit_endpoint.into(),
            provenance: ExecutionBackendProvenance::PhysicalDevice,
            adb_endpoint: None,
            configuration: None,
            capabilities: None,
        }
    }

    pub fn fixture_simulation(instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            audit_endpoint: "fixture-simulation".to_owned(),
            provenance: ExecutionBackendProvenance::FixtureSimulation,
            adb_endpoint: None,
            configuration: None,
            capabilities: None,
        }
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub fn audit_endpoint(&self) -> &str {
        &self.audit_endpoint
    }

    pub const fn provenance(&self) -> ExecutionBackendProvenance {
        self.provenance
    }

    pub fn with_adb_endpoint(mut self, adb_endpoint: ResolvedAdbEndpoint) -> Self {
        self.adb_endpoint = Some(ResolvedInstanceEndpoint::Bound(adb_endpoint));
        self
    }

    pub fn with_pending_endpoint(mut self, pending: PendingAdbEndpoint) -> Self {
        self.adb_endpoint = Some(ResolvedInstanceEndpoint::Pending(pending));
        self
    }

    pub const fn adb_endpoint(&self) -> Option<&ResolvedInstanceEndpoint> {
        self.adb_endpoint.as_ref()
    }

    pub fn with_configuration(
        mut self,
        configuration: actingcommand_contract::EffectiveDeviceConfiguration,
    ) -> Self {
        self.configuration = Some(configuration);
        self
    }

    pub fn configuration(&self) -> Option<&actingcommand_contract::EffectiveDeviceConfiguration> {
        self.configuration.as_ref()
    }

    pub fn with_capabilities(
        mut self,
        capabilities: actingcommand_contract::EmulatorCapabilityProfile,
    ) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    pub fn capabilities(&self) -> Option<&actingcommand_contract::EmulatorCapabilityProfile> {
        self.capabilities.as_ref()
    }
}

impl fmt::Debug for ResolvedExecutionInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedExecutionInstance")
            .field("instance_id", &self.instance_id)
            .field("audit_endpoint", &"<redacted>")
            .field("provenance", &self.provenance)
            .finish()
    }
}

/// Daemon-only factory boundary. Implementations open backends inside execution worker threads.
///
/// Backend implementations have no outcome-fact ingress. Scheduling outcomes must be committed by
/// Runtime from the terminal ledger fact instead of being supplied by a backend:
///
/// ```compile_fail
/// use actingcommand_execution_kernel::ExecutionBackendProvider;
///
/// fn inject_outcome(provider: &dyn ExecutionBackendProvider) {
///     let _ = provider.outcomes();
/// }
/// ```
/// One read-only foreground observation of a running instance (slice #316-B3): what Android
/// reported as the resumed package, next to the application the instance is assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundApplicationObservation {
    /// The resumed package, or `None` when the device reported no resumed activity.
    pub foreground: Option<String>,
    /// The instance's configured `application_id`.
    pub assigned: String,
}

/// One on-demand instance discovery answer of a provider: the provider version and every
/// instance it reported. Install roots, tool paths and the resolution source are not part of
/// it; they stay in the ledger and in native failure detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInstanceDiscovery {
    pub provider_version: String,
    pub instances: Vec<ProviderDiscoveredInstance>,
}

/// One instance an on-demand discovery reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveredInstance {
    pub instance_index: u16,
    pub instance_name: String,
    /// Present while `running`; otherwise only when reported.
    pub adb_host: Option<String>,
    /// Present while `running`; otherwise only when reported.
    pub adb_port: Option<u16>,
    pub running: bool,
    pub android_version: Option<String>,
}

/// Workflow #398 RD-2 (model-398 v1.1 §4.1): one event of the device-link watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceLinkWatchEvent {
    /// The adb server's device list changed: the link state of every bound handset the watch
    /// covers, by its instance alias (a handset the list does not name is `absent`).
    Links(BTreeMap<String, actingcommand_contract::DeviceLinkState>),
    /// The watch's process ended, or its output closed.
    Ended,
    /// The watch's output has a shape the parser does not know.
    ProtocolInvalid,
}

/// Workflow #398 RD-2 (model-398 v1.1 §4.1): the device-link watch of a provider's bound
/// handsets, one `adb track-devices` process. It is event driven: nothing polls the devices.
pub trait DeviceLinkWatch: Send {
    /// Blocks until the next event. After `Ended` or `ProtocolInvalid` the watch is over.
    fn next_event(&mut self) -> DeviceLinkWatchEvent;

    /// What ends the watch from another thread: a blocked `next_event` then returns `Ended`.
    fn stopper(&self) -> Arc<dyn DeviceLinkWatchStop>;
}

/// Ends a device-link watch from another thread.
pub trait DeviceLinkWatchStop: Send + Sync {
    /// Ends the watch's process; later calls do nothing. An error is a stop that could not be
    /// confirmed.
    fn stop(&self) -> DeviceResult<()>;
}

/// Typed refusal of an on-demand instance discovery. `code` is one of the startup discovery
/// codes (`instance_discovery_unavailable`, `mumu_manager_version_unsupported`); `error`
/// keeps the native device detail.
#[derive(Debug, Clone)]
pub struct InstanceDiscoveryFailure {
    pub code: &'static str,
    pub error: DeviceError,
}

pub trait ExecutionBackendProvider: Send + Sync + 'static {
    fn instance_aliases(&self) -> Vec<String>;

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance>;

    fn open_input(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn InputBackend>>>;

    fn open_capture(
        &self,
        instance_alias: &str,
        _memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn CaptureBackend>>>;

    /// `_memory` is the opening Capture command's frame budget (`None` for an Input command);
    /// a paired open with a budget primes its first frame (Workflow #317 sc2).
    fn open_nemu_session(
        &self,
        _instance_alias: &str,
        _memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<
        Option<actingcommand_device::OpenedBackend<actingcommand_device::NemuSessionBackends>>,
    > {
        Ok(None)
    }

    /// An application-lifecycle device write, admitted by the step witness the kernel
    /// carries from the Host; the provider passes it on to the device write it performs.
    fn control_application(
        &self,
        witness: &FencedWrite,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()>;

    /// One ADB baseline probe of a bound instance (slice #316-B3): `ensure_device` with a
    /// connect attempt allowed, nothing else. `Ok` means adbd answered `device`; an error is
    /// the ADB failure. Opens no device session. Providers without an ADB baseline have
    /// nothing to wait for and keep this `Ok`.
    fn probe_adb_baseline(&self, _instance_alias: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn probe_adb_baseline_until(
        &self,
        instance_alias: &str,
        deadline: std::time::Instant,
        stopped: &dyn Fn() -> bool,
    ) -> DeviceResult<()> {
        if stopped() || std::time::Instant::now() >= deadline {
            return Err(DeviceError::fatal(
                "ADB baseline stopped or deadline expired",
            ));
        }
        let result = self.probe_adb_baseline(instance_alias);
        if stopped() || std::time::Instant::now() >= deadline {
            return match result {
                Err(error) => Err(error),
                Ok(()) => Err(DeviceError::fatal(
                    "ADB baseline stopped or deadline expired",
                )),
            };
        }
        result
    }

    /// Read-only: the package the instance reports in the foreground, next to the application
    /// assigned to the instance (slice #316-B3). Goes through the ADB baseline only and opens
    /// no device session; a failure is an ADB failure. Providers without an ADB baseline keep
    /// this typed refusal.
    fn observe_foreground_application(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<ForegroundApplicationObservation> {
        Err(
            DeviceError::fatal("foreground application observation unsupported by this provider")
                .with_diagnostic(
                    DeviceErrorCategory::Protocol,
                    ContractLocation::ApplicationForegroundUnsupported,
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    ContractLocation::ObserveForegroundApplication,
                    DeviceErrorSensitivity::Sensitive,
                ),
        )
    }

    /// Starts, stops or restarts the emulator instance itself through its provider. Opens no
    /// device session. Providers without an instance-control surface keep this typed refusal.
    fn control_instance(
        &self,
        _instance_alias: &str,
        _action: EmulatorInstanceAction,
    ) -> EmulatorControlResult<EmulatorControlOutcome> {
        Err(EmulatorControlFailure::without_output(
            DeviceError::fatal("emulator control unsupported by this provider")
                .with_diagnostic(
                    DeviceErrorCategory::Protocol,
                    ExecutionKernelLocation::EmulatorControlSupport,
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    ContractLocation::ControlInstance,
                    DeviceErrorSensitivity::Sensitive,
                ),
            0,
        ))
    }

    /// Read-only: one bounded `info` read of the emulator instance itself (Workflow #381 B,
    /// review M3), taken after a failed control action so the host applies the instance's
    /// current state. Opens no device session. Providers without an instance-control surface
    /// keep this typed refusal.
    fn read_instance_state(&self, _instance_alias: &str) -> DeviceResult<InstanceState> {
        Err(
            DeviceError::fatal("emulator state read unsupported by this provider")
                .with_diagnostic(
                    DeviceErrorCategory::Protocol,
                    ExecutionKernelLocation::EmulatorControlSupport,
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    ContractLocation::ReadInstanceState,
                    DeviceErrorSensitivity::Sensitive,
                ),
        )
    }

    /// Completes or reverts the discovery binding of an instance after emulator control:
    /// `Some(port)` binds the discovered host with the port the started instance reported
    /// (`start` / `restart`), `None` returns the entry to pending (`stop`). Opens no device
    /// session; the caller holds the instance's admission guard and has closed its session.
    /// Providers without an instance-control surface keep this typed refusal.
    fn rebind_discovered_endpoint(
        &self,
        _instance_alias: &str,
        _adb_port: Option<u16>,
    ) -> DeviceResult<()> {
        Err(
            DeviceError::fatal("emulator control unsupported by this provider")
                .with_diagnostic(
                    DeviceErrorCategory::Protocol,
                    ExecutionKernelLocation::EmulatorControlSupport,
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    ContractLocation::RebindDiscoveredEndpoint,
                    DeviceErrorSensitivity::Sensitive,
                ),
        )
    }

    /// #402 HS-2 (design HS-M §3.2 step 3.3; register §1.270): replaces the instance's touch and
    /// capture backends in its registration with `touch` and `capture`, under the registration's
    /// own checks (explicit choices, one target, Nemu touch only beside Nemu capture), and
    /// recomputes its capability table from them. Opens nothing; the caller holds the instance's
    /// admission guard and has closed its session. Returns the Nemu application index the
    /// instance opens Nemu touch with, when `touch` is Nemu IPC and the registration has one.
    /// Providers without switchable backends keep this refusal.
    fn switch_instance_backends(
        &self,
        _instance_alias: &str,
        _touch: actingcommand_device::TouchBackendChoice,
        _capture: actingcommand_device::CaptureBackendChoice,
    ) -> DeviceResult<Option<u32>> {
        Err(DeviceError::fatal(
            "backend switch unsupported by this provider",
        ))
    }

    /// #402 HS-3 (design HS-M §4.1; register §1.271): the Nemu application index the provider
    /// resolved for the instance when it last opened its paired session, the configuration naming
    /// none, while that resolution holds (#402 HS-5b: only a rebinding drops it). `None` when the
    /// configuration names the index (which
    /// [`Self::switch_instance_backends`] returns), when the touch backend is not Nemu IPC, or
    /// when nothing is resolved. Reads memory only. Providers without Nemu keep this default.
    fn resolved_nemu_app_index(&self, _instance_alias: &str) -> Option<u32> {
        None
    }

    /// Workflow #437 MAPP-2a: the application (package name) the provider's registration assigns
    /// the instance, the `assigned` of [`Self::observe_foreground_application`]. Reads memory
    /// only: no device is queried. Providers without an assigned application keep this `None`.
    fn assigned_application_id(&self, _instance_alias: &str) -> Option<String> {
        None
    }

    /// Re-runs the provider's instance discovery on demand and reports what it finds. Binds,
    /// rebinds and registers nothing and opens no device session. Providers without an
    /// instance-discovery surface keep this typed refusal.
    fn discover_instances(
        &self,
    ) -> Result<ProviderInstanceDiscovery, Box<InstanceDiscoveryFailure>> {
        Err(Box::new(InstanceDiscoveryFailure {
            code: SharedCode::InstanceDiscoveryUnavailable.as_str(),
            error: DeviceError::fatal("instance discovery unsupported by this provider")
                .with_diagnostic(
                    DeviceErrorCategory::Protocol,
                    ContractLocation::InstanceDiscoveryUnsupported,
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    ContractLocation::DiscoverInstances,
                    DeviceErrorSensitivity::Sensitive,
                ),
        }))
    }

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        None
    }

    /// Workflow #398 RD-2 (model-398 v1.1 §4.1): opens the device-link watch of the provider's
    /// bound handsets, one `adb track-devices` process. `None` when the provider registers no
    /// bound handset: then no watch process runs (§12 RD-2-T9). Providers without handsets keep
    /// this default.
    fn open_device_link_watch(&self) -> DeviceResult<Option<Box<dyn DeviceLinkWatch>>> {
        Ok(None)
    }

    /// Workflow #398 RD-2 (model-398 v1.1 §4.3, `device_link.offline_action = kick_once`): one
    /// `adb -s <serial> reconnect` of the bound handset `instance_alias`: it kicks that one
    /// transport and touches no other device. Providers without handsets keep this refusal.
    fn reconnect_handset(&self, _instance_alias: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "handset reconnect unsupported by this provider",
        ))
    }

    /// Workflow #398 RD-5b (model-398 v1.1 §7.1-§7.3): hands `work` the device-state shell of
    /// the bound handset `instance_alias` (the closed sets of read-only reads and screen-check
    /// actions of Workflow #398 RD-5a, `adb -s <serial> shell ...` through the installation's
    /// adb) and the instance's configured application id, outside any session. `Ok(false)` when
    /// the instance is not a bound handset: `work` does not run and no command is sent (§7.2:
    /// an emulator gets none). Providers without handsets keep this default.
    fn with_handset_state(
        &self,
        _instance_alias: &str,
        _work: &mut dyn FnMut(&mut dyn actingcommand_device::DeviceStateShell, &str),
    ) -> DeviceResult<bool> {
        Ok(false)
    }

    fn observe_monitor(
        &self,
        _instance_alias: &str,
        _expected_page: &str,
        _frame: &Frame,
    ) -> ExecutionKernelResult<MonitorObservation> {
        Err(ExecutionKernelError::fatal(
            ExecutionKernelCode::MonitorObservationUnavailable.code(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_vision_ffi::{
        GpuFallback, GpuUnavailableReason, NnLabel, OcrInferenceOutput, OcrInferenceRequest,
        OcrModelExecution, OcrModelLayout, OcrTextBlock, VisionGpuCode, VisionModelDescription,
        VisionModelFamily,
    };
    use serde_json::json;

    #[test]
    fn vision_model_identity_rejects_host_paths_and_noncanonical_hashes() {
        for (model_ref, hash) in [
            ("C:\\models\\model.onnx", "a".repeat(64)),
            ("logical-model", "A".repeat(64)),
            ("logical-model", "a".repeat(63)),
        ] {
            let error =
                VisionModelIdentity::new(model_ref, hash).expect_err("invalid model identity");
            assert_eq!(error.code(), VisionProviderErrorCode::ModelMismatch);
        }
    }

    #[test]
    fn nn_adapter_crops_roi_before_calling_existing_ffi_engine() {
        let observed = Arc::new(Mutex::new(None));
        let provider = VisionFfiProvider::new(
            None,
            Some((
                Box::new(RecordingNnEngine {
                    observed: Arc::clone(&observed),
                    backend: VisionBackendKind::OnnxRuntime,
                }),
                identity("fixture-model", 'b'),
            )),
        )
        .expect("provider");
        let pixels = [
            1, 2, 3, 4, 5, 6, //
            7, 8, 9, 10, 11, 12,
        ];
        let labels = vec!["home".to_string()];
        let model_sha256 = "b".repeat(64);

        let result = RecognitionVisionProvider::classify(
            &provider,
            NnProviderRequest {
                frame: VisionProviderFrame {
                    width: 2,
                    height: 2,
                    rgb8_pixels: &pixels,
                },
                region: PackRect {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 2,
                },
                model_ref: "fixture-model",
                model_sha256: &model_sha256,
                candidate_labels: &labels,
                timeout_ms: 1_000,
            },
        )
        .expect("classification");

        assert_eq!(result.labels[0].label, "home");
        let request = observed
            .lock()
            .expect("observed request lock")
            .clone()
            .expect("request observed");
        assert_eq!((request.frame.width, request.frame.height), (1, 2));
        assert_eq!(request.frame.pixels, vec![4, 5, 6, 10, 11, 12]);
    }

    #[test]
    fn adapter_accepts_canonical_non_full_frame_ocr_roi() {
        let provider = VisionFfiProvider::new(
            Some((
                Box::new(StaticOcrEngine {
                    backend: VisionBackendKind::FastDeployPpocr,
                    warnings: Vec::new(),
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let pixels = [0; 12];
        let languages = vec!["en".to_string()];
        let model_sha256 = "a".repeat(64);
        let region = PackRect {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        };

        let result = RecognitionVisionProvider::read_text(
            &provider,
            OcrProviderRequest {
                frame: VisionProviderFrame {
                    width: 2,
                    height: 2,
                    rgb8_pixels: &pixels,
                },
                region,
                languages: &languages,
                timeout_ms: 1_000,
                model_ref: "PP-OCRv6_medium",
                model_sha256: &model_sha256,
                blocks: OcrProviderBlocks::Implicit,
            },
        )
        .expect("canonical ROI accepted");

        assert_eq!(result.text, "home");
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].rect, region);
        assert_eq!(result.blocks[0].confidence, Some(0.99));
    }

    #[test]
    fn adapter_maps_cpu_and_cuda_execution_attestation_and_rejects_model_mismatch() {
        for (provider_name, cuda_ordinal) in [("cpu", None), ("cuda", Some(3))] {
            let provider = VisionFfiProvider::new(
                Some((
                    Box::new(AttestedOcrEngine {
                        attestation: execution_attestation(provider_name, cuda_ordinal, 'a'),
                    }),
                    identity("PP-OCRv6_medium", 'a'),
                )),
                None,
            )
            .expect("provider");
            let result = RecognitionVisionProvider::read_text_with_execution_evidence(
                &provider,
                ocr_request(),
            )
            .expect("attested OCR");
            let evidence = result.execution.expect("execution evidence");
            assert_eq!(evidence.model_ref, "PP-OCRv6_medium");
            assert_eq!(evidence.model_sha256, "a".repeat(64));
            assert_eq!(evidence.requested_provider, evidence.resolved_provider);
            assert_eq!(evidence.requested_cuda_ordinal, cuda_ordinal);
            assert_eq!(evidence.resolved_cuda_ordinal, cuda_ordinal);
            assert_eq!(evidence.cpu_ep_registered, provider_name == "cpu");
            assert_eq!(evidence.cpu_fallback_disabled, provider_name == "cuda");
            assert!(evidence.fallback_forbidden);
            assert_eq!(evidence.fallback_observed, None);
        }

        let provider = VisionFfiProvider::new(
            Some((
                Box::new(AttestedOcrEngine {
                    attestation: execution_attestation("cpu", None, 'b'),
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let error =
            RecognitionVisionProvider::read_text_with_execution_evidence(&provider, ocr_request())
                .expect_err("attested model mismatch must fail closed");
        assert_eq!(error.code(), VisionProviderErrorCode::ModelMismatch);
    }

    #[test]
    fn adapter_rejects_any_ocr_provider_warning() {
        let provider = VisionFfiProvider::new(
            Some((
                Box::new(StaticOcrEngine {
                    backend: VisionBackendKind::FastDeployPpocr,
                    warnings: vec!["unexpected provider degradation".to_string()],
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let pixels = [0; 12];
        let languages = vec!["en".to_string()];
        let model_sha256 = "a".repeat(64);

        let error = RecognitionVisionProvider::read_text(
            &provider,
            OcrProviderRequest {
                frame: VisionProviderFrame {
                    width: 2,
                    height: 2,
                    rgb8_pixels: &pixels,
                },
                region: PackRect {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 2,
                },
                languages: &languages,
                timeout_ms: 1_000,
                model_ref: "PP-OCRv6_medium",
                model_sha256: &model_sha256,
                blocks: OcrProviderBlocks::Implicit,
            },
        )
        .expect_err("provider warning remains fail-closed");

        assert_eq!(error.code(), VisionProviderErrorCode::InvalidResponse);
    }

    #[test]
    fn adapter_rejects_test_double_backend_and_retires_panicking_engine() {
        let pixels = [0, 0, 0];
        let languages = vec!["en".to_string()];
        let model_sha256 = "a".repeat(64);
        let request = || OcrProviderRequest {
            frame: VisionProviderFrame {
                width: 1,
                height: 1,
                rgb8_pixels: &pixels,
            },
            region: PackRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            languages: &languages,
            timeout_ms: 1_000,
            model_ref: "PP-OCRv6_medium",
            model_sha256: &model_sha256,
            blocks: OcrProviderBlocks::Implicit,
        };
        let test_double = VisionFfiProvider::new(
            Some((
                Box::new(StaticOcrEngine {
                    backend: VisionBackendKind::TestDouble,
                    warnings: Vec::new(),
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let error = RecognitionVisionProvider::read_text(&test_double, request())
            .expect_err("test double backend rejected");
        assert_eq!(error.code(), VisionProviderErrorCode::InvalidResponse);

        let panicking = VisionFfiProvider::new(
            Some((
                Box::new(PanickingOcrEngine),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let first = RecognitionVisionProvider::read_text(&panicking, request())
            .expect_err("panic converted to typed error");
        assert_eq!(first.code(), VisionProviderErrorCode::Internal);
        let second = RecognitionVisionProvider::read_text(&panicking, request())
            .expect_err("panicked engine stays retired");
        assert_eq!(second.code(), VisionProviderErrorCode::Unavailable);
    }

    #[test]
    fn adapter_preserves_typed_provider_timeout() {
        let provider = VisionFfiProvider::new(
            Some((Box::new(TimeoutOcrEngine), identity("PP-OCRv6_medium", 'a'))),
            None,
        )
        .expect("provider");
        let pixels = [0, 0, 0];
        let languages = vec!["en".to_string()];
        let model_sha256 = "a".repeat(64);

        let error = RecognitionVisionProvider::read_text(
            &provider,
            OcrProviderRequest {
                frame: VisionProviderFrame {
                    width: 1,
                    height: 1,
                    rgb8_pixels: &pixels,
                },
                region: PackRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                languages: &languages,
                timeout_ms: 1,
                model_ref: "PP-OCRv6_medium",
                model_sha256: &model_sha256,
                blocks: OcrProviderBlocks::Implicit,
            },
        )
        .expect_err("timeout remains typed");

        assert_eq!(error.code(), VisionProviderErrorCode::Timeout);
    }

    fn identity(model_ref: &str, hash_byte: char) -> VisionModelIdentity {
        VisionModelIdentity::new(model_ref, hash_byte.to_string().repeat(64)).expect("identity")
    }

    fn ocr_request<'a>() -> OcrProviderRequest<'a> {
        static PIXELS: [u8; 3] = [0, 0, 0];
        static LANGUAGES: [String; 1] = [String::new()];
        OcrProviderRequest {
            frame: VisionProviderFrame {
                width: 1,
                height: 1,
                rgb8_pixels: &PIXELS,
            },
            region: PackRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            languages: &LANGUAGES,
            timeout_ms: 1_000,
            model_ref: "PP-OCRv6_medium",
            model_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            blocks: OcrProviderBlocks::Implicit,
        }
    }

    fn execution_attestation(
        provider: &str,
        cuda_ordinal: Option<u32>,
        model_hash_byte: char,
    ) -> OcrExecutionAttestation {
        let requested_cuda_device = cuda_ordinal.map(|ordinal| {
            json!({
                "ordinal": ordinal,
                "expected_stable_identity": format!("cuda-{ordinal}")
            })
        });
        let resolved_cuda_device = cuda_ordinal.map(|ordinal| {
            json!({
                "ordinal": ordinal,
                "stable_identity": format!("cuda-{ordinal}"),
                "pci_bus_id": null
            })
        });
        serde_json::from_value(json!({
            "schema_version": "actingcommand.ocr_execution_attestation.v1",
            "invocation_id": "ocr-invocation-0000000000000001",
            "session": {
                "session_id": "ocr-session-0000000000000001",
                "generation": 1,
                "key": {
                    "provider_library_sha256": "b".repeat(64),
                    "runtime_library_path": "runtime.dll",
                    "runtime_library_sha256": "c".repeat(64),
                    "onnxruntime_version": "fixture-runtime",
                    "model_ref": "PP-OCRv6_medium",
                    "model_sha256": model_hash_byte.to_string().repeat(64),
                    "requested_backend": provider,
                    "requested_cuda_device": requested_cuda_device,
                    "resolved_cuda_device": resolved_cuda_device,
                    "provider_options_sha256": "d".repeat(64)
                }
            },
            "resolved_execution_provider": provider,
            "provider": {
                "implementation": "actingcommand-ppocr-onnx-json",
                "crate_version": "0.1.0",
                "build_git_sha": null,
                "binary_sha256": "b".repeat(64)
            },
            "runtime": {
                "onnxruntime_version": "fixture-runtime",
                "onnxruntime_build_info": "fixture",
                "cuda_driver_version": cuda_ordinal.map(|_| 1),
                "cuda_runtime_version": cuda_ordinal.map(|_| "fixture"),
                "cudnn_version": cuda_ordinal.map(|_| "fixture")
            },
            "registered_execution_providers": [provider],
            "cpu_ep_registered": provider == "cpu",
            "cpu_fallback_disabled": provider == "cuda",
            "fallback_policy": "forbidden",
            "fallback_observed": null,
            "complete": true
        }))
        .expect("fixture attestation")
    }

    /// #407 PERF-READ (register §1.280): a request's wait for its model's lock and its engine's
    /// inference are timed apart, on the provider's clock, and summed per model with their
    /// longest. The clock is scripted: each request reads it when it starts, when it holds the
    /// lock, and before and after the engine's call.
    #[test]
    fn vision_timing_separates_lock_wait_from_inference() {
        let mut provider = VisionFfiProvider::new(
            Some((
                Box::new(StaticOcrEngine {
                    backend: VisionBackendKind::FastDeployPpocr,
                    warnings: Vec::new(),
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let readings = Arc::new(Mutex::new(std::collections::VecDeque::from([
            // The first request: 300 µs for the lock, 700 µs in the engine.
            100_u64, 400, 400, 1_100,
            // The second: 50 µs for the lock, 10 µs between the lock and the call, 2 000 µs in
            // the engine.
            2_000, 2_050, 2_060, 4_060,
        ])));
        let script = Arc::clone(&readings);
        provider.clock = Arc::new(move || {
            script
                .lock()
                .expect("scripted clock")
                .pop_front()
                .expect("a scripted reading")
        });
        assert!(provider.model_timings().is_empty(), "no request yet");

        for _ in 0..2 {
            RecognitionVisionProvider::read_text(&provider, ocr_request()).expect("OCR read");
        }

        assert!(
            readings.lock().expect("scripted clock").is_empty(),
            "four readings per request"
        );
        assert_eq!(
            provider.model_timings(),
            vec![VisionModelTimingStatus {
                model_ref: "PP-OCRv6_medium".to_owned(),
                kind: VisionModelKind::Ocr,
                requests: 2,
                inferences: 2,
                lock_wait_total_us: 350,
                lock_wait_longest_us: 300,
                inference_total_us: 2_700,
                inference_longest_us: 2_000,
            }]
        );
    }

    /// A fake adapter identity (design §1.3's form); no real device.
    const FAKE_DXGI_IDENTITY: &str = "dxgi-pci:1234:5678:9abcdef0:01";

    /// Workflow #407 GPU-1 C (design §1.4, §2.4, §2.5), fix 2: a model that runs on the CPU
    /// after the fallback (DirectML requested, the CPU resolved) shows one status warning, the
    /// `vision_gpu_unavailable` its load hands over in slice B2's execution status
    /// (`OcrModelLoad::Loaded.execution`), once however many requests it runs, and each
    /// request's evidence says DirectML was asked for and the CPU ran it, its fallback observed;
    /// a model on DirectML shows none. The start's warning is the decision in effect until the
    /// first OCR model load completes: a load that failed leaves it shown, a completed load
    /// decides again and it is gone. The status reads the execution status live: a model the
    /// engine moves to the CPU after its load (B2's detector fallback) shows its one warning from
    /// then on.
    #[test]
    fn a_model_on_cpu_after_fallback_shows_one_status_warning() {
        let warning = Outcome::new(VisionGpuCode::VisionGpuUnavailable)
            .execution_provider(OnnxExecutionProvider::Directml)
            .gpu_unavailable_reason(GpuUnavailableReason::AdapterMissing)
            .gpu_fallback(GpuFallback::Cpu);
        // B2's warning of one model's own fallback names the model.
        let fallback = Outcome::new(VisionGpuCode::VisionGpuUnavailable)
            .execution_provider(OnnxExecutionProvider::Directml)
            .gpu_unavailable_reason(GpuUnavailableReason::SessionBuildFailed)
            .gpu_fallback(GpuFallback::Cpu)
            .model_name("PP-OCRv6_medium");
        let on_cpu = OcrModelExecution {
            requested: OnnxExecutionProvider::Directml,
            actual: OnnxExecutionProvider::Cpu,
            warning: Some(fallback.clone()),
        };

        // The load completes with the model on the CPU after its fallback.
        let loader = Arc::new(ScriptedOcrLoader {
            loads: Mutex::new(std::collections::VecDeque::from([Some(
                OcrExecutionStatus::new(on_cpu.clone()),
            )])),
        });
        let provider = scripted_ocr_provider(&loader).with_startup_warning(warning.clone());
        assert_eq!(provider.execution_warnings(), vec![warning.clone()]);
        for _ in 0..3 {
            let evidence = RecognitionVisionProvider::read_text_with_execution_evidence(
                &provider,
                ocr_request(),
            )
            .expect("attested OCR on the CPU")
            .execution
            .expect("execution evidence");
            assert_eq!(
                evidence.requested_provider,
                OcrExecutionProviderKind::Directml
            );
            assert_eq!(evidence.resolved_provider, OcrExecutionProviderKind::Cpu);
            assert!(evidence.cpu_ep_registered);
            assert!(!evidence.cpu_fallback_disabled);
            assert!(!evidence.fallback_forbidden);
            assert_eq!(evidence.fallback_observed, Some(true));
            assert_eq!(evidence.requested_cuda_ordinal, None);
            assert_eq!(evidence.resolved_cuda_ordinal, None);
        }
        assert_eq!(
            provider.execution_warnings(),
            vec![fallback.clone()],
            "one warning, the model's, never one per request; the completed load ended the start's"
        );
        assert!(
            loader.loads.lock().expect("scripted loads").is_empty(),
            "one load"
        );

        // The first load fails as `refuse` makes it, the second completes on DirectML.
        let on_directml = OcrExecutionStatus::new(OcrModelExecution::as_requested(
            OnnxExecutionProvider::Directml,
        ));
        let loader = Arc::new(ScriptedOcrLoader {
            loads: Mutex::new(std::collections::VecDeque::from([
                None,
                Some(on_directml.clone()),
            ])),
        });
        let provider = scripted_ocr_provider(&loader).with_startup_warning(warning.clone());
        assert_eq!(provider.execution_warnings(), vec![warning.clone()]);
        let error =
            RecognitionVisionProvider::read_text_with_execution_evidence(&provider, ocr_request())
                .expect_err("the first load fails");
        assert_eq!(error.code(), VisionProviderErrorCode::Unavailable);
        assert_eq!(
            provider.execution_warnings(),
            vec![warning],
            "a failed load leaves the start's warning"
        );
        let evidence =
            RecognitionVisionProvider::read_text_with_execution_evidence(&provider, ocr_request())
                .expect("the second load completes")
                .execution
                .expect("execution evidence");
        assert_eq!(
            evidence.requested_provider,
            OcrExecutionProviderKind::Directml
        );
        assert_eq!(
            evidence.resolved_provider,
            OcrExecutionProviderKind::Directml
        );
        assert!(!evidence.cpu_ep_registered);
        assert!(!evidence.fallback_forbidden);
        assert_eq!(evidence.fallback_observed, Some(true));
        assert!(
            provider.execution_warnings().is_empty(),
            "the completed load's decision is the one in effect, and a model on DirectML shows none"
        );
        // The engine moves the model to the CPU after its load (B2: a DirectML detector that
        // cannot be built on the first full-frame request moves the whole model).
        on_directml.set(on_cpu);
        assert_eq!(
            provider.execution_warnings(),
            vec![fallback],
            "the moved model's one warning, read live"
        );
        assert!(
            loader.loads.lock().expect("scripted loads").is_empty(),
            "two loads"
        );
    }

    /// A provider over one OCR model folder, built through `loader`.
    fn scripted_ocr_provider(loader: &Arc<ScriptedOcrLoader>) -> VisionFfiProvider {
        VisionFfiProvider::from_listing(
            VisionModelListing {
                ocr: vec![OcrModelSpec {
                    model_ref: "PP-OCRv6_medium".to_owned(),
                    layout: OcrModelLayout::Flat,
                    detector_path: None,
                    recognizer_path: PathBuf::from("rec.onnx"),
                    dictionary_path: PathBuf::from("keys.txt"),
                    description: VisionModelDescription::default_for(VisionModelFamily::PpocrCtc),
                    description_sha256: "e".repeat(64),
                }],
                nn: Vec::new(),
                invalid: Vec::new(),
            },
            Arc::clone(loader) as Arc<dyn VisionModelLoader>,
        )
        .expect("provider")
    }

    /// An OCR loader that fails (`None`) or completes each load in turn, handing over the
    /// scripted execution status (slice B2's `OcrModelLoad::Loaded.execution`); the engine is
    /// attested where that status says the model runs.
    struct ScriptedOcrLoader {
        loads: Mutex<std::collections::VecDeque<Option<OcrExecutionStatus>>>,
    }

    impl VisionModelLoader for ScriptedOcrLoader {
        fn load_ocr(
            &self,
            _model: &OcrModelSpec,
            expected_model_sha256: &str,
            _session_id: &OcrSessionId,
            _wait_deadline: Instant,
        ) -> VisionFfiResult<OcrModelLoad> {
            let Some(execution) = self
                .loads
                .lock()
                .expect("scripted loads")
                .pop_front()
                .expect("a scripted load")
            else {
                return Err(VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::ProviderUnavailable,
                    "fixture",
                    "DirectML cannot be used (adapter_missing)",
                ));
            };
            let resolved = execution.current().actual.as_str();
            Ok(OcrModelLoad::Loaded {
                engine: Box::new(AttestedOcrEngine {
                    attestation: directml_attestation(resolved),
                }),
                model_sha256: expected_model_sha256.to_owned(),
                waited_on_others: Duration::ZERO,
                execution,
            })
        }

        fn load_nn(
            &self,
            _model: &NnModelSpec,
            _expected_model_sha256: &str,
            _wait_deadline: Instant,
        ) -> VisionFfiResult<NnModelLoad> {
            Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                "fixture",
                "no NN model in this fixture",
            ))
        }
    }

    /// A DirectML session's attestation (Workflow #407 GPU-1 design §2.4), resolved on
    /// `resolved`: `directml` (CPU nodes allowed, the CPU execution provider not registered) or
    /// `cpu` (the whole model on the CPU, as slice B2 attests it: no resolved adapter, the
    /// fallback policy `cpu_model`). The fallback is observed either way.
    fn directml_attestation(resolved: &str) -> OcrExecutionAttestation {
        let on_cpu = resolved == "cpu";
        serde_json::from_value(json!({
            "schema_version": "actingcommand.ocr_execution_attestation.v1",
            "invocation_id": "ocr-invocation-0000000000000001",
            "session": {
                "session_id": "ocr-session-0000000000000001",
                "generation": 1,
                "key": {
                    "provider_library_sha256": "b".repeat(64),
                    "runtime_library_path": "runtime.dll",
                    "runtime_library_sha256": "c".repeat(64),
                    "onnxruntime_version": "fixture-runtime",
                    "model_ref": "PP-OCRv6_medium",
                    "model_sha256": "a".repeat(64),
                    "requested_backend": "directml",
                    "requested_cuda_device": null,
                    "resolved_cuda_device": null,
                    "requested_directml_device": {"expected_identity": FAKE_DXGI_IDENTITY},
                    "resolved_directml_device": if on_cpu {
                        serde_json::Value::Null
                    } else {
                        json!({"dxgi_index": 0, "identity": FAKE_DXGI_IDENTITY})
                    },
                    "provider_options_sha256": "d".repeat(64)
                }
            },
            "resolved_execution_provider": resolved,
            "provider": {
                "implementation": "actingcommand-ppocr-onnx-json",
                "crate_version": "0.1.0",
                "build_git_sha": null,
                "binary_sha256": "b".repeat(64)
            },
            "runtime": {
                "onnxruntime_version": "fixture-runtime",
                "onnxruntime_build_info": "fixture",
                "cuda_driver_version": null,
                "cuda_runtime_version": null,
                "cudnn_version": null
            },
            "registered_execution_providers": [resolved],
            "cpu_ep_registered": on_cpu,
            "cpu_fallback_disabled": false,
            "fallback_policy": if on_cpu { "cpu_model" } else { "cpu_nodes" },
            "fallback_observed": true,
            "complete": true
        }))
        .expect("fixture attestation")
    }

    struct RecordingNnEngine {
        observed: Arc<Mutex<Option<NnInferenceRequest>>>,
        backend: VisionBackendKind,
    }

    impl NnEngine for RecordingNnEngine {
        fn classify(
            &mut self,
            request: NnInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<NnClassificationResult> {
            *self.observed.lock().expect("observed request lock") = Some(request);
            Ok(NnClassificationResult {
                labels: vec![NnLabel {
                    label: "home".to_string(),
                    score: 0.99,
                }],
                backend: self.backend,
            })
        }
    }

    struct StaticOcrEngine {
        backend: VisionBackendKind,
        warnings: Vec<String>,
    }

    struct AttestedOcrEngine {
        attestation: OcrExecutionAttestation,
    }

    impl OcrEngine for AttestedOcrEngine {
        fn read_text(
            &mut self,
            request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceResult> {
            Ok(attested_result(request))
        }

        fn read_text_with_attestation(
            &mut self,
            request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceOutput> {
            Ok(OcrInferenceOutput {
                result: attested_result(request),
                execution_attestation: Some(self.attestation.clone()),
            })
        }
    }

    fn attested_result(request: OcrInferenceRequest) -> OcrInferenceResult {
        OcrInferenceResult {
            ppocr_diagnostics: Vec::new(),
            text: "home".to_string(),
            blocks: vec![OcrTextBlock {
                text: "home".to_string(),
                rect: request.region,
                confidence: Some(0.99),
            }],
            confidence: Some(0.99),
            backend: VisionBackendKind::FastDeployPpocr,
            warnings: Vec::new(),
            detection: None,
        }
    }

    impl OcrEngine for StaticOcrEngine {
        fn read_text(
            &mut self,
            request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceResult> {
            Ok(OcrInferenceResult {
                ppocr_diagnostics: Vec::new(),
                text: "home".to_string(),
                blocks: vec![OcrTextBlock {
                    text: "home".to_string(),
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                confidence: Some(0.99),
                backend: self.backend,
                warnings: self.warnings.clone(),
                detection: None,
            })
        }
    }

    struct PanickingOcrEngine;

    impl OcrEngine for PanickingOcrEngine {
        fn read_text(
            &mut self,
            _request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceResult> {
            panic!("fixture provider panic")
        }
    }

    struct TimeoutOcrEngine;

    impl OcrEngine for TimeoutOcrEngine {
        fn read_text(
            &mut self,
            _request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceResult> {
            Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Timeout,
                "fixture",
                "injected timeout",
            ))
        }
    }
}
