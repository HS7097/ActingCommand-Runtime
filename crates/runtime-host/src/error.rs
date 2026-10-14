// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::{ContractLocation, DeviceSelfCheckFailure, SharedCode};
use actingcommand_contract::outcome::{CauseRelation, Code, Location, Outcome};
use actingcommand_contract::{
    CleanupCauseDraft, DiagnosticDetailDraft, InstanceId, ResourceQuiescence, RuntimeErrorCode,
    RuntimeErrorProjection,
};
use actingcommand_execution_kernel::{
    ExecutionFailureContext, ExecutionKernelCode, ExecutionKernelError,
};
use actingcommand_runtime_state::RuntimeStateError;
use actingcommand_scheduler::SchedulerError;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub type RuntimeHostResult<T> = Result<T, RuntimeHostError>;

/// The fixed B7 failure joins. Each join retains both original errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeFailureRelation {
    AdmissionRecord,
    DiagnosticArchive,
    LifecycleRecord,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RuntimeCompleteFailure {
    pub(crate) primary: RuntimeHostError,
    pub(crate) secondary: Vec<(RuntimeFailureRelation, RuntimeHostError)>,
}

/// Original values held only while returning a B7 archive/recording failure.
#[derive(Clone, PartialEq, serde::Serialize)]
pub(crate) enum PpocrFailureSource {
    Pages(Box<actingcommand_page_detector::PageBatchResult>),
    Recognition(Box<actingcommand_recognition_pack::RecognitionPackError>),
    Observation {
        status: actingcommand_contract::PageObservationStatus,
        facts: actingcommand_contract::ObservationFacts,
    },
    Saved {
        message: String,
        conflicting_pages: Option<Vec<String>>,
    },
}

#[derive(Clone)]
pub struct RuntimeHostError {
    code: HostErrorCode,
    operation: HostErrorOperation,
    projection: Box<RuntimeErrorProjection>,
    pub(crate) lifecycle: Box<RuntimeHostFailureContext>,
}

/// The code a [`RuntimeHostError`] carries: a registered code, or the spelling of a code that
/// it relays from an error type whose owner has not typed its codes yet (Workflow #378 A2b).
#[derive(Clone, Copy)]
enum HostErrorCode {
    Registered(Code),
    /// A code of the store, state, execution or policy crates (or of another error that relays
    /// one), carried as its spelling until A2c and A2d type those crates' codes.
    Relayed {
        relayed_code: &'static str,
    },
}

impl HostErrorCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Registered(code) => code.as_str(),
            Self::Relayed { relayed_code } => relayed_code,
        }
    }
}

/// The operation a [`RuntimeHostError`] names: a registered location, or the spelling of an
/// operation that it relays together with a relayed code (Workflow #378 A2b).
#[derive(Clone, Copy)]
enum HostErrorOperation {
    Registered(Location),
    /// An operation of the store, state, execution or policy crates, carried as its spelling
    /// until A2c and A2d type those crates' locations.
    Relayed {
        relayed_operation: &'static str,
    },
}

impl HostErrorOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Registered(operation) => operation.as_str(),
            Self::Relayed { relayed_operation } => relayed_operation,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct RuntimeHostFailureContext {
    pub(crate) installation_effect: Option<actingcommand_contract::EffectDisposition>,
    diagnostics: ExecutionFailureContext,
    /// The registered lifecycle stage the failure arose in (ledger revision 3: the `stage` of
    /// its runtime lifecycle failure's outcome).
    pub(crate) failure_stage: Option<Location>,
    pub(crate) ppocr_diagnostics: actingcommand_contract::PpocrDiagnostics,
    pub(crate) ppocr_message: Option<String>,
    pub(crate) ppocr_source: Option<Arc<PpocrFailureSource>>,
    pub(crate) ppocr_artifact_failure:
        Option<Arc<actingcommand_artifact_store::ArtifactStoreError>>,
    pub(crate) complete_failure: Option<Box<RuntimeCompleteFailure>>,
    pub(crate) task_timing: Option<Box<actingcommand_contract::TaskTimingObservations>>,
    pub(crate) capacity: Option<actingcommand_contract::CapacityDecision>,
    pub(crate) raw_os_error: Option<i32>,
    pub(crate) incomplete_device_diagnostic_summary: Option<(&'static str, &'static str)>,
    /// Slice #315-B2c-2: the previous owner probe behind an `owner_resource_unconfirmed`
    /// refusal (`pid <n> alive` or `probe unknown: <reason>`), shown after the code.
    pub(crate) owner_probe: Option<String>,
    /// Workflow #381 A R3′: the `(code, operation)` of the condition latched in the Runtime's
    /// fatal state that stopped an install start; shown after the operation as
    /// `cause=<code> cause_operation=<operation>` on the developer line and in the transition's
    /// status fact. The ledger records it as the `caused_by` link of the error's outcome.
    pub(crate) latched_cause: Option<(&'static str, &'static str)>,
    /// The failure behind this error (ledger revision 3): the `caused_by` link of its outcome.
    pub(crate) caused_by: Option<Outcome>,
    /// Workflow #378 A6a: facts measured before the failure, the `note` links of its outcome
    /// (a failed ledger open's `ledger_open_measured`, model section 7.4 N1).
    notes: Vec<Outcome>,
    /// The error's outcome with the values its code declares, when its producer attached them
    /// ([`RuntimeHostError::with_outcome_values`]); otherwise the outcome is its code and
    /// operation alone.
    outcome_values: Option<Outcome>,
    pub(crate) instance_id: Option<InstanceId>,
    pub(crate) resource_quiescence: Option<ResourceQuiescence>,
    pub(crate) policy_rejection: Option<Box<actingcommand_contract::PolicyDispatchRejection>>,
    pub(crate) resource_declaration:
        Option<Box<actingcommand_contract::ResourceDeclarationRejection>>,
    pub(crate) resource_declaration_event: Option<actingcommand_contract::TerminalEvent>,
    /// The field-positioned rejection of an `ApplyResourceTargets` document (Workflow #308
    /// RT-S1a); its receipt carries it and nothing is recorded.
    pub(crate) resource_targets_rejection:
        Option<Box<actingcommand_contract::ResourceTargetsRejection>>,
}

impl PartialEq for RuntimeHostError {
    fn eq(&self, other: &Self) -> bool {
        self.code() == other.code()
            && self.operation() == other.operation()
            && self.projection == other.projection
            && self.lifecycle.complete_failure == other.lifecycle.complete_failure
            && self.lifecycle.failure_stage == other.lifecycle.failure_stage
            && self.lifecycle.installation_effect == other.lifecycle.installation_effect
            && self.lifecycle.ppocr_message == other.lifecycle.ppocr_message
            && self.lifecycle.ppocr_source == other.lifecycle.ppocr_source
            && self.lifecycle.ppocr_diagnostics == other.lifecycle.ppocr_diagnostics
            && self.lifecycle.ppocr_artifact_failure == other.lifecycle.ppocr_artifact_failure
            && self.diagnostic_detail() == other.diagnostic_detail()
            && self.cleanup_cause() == other.cleanup_cause()
            && self.lifecycle.policy_rejection == other.lifecycle.policy_rejection
            && self.lifecycle.resource_declaration == other.lifecycle.resource_declaration
            && self.lifecycle.resource_declaration_event
                == other.lifecycle.resource_declaration_event
            && self.lifecycle.resource_targets_rejection
                == other.lifecycle.resource_targets_rejection
            && self.lifecycle.incomplete_device_diagnostic_summary
                == other.lifecycle.incomplete_device_diagnostic_summary
            && self.lifecycle.owner_probe == other.lifecycle.owner_probe
            && self.lifecycle.latched_cause == other.lifecycle.latched_cause
            && self.lifecycle.caused_by == other.lifecycle.caused_by
            && self.lifecycle.notes == other.lifecycle.notes
            && self.lifecycle.outcome_values == other.lifecycle.outcome_values
    }
}
impl Eq for RuntimeHostError {}

impl From<actingcommand_policy::PolicyEvaluationError> for RuntimeHostError {
    fn from(error: actingcommand_policy::PolicyEvaluationError) -> Self {
        // Workflow #355 D12: the lifecycle record names the policy's failure. Ledger revision 3
        // keeps its code as the `caused_by` link; the policy's English message is not recorded.
        let mut result = Self::request(
            HostCode::PolicyEvaluationRejected,
            HostLocation::EvaluatePolicyCycle,
            RuntimeErrorCode::InvalidRequest,
        );
        result.lifecycle.caused_by = Some(Outcome::new(error.code()));
        result
    }
}

impl RuntimeHostError {
    pub(crate) fn diagnostics(&self) -> &ExecutionFailureContext {
        &self.lifecycle.diagnostics
    }

    pub(crate) fn with_native_failure_detail(
        mut self,
        detail: actingcommand_contract::LifecycleNativeDetail,
    ) -> Self {
        self.lifecycle.diagnostics = self.lifecycle.diagnostics.with_native_detail(detail);
        self
    }

    pub(crate) fn with_recording_from(mut self, source: &Self) -> Self {
        self.lifecycle.diagnostics = self
            .lifecycle
            .diagnostics
            .with_recording_from(source.diagnostics());
        self
    }

    pub(crate) fn with_related_causes(mut self, related: &Self) -> Self {
        self.lifecycle.diagnostics = self
            .lifecycle
            .diagnostics
            .with_related_causes(related.diagnostics());
        self
    }

    pub(crate) fn with_failure_stage(mut self, stage: impl Into<Location>) -> Self {
        let stage: Location = stage.into();
        self.lifecycle.failure_stage.get_or_insert(stage);
        if let Some(complete) = &mut self.lifecycle.complete_failure {
            complete
                .primary
                .lifecycle
                .failure_stage
                .get_or_insert(stage);
        }
        self
    }

    pub(crate) fn has_ppocr_diagnostics(&self) -> bool {
        !self.lifecycle.ppocr_diagnostics.is_empty()
            || self
                .lifecycle
                .complete_failure
                .as_ref()
                .is_some_and(|complete| {
                    complete.primary.has_ppocr_diagnostics()
                        || complete
                            .secondary
                            .iter()
                            .any(|(_, error)| error.has_ppocr_diagnostics())
                })
    }

    /// Joins one of B7's original admission/archive/recording boundaries. Callers
    /// join once per boundary; this is not a retry or an input-driven error queue.
    pub(crate) fn with_complete_failure(
        mut self,
        relation: RuntimeFailureRelation,
        mut secondary: Self,
    ) -> Self {
        let mut projection = self.projection.clone();
        projection.fatal |= secondary.is_fatal();
        if secondary.projection.code == RuntimeErrorCode::LedgerFailure {
            projection = secondary.projection.clone();
        }
        let mut complete = self
            .lifecycle
            .complete_failure
            .take()
            .map(|value| *value)
            .unwrap_or_else(|| RuntimeCompleteFailure {
                primary: self.clone(),
                secondary: Vec::new(),
            });
        match secondary.lifecycle.complete_failure.take() {
            Some(other) => {
                complete.secondary.push((relation, other.primary));
                complete.secondary.extend(other.secondary);
            }
            None => complete.secondary.push((relation, secondary)),
        }
        let mut combined = self;
        combined.projection = projection;
        combined.lifecycle.complete_failure = Some(Box::new(complete));
        combined
    }

    /// Complete original public displays for the process shell. Native details
    /// remain in the original typed errors and the Ledger's privacy projection.
    pub fn complete_message(&self) -> String {
        if let Some(complete) = &self.lifecycle.complete_failure {
            let mut message = complete.primary.complete_message();
            for (relation, error) in &complete.secondary {
                message.push_str(&format!("; {relation:?}: {}", error.complete_message()));
            }
            return message;
        }
        match self.lifecycle.failure_stage {
            Some(stage) => format!("{self} [stage={stage}]"),
            None => self.to_string(),
        }
    }

    /// The policy dispatch rejection of this error: the one recorded on it, else one of its
    /// outcome (ledger revision 3: the refusal's code with its `operation`; no `fatal` flag).
    pub(crate) fn policy_rejection(&self) -> actingcommand_contract::PolicyDispatchRejection {
        self.lifecycle
            .policy_rejection
            .as_deref()
            .cloned()
            .unwrap_or_else(|| actingcommand_contract::PolicyDispatchRejection {
                outcome: self.outcome(),
                budget: None,
                next_eligible_unix_ms: None,
            })
    }

    /// The error as an outcome (ledger revision 3): its code (a registered one, or the spelling
    /// it relays, whose category the embedded catalog gives) with its `operation` (a registered
    /// location, or the relayed spelling when the catalog lists it), and the failure behind it
    /// as its `caused_by` link. Every record that carries this failure builds its outcome here.
    pub(crate) fn outcome(&self) -> Outcome {
        let outcome = self
            .lifecycle
            .outcome_values
            .clone()
            .unwrap_or_else(|| self.code_outcome());
        let outcome = match &self.lifecycle.caused_by {
            Some(cause) => outcome.caused_by(cause.clone()),
            None => outcome,
        };
        self.lifecycle
            .notes
            .iter()
            .cloned()
            .fold(outcome, |outcome, note| {
                outcome.link(CauseRelation::Note, note)
            })
    }

    /// This error with a fact measured before it failed as a `note` link of its outcome
    /// (Workflow #378 A6a).
    pub(crate) fn with_note(mut self, note: Outcome) -> Self {
        self.lifecycle.notes.push(note);
        self
    }

    /// The outcome a record of this failure carries as its own (`capture.failed`,
    /// `input.failed`, a `runtime.failed` of the error; ledger revision 3): [`Self::outcome`]
    /// with the error's native text as a `caused_by` foreign link, `foreign_os_error` when the
    /// error has an OS error number. A lifecycle failure takes [`Self::outcome`] instead: its
    /// draft attaches the native text and the OS error itself.
    pub(crate) fn record_outcome(&self) -> Outcome {
        let outcome = self.outcome();
        let native = self.diagnostics().native_detail();
        match (native, self.lifecycle.raw_os_error) {
            (Some(native), Some(os_error)) => outcome.caused_by(native.os_outcome(os_error)),
            (Some(native), None) => outcome.caused_by(native.outcome()),
            (None, Some(os_error)) => {
                outcome.caused_by(actingcommand_contract::os_error_outcome(os_error))
            }
            (None, None) => outcome,
        }
    }

    /// The outcome of the error's code with its operation, before any value or link.
    fn code_outcome(&self) -> Outcome {
        let outcome = match self.code {
            HostErrorCode::Registered(code) => Outcome::new(code),
            HostErrorCode::Relayed { relayed_code } => Outcome::relayed(relayed_code),
        };
        let operation = match self.operation {
            HostErrorOperation::Registered(operation) => Some(operation),
            HostErrorOperation::Relayed { relayed_operation } => {
                Location::relayed(relayed_operation)
            }
        };
        match operation {
            Some(operation) => outcome.operation(operation),
            None => outcome,
        }
    }

    /// This error with the values its code declares on its outcome (ledger revision 3): `values`
    /// receives the outcome of the error's code and operation and returns it with the values.
    pub(crate) fn with_outcome_values(mut self, values: impl FnOnce(Outcome) -> Outcome) -> Self {
        let outcome = values(self.code_outcome());
        self.lifecycle.outcome_values = Some(outcome);
        self
    }

    /// Whether the error's producer attached values to its outcome
    /// ([`Self::with_outcome_values`]): the outcome is then more than its code and operation, and
    /// a record that carries only those does not carry the failure (ledger revision 3).
    pub(crate) fn has_outcome_values(&self) -> bool {
        self.lifecycle.outcome_values.is_some()
    }

    pub(crate) fn artifact(error: actingcommand_artifact_store::ArtifactStoreError) -> Self {
        let fatal = error.is_fatal();
        Self::artifact_as(error, fatal)
    }

    /// Workflow #375 R5b: an artifact-store failure of a material read, which leaves the
    /// Runtime running: a request error with the store's code, operation and native detail,
    /// whatever the store's own category.
    pub(crate) fn artifact_read(error: actingcommand_artifact_store::ArtifactStoreError) -> Self {
        Self::artifact_as(error, false)
    }

    fn artifact_as(error: actingcommand_artifact_store::ArtifactStoreError, fatal: bool) -> Self {
        let mut result = if fatal {
            Self::relayed_fatal(
                error.code(),
                error.operation(),
                RuntimeErrorCode::RuntimeHalted,
            )
        } else {
            Self::relayed_request(
                error.code(),
                error.operation(),
                RuntimeErrorCode::InvalidRequest,
            )
        };
        result = result.with_native_failure_detail(error.native_detail());
        result.lifecycle.capacity = error.capacity().cloned();
        result.lifecycle.raw_os_error = error.raw_os_error();
        result
    }
    /// The code's spelling: a registered code, or one the error relays (A2c and A2d type the
    /// relayed ones, then this returns a [`Code`]).
    pub const fn code(&self) -> &'static str {
        self.code.as_str()
    }

    /// The operation's spelling: a registered location, or one the error relays (A2c and A2d
    /// type the relayed ones, then this returns a [`Location`]).
    pub const fn operation(&self) -> &'static str {
        self.operation.as_str()
    }

    pub const fn is_fatal(&self) -> bool {
        self.projection.fatal
    }

    pub fn projection(&self) -> &RuntimeErrorProjection {
        &self.projection
    }

    /// This error as the outcome of a denied or failed receipt (Workflow #378 A4, model section
    /// 7.2): the outcome its ledger record carries ([`Self::outcome`]: its code on top with the
    /// values its producer attached, its operation, and the failure behind it as the `caused_by`
    /// link), with its projection's Runtime error code, halt, holder and retry as values
    /// ([`RuntimeErrorProjection::with_receipt_values`]). A receipt and the record of the same
    /// failure carry one outcome; a relayed spelling the catalog does not list tops both as
    /// `outcome_relay_unregistered` with its `raw_code`.
    pub(crate) fn receipt_outcome(&self) -> Outcome {
        self.projection.with_receipt_values(self.outcome())
    }

    /// Workflow #378 A6a (model section 2.3, M1a): how a latched host answers a later request.
    /// A fatal condition answers as itself, so the fatal-link rule holds; an error condition
    /// answers as `runtime_halted`, with the condition as its `caused_by` link and the
    /// condition's projection (its Runtime error code and the halt) kept.
    pub(crate) fn latched_answer(self) -> Self {
        let condition = self.outcome();
        if condition.holds_fatal() {
            return self;
        }
        Self::with_projection(
            RuntimeErrorCode::RuntimeHalted,
            HostLocation::ReadRuntimeFatal,
            self.projection.as_ref().clone(),
        )
        .with_caused_by(condition)
    }

    /// Workflow #378 A6a (model section 7.4): the error as actingd's daemon line and terminal
    /// outcome carry it. It is the outcome its record carries ([`Self::record_outcome`]: the
    /// code, its values and operation, the failure behind it and the native text as a foreign
    /// link), with the stage it arose in and each joined original error (B7) as a link of its
    /// relation. A joined error with a fatal outcome under a top that is not fatal tops the
    /// outcome instead, so a fatal link always sits under a fatal top.
    pub fn daemon_outcome(&self) -> Outcome {
        let Some(complete) = &self.lifecycle.complete_failure else {
            return self.staged_record_outcome();
        };
        complete.secondary.iter().fold(
            complete.primary.staged_record_outcome(),
            |top, (relation, error)| {
                let relation = match relation {
                    RuntimeFailureRelation::AdmissionRecord => CauseRelation::AdmissionRecord,
                    RuntimeFailureRelation::DiagnosticArchive => CauseRelation::DiagnosticArchive,
                    RuntimeFailureRelation::LifecycleRecord => CauseRelation::LifecycleRecord,
                };
                top.joined(relation, error.staged_record_outcome())
            },
        )
    }

    /// [`Self::record_outcome`] with the lifecycle stage the failure arose in.
    fn staged_record_outcome(&self) -> Outcome {
        let outcome = self.record_outcome();
        match self.lifecycle.failure_stage {
            Some(stage) if outcome.value("stage").is_none() => outcome.stage(stage),
            _ => outcome,
        }
    }

    /// This refusal naming the lease it refused (Workflow #378 A4): `lease_expired` and
    /// `lease_missing` carry it as `lease_id` wherever the token is in scope. An error with any
    /// other code is returned as it is.
    pub(crate) fn naming_lease(self, lease_id: actingcommand_contract::LeaseId) -> Self {
        if !spelled(
            self.code(),
            &[
                RuntimeErrorCode::LeaseExpired.code(),
                RuntimeErrorCode::LeaseMissing.code(),
            ],
        ) {
            return self;
        }
        let lease_id = crate::failure_identity::identifier_text(&lease_id);
        self.with_outcome_values(|outcome| outcome.lease_id(lease_id))
    }

    pub fn resource_declaration(
        &self,
    ) -> Option<&actingcommand_contract::ResourceDeclarationRejection> {
        self.lifecycle.resource_declaration.as_deref()
    }

    pub fn resource_targets_rejection(
        &self,
    ) -> Option<&actingcommand_contract::ResourceTargetsRejection> {
        self.lifecycle.resource_targets_rejection.as_deref()
    }

    /// The bounded native detail, if any. The ledger keeps it under its declared sensitivity;
    /// a process shell prints it only for refusals whose detail it knows to carry no device or
    /// vendor output, such as the catalog plan's remedy (Workflow #361 A).
    pub fn native_detail(&self) -> Option<&str> {
        self.diagnostics()
            .native_detail()
            .map(actingcommand_contract::LifecycleNativeDetail::text)
    }

    /// A refused `ApplyResourceTargets` document: non-fatal `InvalidRequest`, host code
    /// `resource_targets_rejected`, carrying the field-positioned rejection.
    pub(crate) fn resource_targets_rejected(
        rejection: actingcommand_contract::ResourceTargetsRejection,
    ) -> Self {
        let mut error = Self::request(
            HostCode::ResourceTargetsRejected,
            ContractLocation::ApplyResourceTargets,
            RuntimeErrorCode::InvalidRequest,
        );
        error.lifecycle.resource_targets_rejection = Some(Box::new(rejection));
        error
    }

    pub(crate) fn into_fatal(mut self) -> Self {
        self.projection.fatal = true;
        self
    }

    pub(crate) fn diagnostic_detail(&self) -> Option<&DiagnosticDetailDraft> {
        self.diagnostics().diagnostic_detail()
    }

    pub(crate) fn with_diagnostic_detail(mut self, detail: DiagnosticDetailDraft) -> Self {
        self.lifecycle.diagnostics = self.lifecycle.diagnostics.with_diagnostic_detail(detail);
        self
    }

    pub(crate) fn cleanup_cause(&self) -> Option<&CleanupCauseDraft> {
        self.diagnostics().cleanup_cause()
    }

    pub(crate) fn fatal(
        code: impl Into<Code>,
        operation: impl Into<Location>,
        runtime_code: RuntimeErrorCode,
    ) -> Self {
        Self::with_projection(
            code,
            operation,
            RuntimeErrorProjection::new(runtime_code, true),
        )
    }

    pub(crate) fn request(
        code: impl Into<Code>,
        operation: impl Into<Location>,
        runtime_code: RuntimeErrorCode,
    ) -> Self {
        Self::with_projection(
            code,
            operation,
            RuntimeErrorProjection::new(runtime_code, false),
        )
    }

    pub(crate) fn with_projection(
        code: impl Into<Code>,
        operation: impl Into<Location>,
        projection: RuntimeErrorProjection,
    ) -> Self {
        Self::build(
            HostErrorCode::Registered(code.into()),
            HostErrorOperation::Registered(operation.into()),
            projection,
        )
    }

    /// A fatal error that relays the code and operation of another owner's error as spellings
    /// (the store, state, execution and policy crates; A2c and A2d type them and remove it).
    pub(crate) fn relayed_fatal(
        relayed_code: &'static str,
        relayed_operation: &'static str,
        runtime_code: RuntimeErrorCode,
    ) -> Self {
        Self::relayed_with_projection(
            relayed_code,
            relayed_operation,
            RuntimeErrorProjection::new(runtime_code, true),
        )
    }

    /// A request error that relays the code and operation of another owner's error as spellings
    /// (the store, state, execution and policy crates; A2c and A2d type them and remove it).
    pub(crate) fn relayed_request(
        relayed_code: &'static str,
        relayed_operation: &'static str,
        runtime_code: RuntimeErrorCode,
    ) -> Self {
        Self::relayed_with_projection(
            relayed_code,
            relayed_operation,
            RuntimeErrorProjection::new(runtime_code, false),
        )
    }

    /// An error with the given projection that relays the code and operation of another owner's
    /// error as spellings (A2c and A2d type them and remove this bridge).
    pub(crate) fn relayed_with_projection(
        relayed_code: &'static str,
        relayed_operation: &'static str,
        projection: RuntimeErrorProjection,
    ) -> Self {
        Self::build(
            HostErrorCode::Relayed { relayed_code },
            HostErrorOperation::Relayed { relayed_operation },
            projection,
        )
    }

    fn build(
        code: HostErrorCode,
        operation: HostErrorOperation,
        projection: RuntimeErrorProjection,
    ) -> Self {
        Self {
            code,
            operation,
            projection: Box::new(projection),
            lifecycle: Box::default(),
        }
    }

    pub(crate) fn scheduler(operation: impl Into<Location>, error: &SchedulerError) -> Self {
        Self::with_projection(error.code(), operation, error.projection())
    }

    pub(crate) fn execution(operation: impl Into<Location>, error: &ExecutionKernelError) -> Self {
        let kernel_code = error.code().as_str();
        let runtime_code = match kernel_code {
            _ if spelled(kernel_code, &[SharedCode::FrameWorkspaceUnavailable.code()]) => {
                RuntimeErrorCode::InvalidRequest
            }
            _ if spelled(
                kernel_code,
                &[DeviceSelfCheckFailure::InputBackendOpenFailed.code()],
            ) =>
            {
                RuntimeErrorCode::BackendOpenFailed
            }
            _ if spelled(
                kernel_code,
                &[SharedCode::InputBackendOperationFailed.code()],
            ) =>
            {
                RuntimeErrorCode::BackendOperationFailed
            }
            _ if spelled(
                kernel_code,
                &[
                    DeviceSelfCheckFailure::CaptureBackendOpenFailed.code(),
                    SharedCode::CaptureFrameInvalid.code(),
                    SharedCode::CaptureBackendOperationFailed.code(),
                    ExecutionKernelCode::ExecutionSessionClosePending.code(),
                    ExecutionKernelCode::CaptureGeometryKernelBusy.code(),
                    ExecutionKernelCode::CaptureGeometryKernelClosed.code(),
                    ExecutionKernelCode::CaptureGeometrySessionMissing.code(),
                    ExecutionKernelCode::CaptureGeometrySessionChanged.code(),
                    ExecutionKernelCode::CaptureGeometryQueueFull.code(),
                    ExecutionKernelCode::CaptureGeometryDeadlineElapsed.code(),
                    ExecutionKernelCode::CaptureGeometrySessionBusy.code(),
                    ExecutionKernelCode::CaptureGeometrySessionClosed.code(),
                    ExecutionKernelCode::CaptureGeometryReadFailed.code(),
                ],
            ) =>
            {
                RuntimeErrorCode::CaptureFailed
            }
            _ if spelled(
                kernel_code,
                &[ExecutionKernelCode::MonitorObservationUnavailable.code()],
            ) =>
            {
                RuntimeErrorCode::RecognitionFailed
            }
            "monitor_observation_failed" => RuntimeErrorCode::RecognitionFailed,
            _ => RuntimeErrorCode::RuntimeHalted,
        };
        let runtime_error = Self {
            code: HostErrorCode::Registered(error.code()),
            operation: HostErrorOperation::Registered(operation.into()),
            projection: Box::new(RuntimeErrorProjection::new(runtime_code, error.is_fatal())),
            lifecycle: Box::new(RuntimeHostFailureContext {
                diagnostics: error.failure_context().clone(),
                instance_id: error.instance_id(),
                resource_quiescence: error.resource_quiescence(),
                ..RuntimeHostFailureContext::default()
            }),
        };
        if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
            runtime_error.into_fatal()
        } else {
            runtime_error
        }
    }

    pub(crate) fn readonly_capture(error: &ExecutionKernelError) -> Self {
        let mut result = Self::execution(HostLocation::ExecuteCaptureBackend, error);
        if error.code() == SharedCode::CaptureFrameInvalid.code()
            && error.secondary_code().is_none()
            && error.lifecycle_causes().is_empty()
            && error.cleanup_cause().is_none()
            && error.resource_quiescence() != Some(ResourceQuiescence::Unconfirmed)
        {
            *result.projection =
                RuntimeErrorProjection::new(RuntimeErrorCode::CaptureFailed, false);
        }
        result
    }

    /// A contained task's first capture lost to an unreachable ADB baseline (#316-P4): the
    /// kernel code stays, projected as a nonfatal `CaptureFailed`.
    pub(crate) fn adb_unreachable_capture(
        operation: impl Into<Location>,
        error: &ExecutionKernelError,
    ) -> Self {
        let mut result = Self::execution(operation, error);
        if error.resource_quiescence() != Some(ResourceQuiescence::Unconfirmed) {
            *result.projection =
                RuntimeErrorProjection::new(RuntimeErrorCode::CaptureFailed, false);
        }
        result
    }

    /// A contained task's first capture failed on a device session kept from an earlier lease
    /// while the ADB baseline answers (Workflow #191 H): the idle session went stale. The kernel
    /// code stays, projected as a nonfatal `CaptureFailed`; an unconfirmed close stays fatal.
    pub(crate) fn reused_session_capture(
        operation: impl Into<Location>,
        error: &ExecutionKernelError,
    ) -> Self {
        let mut result = Self::execution(operation, error);
        if error.resource_quiescence() != Some(ResourceQuiescence::Unconfirmed) {
            *result.projection =
                RuntimeErrorProjection::new(RuntimeErrorCode::CaptureFailed, false);
        }
        result
    }

    pub(crate) fn state(error: &RuntimeStateError) -> Self {
        if error.is_fatal() {
            Self::relayed_fatal(
                error.code(),
                error.operation(),
                RuntimeErrorCode::RuntimeHalted,
            )
        } else {
            Self::relayed_request(
                error.code(),
                error.operation(),
                RuntimeErrorCode::InvalidRequest,
            )
        }
    }

    /// [`Self::with_native_detail`] for MuMuManager's output from a command that exited: the
    /// text becomes a `foreign_mumu_manager_error` link with the exit status as `exit_code`
    /// (ledger revision 3). An exit status is never an OS error.
    pub(crate) fn with_mumu_manager_output(self, output: &str, exit_code: i32) -> Self {
        match actingcommand_contract::LifecycleNativeDetail::bounded(output) {
            Some(native) => {
                self.with_native_failure_detail(native.with_mumu_manager_exit_code(exit_code))
            }
            None => self,
        }
    }

    pub(crate) fn with_native_detail(self, detail: String) -> Self {
        let mut end = detail.len().min(1024);
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        self.with_native_failure_detail(actingcommand_contract::LifecycleNativeDetail::new(
            &detail[..end],
            end < detail.len(),
        ))
    }

    /// Workflow #381 A R3′: this error with the condition `cause` latched in the fatal state named
    /// on it: on the developer line, and (ledger revision 3) as the `caused_by` link of its
    /// outcome, which its lifecycle failure records.
    pub(crate) fn with_latched_cause(mut self, cause: &Self) -> Self {
        self.lifecycle.latched_cause = Some((cause.code(), cause.operation()));
        self.lifecycle.caused_by = Some(cause.outcome());
        self
    }

    /// Workflow #398 RD-2 (model-398 v1.1 §4.4; spec §5, the condition is the result): this
    /// device failure as the condition that ended its work, the handset's device link no longer
    /// `device`: `device_link_lost` on top with the handset's label and link state, and this
    /// failure, as it was, its `caused_by` link. The lost link explains the device failure, as
    /// an unreachable adb does (`adb_unreachable_capture`): the projection keeps its code and
    /// is nonfatal, unless a resource close stayed unconfirmed. The instance, resources and
    /// diagnostics it carries stay as they were.
    pub(crate) fn into_device_link_lost(
        mut self,
        device_label: &str,
        state: actingcommand_contract::DeviceLinkState,
    ) -> Self {
        let cause = self.outcome();
        self.code = HostErrorCode::Registered(HostCode::DeviceLinkLost.into());
        let outcome = self
            .code_outcome()
            .device_label(device_label)
            .device_link_state(state);
        self.lifecycle.outcome_values = Some(outcome);
        self.lifecycle.notes.clear();
        self.lifecycle.caused_by = Some(cause);
        if self.lifecycle.resource_quiescence != Some(ResourceQuiescence::Unconfirmed) {
            self.projection.fatal = false;
        }
        self
    }

    /// This error with the failure behind it as the `caused_by` link of its outcome (ledger
    /// revision 3), in place of that failure's text.
    pub(crate) fn with_caused_by(mut self, cause: Outcome) -> Self {
        self.lifecycle.caused_by = Some(cause);
        self
    }

    pub(crate) const fn latched_cause(&self) -> Option<(&'static str, &'static str)> {
        self.lifecycle.latched_cause
    }

    pub(crate) fn with_related_failure(mut self, relation: &'static str, other: &Self) -> Self {
        if self.has_ppocr_diagnostics() || other.has_ppocr_diagnostics() {
            return self
                .with_complete_failure(RuntimeFailureRelation::DiagnosticArchive, other.clone());
        }
        self.lifecycle.diagnostics = self
            .lifecycle
            .diagnostics
            .with_related_stdio(other.diagnostics());
        if self.lifecycle.task_timing.is_none() {
            self.lifecycle.task_timing = other.lifecycle.task_timing.clone();
        }
        if self.lifecycle.capacity.is_none() {
            self.lifecycle.capacity = other.lifecycle.capacity.clone();
        }
        if self.code() == other.code()
            && self.operation() == other.operation()
            && self.diagnostics().native_detail() == other.diagnostics().native_detail()
        {
            return self;
        }
        let mut text = format!(
            "primary {} during {}; {relation} {} during {}",
            self.code(),
            self.operation(),
            other.code(),
            other.operation()
        );
        let details = [
            self.diagnostics().native_detail(),
            other.diagnostics().native_detail(),
        ];
        let count = details.iter().flatten().count();
        let budget = 1024usize.saturating_sub(text.len() + count * 3) / count.max(1);
        let mut truncated = false;
        for detail in details.into_iter().flatten() {
            let mut end = detail.text().len().min(budget);
            while !detail.text().is_char_boundary(end) {
                end -= 1;
            }
            truncated |= detail.truncated() || end < detail.text().len();
            text.push_str(" | ");
            text.push_str(&detail.text()[..end]);
        }
        self = self.with_native_detail(text);
        self.lifecycle.diagnostics = self.lifecycle.diagnostics.with_fresh_recording();
        if truncated && let Some(detail) = self.diagnostics().native_detail() {
            let detail = actingcommand_contract::LifecycleNativeDetail::new(detail.text(), true);
            self = self.with_native_failure_detail(detail);
        }
        self
    }
}

impl fmt::Debug for RuntimeHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeHostError")
            .field("code", &self.code())
            .field("operation", &self.operation())
            .field("fatal", &self.is_fatal())
            .field(
                "incomplete_device_diagnostic_summary",
                &self.lifecycle.incomplete_device_diagnostic_summary,
            )
            .field("owner_probe", &self.lifecycle.owner_probe)
            .field("latched_cause", &self.lifecycle.latched_cause)
            .finish()
    }
}

impl fmt::Display for RuntimeHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime host error {} during {}",
            self.code(),
            self.operation()
        )?;
        if let Some((code, operation)) = self.lifecycle.latched_cause {
            write!(formatter, " cause={code} cause_operation={operation}")?;
        }
        if let Some(probe) = &self.lifecycle.owner_probe {
            write!(formatter, ": {probe}")?;
        }
        if let Some((code, operation)) = self.lifecycle.incomplete_device_diagnostic_summary {
            write!(
                formatter,
                "; device diagnostic summary incomplete: {code} during {operation}"
            )?;
        }
        Ok(())
    }
}

impl Error for RuntimeHostError {}

/// Whether a code received as a spelling (one an error relays) is one of the given registered
/// codes. The relayed spellings become [`Code`]s when A2c and A2d type their owners' codes.
pub(crate) fn spelled(spelling: &str, codes: &[Code]) -> bool {
    codes.iter().any(|code| code.as_str() == spelling)
}

#[derive(Clone, Default)]
pub(crate) struct FatalState {
    inner: Arc<Mutex<Option<RuntimeHostError>>>,
    shutdown: Arc<AtomicBool>,
}

impl FatalState {
    pub(crate) fn mark(&self, error: RuntimeHostError) -> RuntimeHostResult<()> {
        if !error.is_fatal() {
            return Err(RuntimeHostError::fatal(
                HostCode::NonfatalErrorMarkedFatal,
                HostLocation::MarkRuntimeFatal,
                RuntimeErrorCode::RuntimeHalted,
            ));
        }
        let mut current = self.inner.lock().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::FatalStatePoisoned,
                HostLocation::MarkRuntimeFatal,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        if current.is_none() {
            *current = Some(error);
        }
        self.shutdown.store(true, Ordering::Release);
        Ok(())
    }

    /// The latched condition as the host answers a later request with it (Workflow #378 A6a,
    /// model section 2.3): [`RuntimeHostError::latched_answer`].
    pub(crate) fn answer(&self) -> RuntimeHostResult<Option<RuntimeHostError>> {
        Ok(self.current()?.map(RuntimeHostError::latched_answer))
    }

    pub(crate) fn current(&self) -> RuntimeHostResult<Option<RuntimeHostError>> {
        self.inner.lock().map(|value| value.clone()).map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::FatalStatePoisoned,
                HostLocation::ReadRuntimeFatal,
                RuntimeErrorCode::RuntimeHalted,
            )
        })
    }

    pub(crate) fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    pub(crate) fn is_shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }
}

/// A scheduler claim kind as a token of the catalog vocabulary `claim_kind` (the scheduler's
/// Q-2 kinds), so a record outcome can carry it as `holder_kind` (ledger revision 3).
#[derive(Clone, Copy)]
pub(crate) struct ClaimKindToken(pub(crate) actingcommand_scheduler::ClaimKind);

impl actingcommand_contract::outcome::VocabularyToken for ClaimKindToken {
    const VOCABULARY: &'static str = "claim_kind";

    fn token(self) -> &'static str {
        self.0.as_str()
    }
}
