// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{ClientCode, ClientLocation};
use actingcommand_contract::outcome::CodeStr;
use actingcommand_contract::outcome::{
    CauseRelation, Code, IoKind, IoOp, Location, Outcome, RawSource, VocabularyToken,
};
use actingcommand_contract::{
    CorrelationId, OwnerEpoch, PlayerHoldAttempt, PlayerHoldDecision, PlayerHoldNext,
    PlayerHoldSettlement, PlayerHoldSource, RequestId, RuntimeChannel, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeInfo, RuntimeReceipt, RuntimeRequest,
};
use serde::Serialize;
use std::error::Error;
use std::fmt;

pub type RuntimeClientResult<T> = Result<T, RuntimeClientError>;

/// The one result class of a failed client call (Workflow #338 R6), shared by every surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeClientErrorClass {
    /// The request or its arguments were refused.
    Usage,
    /// A lease, queue, connection, instance or busy rule refused the request.
    Safety,
    /// Capture, recognition, a backend, or the contained run itself failed or stopped.
    Device,
    /// The Runtime, its ledger, its protocol, or the local transport failed.
    Runtime,
    /// The request may have been sent and its effect is unknown: read the run before anything else.
    Uncertain,
}

impl RuntimeClientErrorClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Safety => "safety",
            Self::Device => "device",
            Self::Runtime => "runtime",
            Self::Uncertain => "uncertain",
        }
    }
}

/// Client failures raised after the request frame was written, without an error projection:
/// an unread, unconfirmed, or mismatched receipt.
const UNCERTAIN_AFTER_SEND_CODES: &[ClientCode] = &[
    ClientCode::RuntimeReceiptHeaderFailed,
    ClientCode::RuntimeReceiptReadFailed,
    ClientCode::RuntimeReceiptFrameInvalid,
    ClientCode::RuntimeReceiptDecodeFailed,
    ClientCode::RuntimeReceiptTimeout,
    ClientCode::RuntimeReadTimeoutRestoreFailed,
    ClientCode::RuntimeReceiptInvalid,
    ClientCode::RuntimeReceiptIdentityMismatch,
    ClientCode::RuntimeResultMissing,
    ClientCode::RuntimeResultUnexpected,
    ClientCode::RuntimeShutdownReceiptUnconfirmed,
    ClientCode::InstallTransitionReceiptUnconfirmed,
    ClientCode::MaterialReadReceiptSelectionMismatch,
    ClientCode::RuntimeSchedulingResumeUnconfirmed,
    // Workflow #429 R-7a: a player hold request whose effect `Status` could not confirm.
    ClientCode::RuntimePlayerHoldUnconfirmed,
];

/// Client conclusions that settle a request whose receipt never arrived (Workflow #338 R4, R5;
/// Workflow #429 R-7a): the same owner, reached again, shows the request's effect is known: it
/// was not performed (`runtime_operation_unsupported`), or the player hold settlement read its
/// final state (`runtime_player_hold_settled`, which may be `completed`).
const SETTLED_WITHOUT_RECEIPT_CODES: &[ClientCode] = &[
    ClientCode::RuntimeOperationUnsupported,
    ClientCode::RuntimePlayerHoldSettled,
];

/// The client codes of a contained run's own cancelled receipt (`run_contained_task`).
const CONTAINED_TASK_STOPPED_CODES: &[ClientCode] = &[
    ClientCode::RuntimeContainedTaskCancelled,
    ClientCode::RuntimeContainedTaskPaused,
    ClientCode::RuntimeContainedTaskResponseTimeout,
];

/// Whether `code` is one of `codes`.
fn listed(codes: &[ClientCode], code: Code) -> bool {
    codes.iter().any(|listed| listed.code() == code)
}

const fn runtime_error_class(code: RuntimeErrorCode) -> RuntimeClientErrorClass {
    use RuntimeClientErrorClass::{Device, Runtime, Safety, Usage};
    match code {
        RuntimeErrorCode::InvalidRequest
        | RuntimeErrorCode::InstanceUnknown
        | RuntimeErrorCode::PackageInvalid
        | RuntimeErrorCode::ReadonlyCapabilityInvalid => Usage,
        RuntimeErrorCode::LeaseBusy
        | RuntimeErrorCode::LeaseCooldown
        | RuntimeErrorCode::LeaseExpired
        | RuntimeErrorCode::LeaseMissing
        | RuntimeErrorCode::LeaseMismatch
        | RuntimeErrorCode::HolderMismatch
        | RuntimeErrorCode::ConnectionMismatch
        | RuntimeErrorCode::InstanceMismatch
        | RuntimeErrorCode::QueueFull
        | RuntimeErrorCode::QueueExpired
        | RuntimeErrorCode::QueueMissing
        | RuntimeErrorCode::QueueConnectionMismatch
        | RuntimeErrorCode::TransferNotSafe
        | RuntimeErrorCode::ContainedTaskBusy
        | RuntimeErrorCode::RuntimeBusy => Safety,
        RuntimeErrorCode::CaptureFailed
        | RuntimeErrorCode::RecognitionFailed
        | RuntimeErrorCode::BackendOpenFailed
        | RuntimeErrorCode::BackendOperationFailed
        | RuntimeErrorCode::ContainedTaskDeadlineExceeded
        | RuntimeErrorCode::ContainedTaskCancelled
        | RuntimeErrorCode::ContainedTaskPaused => Device,
        RuntimeErrorCode::RuntimeUnavailable
        | RuntimeErrorCode::RuntimeHalted
        | RuntimeErrorCode::OwnerConflict
        | RuntimeErrorCode::StaleOwnerEpoch
        | RuntimeErrorCode::LedgerFailure
        | RuntimeErrorCode::RuntimeOwnerMismatch
        | RuntimeErrorCode::ProtocolInvalid
        | RuntimeErrorCode::EvidenceExportFailed => Runtime,
    }
}

/// Direct I/O facts from the failed four-byte receipt-header read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReceiptHeaderIo {
    kind: std::io::ErrorKind,
    raw_os_error: Option<i32>,
    message: String,
    message_truncated: bool,
    request_id: Option<RequestId>,
    correlation_id: Option<CorrelationId>,
    expected_owner_epoch: Option<OwnerEpoch>,
    expected_runtime_pid: Option<u32>,
}

impl RuntimeReceiptHeaderIo {
    pub const fn kind(&self) -> std::io::ErrorKind {
        self.kind
    }
    pub const fn raw_os_error(&self) -> Option<i32> {
        self.raw_os_error
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub const fn message_truncated(&self) -> bool {
        self.message_truncated
    }
    pub fn request_id(&self) -> Option<&RequestId> {
        self.request_id.as_ref()
    }
    pub fn correlation_id(&self) -> Option<&CorrelationId> {
        self.correlation_id.as_ref()
    }
    pub fn expected_owner_epoch(&self) -> Option<&OwnerEpoch> {
        self.expected_owner_epoch.as_ref()
    }
    pub const fn expected_runtime_pid(&self) -> Option<u32> {
        self.expected_runtime_pid
    }

    /// The failed read as a `foreign_os_error` link (Workflow #378 A4): the I/O kind and the OS
    /// error number with the system's text, as far as the client kept it.
    fn outcome(&self) -> Outcome {
        let mut outcome = Outcome::new(actingcommand_contract::codes::ContractCode::ForeignOsError)
            .raw_source(RawSource::Os)
            .io_kind(IoKind::from_error_kind(self.kind))
            .io_op(IoOp::Read);
        if let Some(os_error) = self.raw_os_error {
            outcome = outcome.os_error(os_error);
        }
        outcome = outcome.raw_text(&self.message);
        if self.message_truncated {
            outcome = outcome.raw_text_truncated(true);
        }
        outcome
    }
}

/// The outcome key of the location a request failed at.
const OPERATION_KEY: &str = "operation";

/// Redacted local transport or typed Runtime rejection error.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeClientError {
    code: Code,
    operation: Location,
    projection: Option<Box<RuntimeErrorProjection>>,
    /// Workflow #378 A4: the outcome of the denied or failed receipt behind `projection`.
    receipt_outcome: Option<Box<Outcome>>,
    related: Option<Box<RuntimeClientError>>,
    committed_receipt: Option<Box<RuntimeReceipt>>,
    received_receipt: Option<Box<RuntimeReceipt>>,
    receipt_header_io: Option<Box<RuntimeReceiptHeaderIo>>,
    /// Workflow #429 R-7a: the settlement of a player hold request whose receipt never arrived,
    /// on the three settlement codes (`runtime_player_hold_settled`,
    /// `runtime_player_hold_unconfirmed`, `runtime_operation_unsupported` in rows A7 and R8).
    player_hold: Option<Box<(PlayerHoldSettlement, PlayerHoldAttempt)>>,
}

impl RuntimeClientError {
    pub const fn code(&self) -> Code {
        self.code
    }

    pub const fn operation(&self) -> Location {
        self.operation
    }

    pub fn is_fatal(&self) -> bool {
        self.projection.as_ref().is_none_or(|value| value.fatal)
    }

    pub fn is_fallback_eligible(&self) -> bool {
        self.projection.as_ref().is_some_and(|value| {
            !value.fatal
                && matches!(
                    value.code,
                    RuntimeErrorCode::LeaseBusy
                        | RuntimeErrorCode::LeaseCooldown
                        | RuntimeErrorCode::BackendOpenFailed
                        | RuntimeErrorCode::BackendOperationFailed
                )
        })
    }

    pub fn projection(&self) -> Option<&RuntimeErrorProjection> {
        self.projection.as_deref()
    }

    /// The result class of this failure (Workflow #338 R6). A failure after the request was sent
    /// without an error projection anywhere in its chain (an unread or unconfirmed receipt, a
    /// connection latched by it, an owner epoch change, `runtime_contained_task_recovery_*`) is
    /// `Uncertain`; otherwise the Runtime's error code decides, and a client failure without a
    /// projection is `Runtime`, except the contained run's own stopped receipt (`Device`).
    pub fn disposition(&self) -> RuntimeClientErrorClass {
        if self.is_uncertain() {
            return RuntimeClientErrorClass::Uncertain;
        }
        match self.projection.as_deref() {
            Some(projection) => runtime_error_class(projection.code),
            None if listed(CONTAINED_TASK_STOPPED_CODES, self.code) => {
                RuntimeClientErrorClass::Device
            }
            None => RuntimeClientErrorClass::Runtime,
        }
    }

    /// A committed terminal receipt, or a client conclusion that the request was not performed,
    /// settles its own request; a later or underlying related failure does not make that
    /// request's effect unknown.
    fn is_uncertain(&self) -> bool {
        if self.committed_receipt.is_some() || listed(SETTLED_WITHOUT_RECEIPT_CODES, self.code) {
            return false;
        }
        let own = self.projection.is_none()
            && (listed(UNCERTAIN_AFTER_SEND_CODES, self.code)
                || self
                    .code
                    .as_str()
                    .starts_with("runtime_contained_task_recovery_")
                || (self.code == ClientCode::RuntimeOwnerEpochChanged.code()
                    && self.operation != ClientLocation::ConnectRuntime.location()));
        own || self
            .related
            .as_deref()
            .is_some_and(RuntimeClientError::is_uncertain)
    }

    /// The Runtime's closed failure code and operation, when its receipt carried both: the
    /// receipt outcome's top code and its `operation` value (Workflow #378 A4).
    pub fn host_failure(&self) -> Option<(&str, &str)> {
        let outcome = self.receipt_outcome.as_deref()?;
        let operation = outcome.value(OPERATION_KEY)?.as_str()?;
        Some((outcome.code().as_str(), operation))
    }

    /// The outcome of the denied or failed receipt this error carries, as the Runtime wrote it.
    pub fn receipt_outcome(&self) -> Option<&Outcome> {
        self.receipt_outcome.as_deref()
    }

    /// This failure as one outcome (Workflow #378 A4, model section 7.2): a denied or failed
    /// receipt's outcome on top as the Runtime wrote it, else the client's own code with its
    /// `operation`. The client's relations become links: a related failure is `related`, or
    /// `after_commit` once the request's terminal receipt was committed; the failed receipt-header
    /// read is a `foreign_os_error` `caused_by` link.
    pub fn outcome(&self) -> Outcome {
        let mut outcome = match self.receipt_outcome.as_deref() {
            Some(receipt) => receipt.clone(),
            None => Outcome::new(self.code).operation(self.operation),
        };
        // Workflow #429 R-7a: a player hold settlement carries its decision as values (the
        // typed whole is `player_hold_settlement`). `runtime_operation_unsupported` keeps its
        // registration: in rows A7 and R8 the settlement is only typed.
        if let Some(settled) = self.player_hold.as_deref()
            && let Some(decision) = settled.0.decision()
            && listed(
                &[
                    ClientCode::RuntimePlayerHoldSettled,
                    ClientCode::RuntimePlayerHoldUnconfirmed,
                ],
                self.code,
            )
        {
            outcome = settlement_values(outcome, decision, &settled.1);
        }
        if let Some(header) = self.receipt_header_io.as_deref() {
            outcome = outcome.caused_by(header.outcome());
        }
        if let Some(related) = self.related.as_deref() {
            let relation = if self.committed_receipt.is_some() {
                CauseRelation::AfterCommit
            } else {
                CauseRelation::Related
            };
            outcome = outcome.link(relation, related.outcome());
        }
        outcome
    }

    pub fn committed_receipt(&self) -> Option<&RuntimeReceipt> {
        self.committed_receipt.as_deref()
    }

    /// Workflow #429 R-7a (§3.3.2): the typed settlement of a player hold request whose receipt
    /// never arrived; present on the three settlement codes, `None` on every other error. Read
    /// this rather than the class's general wording: a settled `completed` is not a Runtime
    /// failure although its `disposition()` is `runtime`.
    pub fn player_hold_settlement(&self) -> Option<&PlayerHoldSettlement> {
        self.player_hold.as_deref().map(|settled| &settled.0)
    }

    /// Workflow #429 R-7a: the attempt the settlement judged, beside it.
    pub fn player_hold_attempt(&self) -> Option<&PlayerHoldAttempt> {
        self.player_hold.as_deref().map(|settled| &settled.1)
    }

    pub(crate) fn with_player_hold(
        mut self,
        settlement: PlayerHoldSettlement,
        attempt: PlayerHoldAttempt,
    ) -> Self {
        self.player_hold = Some(Box::new((settlement, attempt)));
        self
    }

    /// A validated reply actually received from this Runtime, including refusals without a terminal.
    /// Receiving a reply alone does not establish Ledger commitment or execution.
    pub fn received_receipt(&self) -> Option<&RuntimeReceipt> {
        self.received_receipt
            .as_deref()
            .or_else(|| self.committed_receipt())
    }

    pub fn receipt_header_io(&self) -> Option<&RuntimeReceiptHeaderIo> {
        self.receipt_header_io.as_deref()
    }

    pub(crate) fn with_receipt_header_io(mut self, error: &std::io::Error) -> Self {
        const MAX_IO_MESSAGE_CHARS: usize = 256;
        let message = error.to_string();
        let mut chars = message.chars();
        let bounded = chars.by_ref().take(MAX_IO_MESSAGE_CHARS).collect();
        let message_truncated = chars.next().is_some();
        self.receipt_header_io = Some(Box::new(RuntimeReceiptHeaderIo {
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
            message: bounded,
            message_truncated,
            request_id: None,
            correlation_id: None,
            expected_owner_epoch: None,
            expected_runtime_pid: None,
        }));
        self
    }

    pub(crate) fn with_receipt_header_context(
        mut self,
        request: &RuntimeRequest,
        info: &RuntimeInfo,
    ) -> Self {
        if let Some(context) = self.receipt_header_io.as_mut() {
            context.request_id = Some(request.request_id());
            context.correlation_id = Some(request.correlation_id());
            context.expected_owner_epoch = Some(info.owner_epoch());
            context.expected_runtime_pid = Some(info.pid());
        }
        self
    }

    pub(crate) fn fatal(code: impl Into<Code>, operation: impl Into<Location>) -> Self {
        Self {
            code: code.into(),
            operation: operation.into(),
            projection: None,
            receipt_outcome: None,
            related: None,
            committed_receipt: None,
            received_receipt: None,
            receipt_header_io: None,
            player_hold: None,
        }
    }

    /// A denied or failed receipt: its projection and the outcome it carried (Workflow #378 A4).
    pub(crate) fn rejected(
        operation: impl Into<Location>,
        projection: RuntimeErrorProjection,
        receipt_outcome: Outcome,
    ) -> Self {
        Self {
            code: ClientCode::RuntimeRequestRejected.code(),
            operation: operation.into(),
            projection: Some(Box::new(projection)),
            receipt_outcome: Some(Box::new(receipt_outcome)),
            related: None,
            committed_receipt: None,
            received_receipt: None,
            receipt_header_io: None,
            player_hold: None,
        }
    }

    pub(crate) fn after_commit(
        code: impl Into<Code>,
        operation: impl Into<Location>,
        receipt: RuntimeReceipt,
        related: RuntimeClientError,
    ) -> Self {
        Self {
            code: code.into(),
            operation: operation.into(),
            projection: None,
            receipt_outcome: None,
            related: Some(Box::new(related)),
            committed_receipt: Some(Box::new(receipt)),
            received_receipt: None,
            receipt_header_io: None,
            player_hold: None,
        }
    }

    pub(crate) fn with_related(mut self, related: RuntimeClientError) -> Self {
        match self.related.take() {
            Some(existing) => self.related = Some(Box::new(existing.with_related(related))),
            None => self.related = Some(Box::new(related)),
        }
        self
    }

    pub(crate) fn with_committed_receipt(mut self, receipt: RuntimeReceipt) -> Self {
        self.committed_receipt = Some(Box::new(receipt));
        self
    }

    pub(crate) fn with_received_receipt(mut self, receipt: RuntimeReceipt) -> Self {
        self.received_receipt = Some(Box::new(receipt));
        self
    }
}

/// Workflow #429 R-7a (§6.1): a player hold settlement's values on its code: the attempt, the
/// row and its verdict, the status epoch, the hold now, the own record's result (rows A1-A3,
/// R1-R3) or a summary of the record now, and who ended the hold. Codes mode keeps the
/// vocabulary values (`settlement_outcome`, `settlement_observed`, the rule, the phases) and
/// drops the rest, as everywhere.
fn settlement_values(
    mut outcome: Outcome,
    decision: &PlayerHoldDecision,
    attempt: &PlayerHoldAttempt,
) -> Outcome {
    use actingcommand_contract::PlayerHoldSettlementRule as Rule;
    outcome = outcome
        .operation_kind(attempt.kind)
        .settlement_rule(decision.rule)
        .settlement_trigger(attempt.trigger)
        .settlement_final(decision.is_final);
    if let Some(request_id) = id_text(&attempt.request_id) {
        outcome = outcome.request_id(request_id);
    }
    if let Some(settled) = decision.settlement_outcome {
        outcome = outcome.settlement_outcome(settled);
    }
    if let Some(observed) = decision.observed {
        outcome = outcome.settlement_observed(observed);
    }
    if let Some(epoch) = decision.status_owner_epoch.as_ref().and_then(id_text) {
        outcome = outcome.owner_epoch(epoch);
    }
    if decision.restored {
        outcome = outcome.hold_restored(true);
    }
    if let Some(hold) = &decision.hold {
        if let Some(hold_id) = id_text(&hold.hold_id) {
            outcome = outcome.hold_id(hold_id);
        }
        outcome = outcome.hold_phase(hold.phase).hold_revision(hold.revision);
        if let Some(from) = hold.restored_from_phase {
            outcome = outcome.restored_from_phase(from);
        }
        outcome = source_values(outcome, &hold.source);
    }
    let own = matches!(
        decision.rule,
        Rule::A1 | Rule::A2 | Rule::A3 | Rule::R1 | Rule::R2 | Rule::R3
    );
    if let Some(record) = &decision.operation {
        if own {
            if let Some(code) = record.code.as_ref().and_then(CodeStr::registered) {
                outcome = outcome.failure_code(code);
            }
            if let Some(phase) = record.failed_phase {
                outcome = outcome.failed_phase(phase);
            }
            if let Some(result) = &record.result {
                if let Some(revision) = result.revision {
                    outcome = outcome.hold_revision(revision);
                }
                if let Some(drained) = result.drained {
                    outcome = outcome
                        .drained_finished(drained.finished)
                        .drained_cancelled(drained.cancelled);
                }
                if let Some(next) = result.next {
                    outcome = outcome.hold_next(next);
                }
                if let Some(startup) = result.startup_package {
                    outcome = outcome.startup_package(startup);
                }
                if let Some(withheld) = result.startup_package_withheld {
                    outcome =
                        outcome.startup_package_withheld(PlayerHoldNext::of_withheld(withheld));
                }
                if let Some(pauses) = result.pauses_at_end {
                    if let Some(revision) = pauses.instance_revision {
                        outcome = outcome.pause_instance_revision(revision);
                    }
                    if let Some(revision) = pauses.global_revision {
                        outcome = outcome.pause_global_revision(revision);
                    }
                }
                if let Some(code) = result
                    .selfcheck
                    .as_ref()
                    .and_then(|selfcheck| selfcheck.failure_code.clone())
                    .and_then(|code| serde_json::from_value::<CodeStr>(code.into()).ok())
                    .and_then(|code| code.registered())
                {
                    outcome = outcome.selfcheck_failure_code(code);
                }
            }
        } else {
            outcome = outcome
                .operation_kind_now(record.kind)
                .operation_state_now(record.state);
        }
    }
    if let Some(ended_by) = &decision.ended_by {
        outcome = outcome
            .ended_by_actor(ended_by.source.actor)
            .ended_by_channel(ended_by.source.channel)
            .ended_by_state(ended_by.state);
        if let Some(via) = ended_by.source.via {
            outcome = outcome.ended_by_via(via_channel(via));
        }
    }
    outcome
}

fn source_values(mut outcome: Outcome, source: &PlayerHoldSource) -> Outcome {
    outcome = outcome
        .source_actor(source.actor)
        .source_channel(source.channel);
    if let Some(via) = source.via {
        outcome = outcome.source_via(via_channel(via));
    }
    outcome
}

/// A `via` as a token of the `channel` vocabulary (its two values are two channels).
const fn via_channel(via: actingcommand_contract::ChannelVia) -> RuntimeChannel {
    match via {
        actingcommand_contract::ChannelVia::Mcp => RuntimeChannel::Mcp,
        actingcommand_contract::ChannelVia::Ui => RuntimeChannel::Ui,
    }
}

/// An identifier's canonical spelling, as it travels.
fn id_text<T: Serialize>(id: &T) -> Option<String> {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => Some(text),
        _ => None,
    }
}

impl fmt::Debug for RuntimeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeClientError")
            .field("code", &self.code)
            .field("operation", &self.operation)
            .field("fatal", &self.is_fatal())
            .field(
                "runtime_code",
                &self.projection.as_ref().map(|value| value.code),
            )
            .field("related", &self.related)
            .field("receipt_header_io", &self.receipt_header_io)
            .field("committed_receipt", &self.committed_receipt.is_some())
            .field("received_receipt", &self.received_receipt().is_some())
            .finish()
    }
}

impl fmt::Display for RuntimeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.projection {
            Some(projection) => {
                write!(
                    formatter,
                    "runtime client error {} during {} with runtime code {}",
                    self.code,
                    self.operation,
                    projection.code.as_str()
                )?;
                if let Some(receipt) = self.received_receipt() {
                    write!(formatter, " state={}", receipt.state().token())?;
                }
                write!(formatter, " fatal={}", projection.fatal)?;
                if let Some(retry_after_ms) = projection.retry_after_ms {
                    write!(formatter, " retry_after_ms={retry_after_ms}")?;
                }
                if let Some((code, operation)) = self.host_failure() {
                    write!(formatter, " host code {code} during {operation}")?;
                }
                if let Some(related) = &self.related {
                    write!(formatter, "; related failure: {related}")?;
                }
                Ok(())
            }
            None => match (&self.committed_receipt, &self.related) {
                (Some(_), Some(related)) => write!(
                    formatter,
                    "runtime client error {} during {}; terminal receipt was committed before related failure: {}",
                    self.code, self.operation, related
                ),
                (None, Some(related)) => write!(
                    formatter,
                    "runtime client error {} during {}; related failure: {}",
                    self.code, self.operation, related
                ),
                _ => write!(
                    formatter,
                    "runtime client error {} during {}",
                    self.code, self.operation
                ),
            },
        }?;
        if let Some(context) = &self.receipt_header_io {
            write!(formatter, "; receipt_header_io={context:?}")?;
        }
        Ok(())
    }
}

impl Error for RuntimeClientError {}
