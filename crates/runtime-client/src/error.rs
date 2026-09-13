// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CorrelationId, OwnerEpoch, RequestId, RuntimeErrorCode, RuntimeErrorProjection, RuntimeInfo,
    RuntimeReceipt, RuntimeRequest,
};
use std::error::Error;
use std::fmt;

pub type RuntimeClientResult<T> = Result<T, RuntimeClientError>;

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
    projection: Option<RuntimeErrorProjection>,
    related: Option<Box<RuntimeClientError>>,
    committed_receipt: Option<Box<RuntimeReceipt>>,
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

    pub const fn projection(&self) -> Option<&RuntimeErrorProjection> {
        self.projection.as_ref()
    }

    pub fn committed_receipt(&self) -> Option<&RuntimeReceipt> {
        self.committed_receipt.as_deref()
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
            receipt_header_io: None,
        }
    }

    pub(crate) const fn rejected(
        operation: &'static str,
        projection: RuntimeErrorProjection,
    ) -> Self {
        Self {
            code: "runtime_request_rejected",
            operation,
            projection: Some(projection),
            related: None,
            committed_receipt: None,
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
