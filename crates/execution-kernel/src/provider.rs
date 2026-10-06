// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ExecutionKernelError, ExecutionKernelResult};
pub use actingcommand_contract::ExecutionBackendProvenance;
use actingcommand_contract::{
    ApplicationLifecycleAction, EmulatorInstanceAction, FencedWrite, InstanceId, MonitorObservation,
};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceErrorCategory, DeviceErrorSensitivity, DeviceResult, Frame,
    InputBackend,
};
pub use actingcommand_device::{
    EmulatorControlFailure, EmulatorControlOutcome, EmulatorControlResult,
};
pub use actingcommand_recognition_pack::VisionProvider as RecognitionVisionProvider;
use actingcommand_recognition_pack::{
    NnProviderLabel, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderExecutionEvidence, OcrProviderObservation, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProviderError, VisionProviderErrorCode,
    VisionProviderFrame,
};
use actingcommand_vision_ffi::{
    NnClassificationResult, NnEngine, NnInferenceRequest, NnModelLoad, NnModelSpec, OcrEngine,
    OcrExecutionAttestation, OcrFallbackPolicy, OcrInferenceRequestView, OcrInferenceResult,
    OcrModelLoad, OcrModelSpec, OcrSessionId, OnnxExecutionProvider, VisionBackendKind,
    VisionFfiError, VisionFfiErrorCode, VisionFfiResult, VisionFrame, VisionFrameView,
    VisionModelListing, VisionModelLoader, VisionPixelFormat, VisionRect, next_ocr_session_id,
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

/// One model: its own lock serialises its load and its inferences, never another model's.
struct ModelSlot<E: ?Sized, S> {
    model_ref: String,
    spec: Option<S>,
    admitted_sha256: Option<String>,
    /// The content identity established by the first hash of the model's files.
    verified_sha256: OnceLock<String>,
    cell: Mutex<SlotCell<E>>,
    last_used: AtomicU64,
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
        }
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
}

impl VisionFfiProvider {
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
    /// model's lock consumes the request's `timeout_ms`; the request's own model load does
    /// not; the engine receives what remains.
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
        let mut cell = lock_until(&slot.cell, deadline, "OCR", model_ref)?;
        let waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(reason) = &cell.retired {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                reason.clone(),
            ));
        }
        if cell.engine.is_none() {
            self.load_ocr(slot, &mut cell, model_sha256, deadline)?;
        }
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
            catch_unwind(AssertUnwindSafe(|| call(engine, remaining)))
        };
        match outcome {
            Ok(result) => result.map(|value| (value, identity)).map_err(map_ffi_error),
            Err(_) => {
                self.retire(&mut *cell, "OCR engine was retired after a provider panic");
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
    ) -> Result<(), VisionProviderError> {
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
        match loader.load_ocr(spec, model_sha256, &session_id, deadline) {
            Ok(OcrModelLoad::Loaded {
                engine,
                model_sha256: actual,
            }) => {
                if !slot.verify(&actual) {
                    self.release_loaded_model();
                    return Err(self.changed_on_disk(slot, cell, &actual));
                }
                cell.engine = Some(engine);
                cell.counted = true;
                Ok(())
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
    ) -> Result<(), VisionProviderError> {
        let (Some(spec), Some(loader)) = (slot.spec.as_ref(), self.loader.as_ref()) else {
            return Err(unavailable("NN"));
        };
        self.reserve_loaded_model()?;
        match loader.load_nn(spec, model_sha256, deadline) {
            Ok(NnModelLoad::Loaded {
                engine,
                model_sha256: actual,
            }) => {
                if !slot.verify(&actual) {
                    self.release_loaded_model();
                    return Err(self.changed_on_disk(slot, cell, &actual));
                }
                cell.engine = Some(engine);
                cell.counted = true;
                Ok(())
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
        self.retire(cell, &reason);
        VisionProviderError::new(VisionProviderErrorCode::Unavailable, reason)
    }

    fn retire<E: ?Sized>(&self, cell: &mut SlotCell<E>, reason: &str) {
        cell.engine = None;
        cell.retired = Some(reason.to_string());
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
            true
        }
        _ => false,
    }
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
        let mut cell = lock_until(&slot.cell, deadline, "NN", request.model_ref)?;
        let waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(reason) = &cell.retired {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                reason.clone(),
            ));
        }
        if cell.engine.is_none() {
            self.load_nn(slot, &mut cell, request.model_sha256, deadline)?;
        }
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
            catch_unwind(AssertUnwindSafe(|| engine.classify(ffi_request)))
        };
        let result = match outcome {
            Ok(result) => result.map_err(map_ffi_error)?,
            Err(_) => {
                self.retire(&mut *cell, "NN engine was retired after a provider panic");
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

fn map_ocr_result(result: OcrInferenceResult) -> OcrProviderResult {
    OcrProviderResult {
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
    let provider_kind = |provider| match provider {
        OnnxExecutionProvider::Cpu => OcrExecutionProviderKind::Cpu,
        OnnxExecutionProvider::Cuda => OcrExecutionProviderKind::Cuda,
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

/// The structured ADB target a registered instance was configured with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAdbEndpoint {
    host: String,
    port: u16,
    serial_configured: bool,
    discovered: Option<DiscoveredInstanceBinding>,
}

impl ResolvedAdbEndpoint {
    pub fn new(host: impl Into<String>, port: u16, serial_configured: bool) -> Self {
        Self {
            host: host.into(),
            port,
            serial_configured,
            discovered: None,
        }
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
                    "application.foreground_unsupported",
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    "observe_foreground_application",
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
                    "emulator_control.unsupported",
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    "control_instance",
                    DeviceErrorSensitivity::Sensitive,
                ),
            0,
        ))
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
                    "emulator_control.unsupported",
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    "rebind_discovered_endpoint",
                    DeviceErrorSensitivity::Sensitive,
                ),
        )
    }

    /// Re-runs the provider's instance discovery on demand and reports what it finds. Binds,
    /// rebinds and registers nothing and opens no device session. Providers without an
    /// instance-discovery surface keep this typed refusal.
    fn discover_instances(
        &self,
    ) -> Result<ProviderInstanceDiscovery, Box<InstanceDiscoveryFailure>> {
        Err(Box::new(InstanceDiscoveryFailure {
            code: "instance_discovery_unavailable",
            error: DeviceError::fatal("instance discovery unsupported by this provider")
                .with_diagnostic(
                    DeviceErrorCategory::Protocol,
                    "instance_discovery.unsupported",
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    "discover_instances",
                    DeviceErrorSensitivity::Sensitive,
                ),
        }))
    }

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        None
    }

    fn observe_monitor(
        &self,
        _instance_alias: &str,
        _expected_page: &str,
        _frame: &Frame,
    ) -> ExecutionKernelResult<MonitorObservation> {
        Err(ExecutionKernelError::fatal(
            "monitor_observation_unavailable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_vision_ffi::{
        NnLabel, OcrInferenceOutput, OcrInferenceRequest, OcrTextBlock,
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
