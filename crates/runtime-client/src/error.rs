// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CorrelationId, OwnerEpoch, RequestId, RuntimeErrorCode, RuntimeErrorProjection, RuntimeInfo,
    RuntimeReceipt, RuntimeRequest,
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
const UNCERTAIN_AFTER_SEND_CODES: &[&str] = &[
    "runtime_receipt_header_failed",
    "runtime_receipt_read_failed",
    "runtime_receipt_frame_invalid",
    "runtime_receipt_decode_failed",
    "runtime_receipt_timeout",
    "runtime_read_timeout_restore_failed",
    "runtime_receipt_invalid",
    "runtime_receipt_identity_mismatch",
    "runtime_result_missing",
    "runtime_result_unexpected",
    "runtime_shutdown_receipt_unconfirmed",
    "material_read_receipt_selection_mismatch",
    "runtime_scheduling_resume_unconfirmed",
];

/// Client conclusions that settle a request whose receipt never arrived (Workflow #338 R4, R5):
/// the same owner, reached again, shows the request was not performed.
const SETTLED_WITHOUT_RECEIPT_CODES: &[&str] = &["runtime_operation_unsupported"];

/// The client codes of a contained run's own cancelled receipt (`run_contained_task`).
const CONTAINED_TASK_STOPPED_CODES: &[&str] = &[
    "runtime_contained_task_cancelled",
    "runtime_contained_task_paused",
    "runtime_contained_task_response_timeout",
];

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
        | RuntimeErrorCode::RuntimeFatal
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
}

/// Redacted local transport or typed Runtime rejection error.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeClientError {
    code: &'static str,
    operation: &'static str,
    projection: Option<Box<RuntimeErrorProjection>>,
    related: Option<Box<RuntimeClientError>>,
    committed_receipt: Option<Box<RuntimeReceipt>>,
    received_receipt: Option<Box<RuntimeReceipt>>,
    receipt_header_io: Option<Box<RuntimeReceiptHeaderIo>>,
}

impl RuntimeClientError {
    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn operation(&self) -> &'static str {
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
            None if CONTAINED_TASK_STOPPED_CODES.contains(&self.code) => {
                RuntimeClientErrorClass::Device
            }
            None => RuntimeClientErrorClass::Runtime,
        }
    }

    /// A committed terminal receipt, or a client conclusion that the request was not performed,
    /// settles its own request; a later or underlying related failure does not make that
    /// request's effect unknown.
    fn is_uncertain(&self) -> bool {
        if self.committed_receipt.is_some() || SETTLED_WITHOUT_RECEIPT_CODES.contains(&self.code) {
            return false;
        }
        let own = self.projection.is_none()
            && (UNCERTAIN_AFTER_SEND_CODES.contains(&self.code)
                || self.code.starts_with("runtime_contained_task_recovery_")
                || (self.code == "runtime_owner_epoch_changed"
                    && self.operation != "connect_runtime"));
        own || self
            .related
            .as_deref()
            .is_some_and(RuntimeClientError::is_uncertain)
    }

    /// The Runtime's closed failure code and operation, when its receipt carried both.
    pub fn host_failure(&self) -> Option<(&str, &str)> {
        let projection = self.projection.as_ref()?;
        Some((projection.host_code()?, projection.host_operation()?))
    }

    pub fn committed_receipt(&self) -> Option<&RuntimeReceipt> {
        self.committed_receipt.as_deref()
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

    pub(crate) const fn fatal(code: &'static str, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            projection: None,
            related: None,
            committed_receipt: None,
            received_receipt: None,
            receipt_header_io: None,
        }
    }

    pub(crate) fn rejected(operation: &'static str, projection: RuntimeErrorProjection) -> Self {
        Self {
            code: "runtime_request_rejected",
            operation,
            projection: Some(Box::new(projection)),
            related: None,
            committed_receipt: None,
            received_receipt: None,
            receipt_header_io: None,
        }
    }

    pub(crate) fn after_commit(
        code: &'static str,
        operation: &'static str,
        receipt: RuntimeReceipt,
        related: RuntimeClientError,
    ) -> Self {
        Self {
            code,
            operation,
            projection: None,
            related: Some(Box::new(related)),
            committed_receipt: Some(Box::new(receipt)),
            received_receipt: None,
            receipt_header_io: None,
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
                    "runtime client error {} during {} with runtime code {:?}",
                    self.code, self.operation, projection.code
                )?;
                if let Some(receipt) = self.received_receipt() {
                    write!(formatter, " state={:?}", receipt.state())?;
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
