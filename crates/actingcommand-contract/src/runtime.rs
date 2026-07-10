// SPDX-License-Identifier: AGPL-3.0-only

//! Typed local Runtime IPC contract shared by resident hosts and disposable clients.

use crate::{
    ActionId, CausationId, CorrelationId, EffectDisposition, EventActor, EventId, EventLinksDraft,
    EventQuery, EventSource, HolderId, InstanceId, IssuedCausationId, IssuedCorrelationId,
    IssuedHolderId, IssuedRequestId, LeaseId, OwnerEpoch, ProjectedEvent, ProjectionProfile,
    RequestId,
};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, SocketAddr};

pub const RUNTIME_REQUEST_SCHEMA_VERSION: &str = "actingcommand.runtime.request.v1";
pub const RUNTIME_RECEIPT_SCHEMA_VERSION: &str = "actingcommand.runtime.receipt.v1";
pub const RUNTIME_INFO_SCHEMA_VERSION: &str = "actingcommand.runtime.info.v1";
pub const RUNTIME_INFO_FILE: &str = "runtime-info.json";
pub const MAX_INSTANCE_ALIAS_BYTES: usize = 256;
pub const MAX_INPUT_TEXT_BYTES: usize = 4096;
pub const MAX_INPUT_KEY_BYTES: usize = 64;
pub const MAX_INPUT_DURATION_MS: u64 = 60_000;

pub type RuntimeContractResult<T> = Result<T, RuntimeContractError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeContractError {
    code: &'static str,
}

impl RuntimeContractError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(self) -> &'static str {
        self.code
    }
}

impl fmt::Display for RuntimeContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime contract validation failed with {}",
            self.code
        )
    }
}

impl Error for RuntimeContractError {}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputAction {
    Tap {
        x: i32,
        y: i32,
    },
    LongTap {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
    Swipe {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    },
    Key {
        key: String,
    },
    Text {
        text: String,
    },
    Reset,
}

impl InputAction {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Tap { x, y } => validate_point(*x, *y),
            Self::LongTap { x, y, duration_ms } => {
                validate_point(*x, *y)?;
                validate_duration(*duration_ms)
            }
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => {
                validate_point(*x1, *y1)?;
                validate_point(*x2, *y2)?;
                validate_duration(*duration_ms)
            }
            Self::Key { key } => validate_bounded_text(key, MAX_INPUT_KEY_BYTES, "invalid_key"),
            Self::Text { text } => {
                validate_bounded_text(text, MAX_INPUT_TEXT_BYTES, "invalid_input_text")
            }
            Self::Reset => Ok(()),
        }
    }

    pub const fn effect(&self) -> EffectDisposition {
        EffectDisposition::Performed
    }

    pub const fn event_action(&self) -> crate::EventAction {
        match self {
            Self::Tap { .. } => crate::EventAction::InputTap,
            Self::LongTap { .. } => crate::EventAction::InputLongTap,
            Self::Swipe { .. } => crate::EventAction::InputSwipe,
            Self::Key { .. } => crate::EventAction::InputKey,
            Self::Text { .. } => crate::EventAction::InputText,
            Self::Reset => crate::EventAction::InputReset,
        }
    }
}

impl fmt::Debug for InputAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Tap { .. } => "InputAction::Tap(<redacted-coordinates>)",
            Self::LongTap { .. } => "InputAction::LongTap(<redacted-coordinates>)",
            Self::Swipe { .. } => "InputAction::Swipe(<redacted-coordinates>)",
            Self::Key { .. } => "InputAction::Key(<redacted-key>)",
            Self::Text { .. } => "InputAction::Text(<redacted-text>)",
            Self::Reset => "InputAction::Reset",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseToken {
    owner_epoch: OwnerEpoch,
    lease_id: LeaseId,
    instance_id: InstanceId,
    holder_id: HolderId,
    expires_at_monotonic_ms: u64,
}

impl LeaseToken {
    pub fn new(
        owner_epoch: OwnerEpoch,
        lease_id: LeaseId,
        instance_id: InstanceId,
        holder_id: HolderId,
        expires_at_monotonic_ms: u64,
    ) -> RuntimeContractResult<Self> {
        let token = Self {
            owner_epoch,
            lease_id,
            instance_id,
            holder_id,
            expires_at_monotonic_ms,
        };
        token.validate()?;
        Ok(token)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.expires_at_monotonic_ms == 0 {
            return Err(RuntimeContractError::new("invalid_lease_expiry"));
        }
        Ok(())
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub const fn lease_id(&self) -> LeaseId {
        self.lease_id
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn holder_id(&self) -> HolderId {
        self.holder_id
    }

    pub const fn expires_at_monotonic_ms(&self) -> u64 {
        self.expires_at_monotonic_ms
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeOperation {
    Health,
    AcquireLease {
        instance_alias: String,
        holder_id: HolderId,
    },
    RenewLease {
        token: LeaseToken,
    },
    ReleaseLease {
        token: LeaseToken,
    },
    AdmitReadonly {
        instance_alias: String,
    },
    Input {
        token: LeaseToken,
        action: InputAction,
    },
    QueryEvents {
        query: EventQuery,
        profile: ProjectionProfile,
    },
}

impl RuntimeOperation {
    pub fn acquire_lease(instance_alias: impl Into<String>, holder_id: IssuedHolderId) -> Self {
        Self::AcquireLease {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
        }
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Health | Self::QueryEvents { .. } => Ok(()),
            Self::AcquireLease { instance_alias, .. } | Self::AdmitReadonly { instance_alias } => {
                validate_instance_alias(instance_alias)
            }
            Self::RenewLease { token } | Self::ReleaseLease { token } => token.validate(),
            Self::Input { token, action } => {
                token.validate()?;
                action.validate()
            }
        }
    }

    pub fn instance_alias(&self) -> Option<&str> {
        match self {
            Self::AcquireLease { instance_alias, .. } | Self::AdmitReadonly { instance_alias } => {
                Some(instance_alias)
            }
            _ => None,
        }
    }

    pub const fn lease_token(&self) -> Option<&LeaseToken> {
        match self {
            Self::RenewLease { token }
            | Self::ReleaseLease { token }
            | Self::Input { token, .. } => Some(token),
            _ => None,
        }
    }
}

impl fmt::Debug for RuntimeOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Health => "RuntimeOperation::Health",
            Self::AcquireLease { .. } => "RuntimeOperation::AcquireLease(<redacted>)",
            Self::RenewLease { .. } => "RuntimeOperation::RenewLease(<opaque-token>)",
            Self::ReleaseLease { .. } => "RuntimeOperation::ReleaseLease(<opaque-token>)",
            Self::AdmitReadonly { .. } => "RuntimeOperation::AdmitReadonly(<redacted>)",
            Self::Input { .. } => "RuntimeOperation::Input(<redacted>)",
            Self::QueryEvents { .. } => "RuntimeOperation::QueryEvents(<typed-query>)",
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRequest {
    schema_version: String,
    request_id: RequestId,
    correlation_id: CorrelationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    causation_id: Option<CausationId>,
    actor: EventActor,
    source: EventSource,
    submitted_at_unix_ms: u64,
    operation: RuntimeOperation,
}

impl RuntimeRequest {
    pub fn new(
        request_id: IssuedRequestId,
        correlation_id: IssuedCorrelationId,
        causation_id: Option<IssuedCausationId>,
        actor: EventActor,
        source: EventSource,
        submitted_at_unix_ms: u64,
        operation: RuntimeOperation,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            schema_version: RUNTIME_REQUEST_SCHEMA_VERSION.to_string(),
            request_id: *request_id.transport(),
            correlation_id: *correlation_id.transport(),
            causation_id: causation_id.map(|value| *value.transport()),
            actor,
            source,
            submitted_at_unix_ms,
            operation,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<ValidatedRuntimeRequest<'_>> {
        if self.schema_version != RUNTIME_REQUEST_SCHEMA_VERSION {
            return Err(RuntimeContractError::new("unsupported_request_schema"));
        }
        if self.submitted_at_unix_ms == 0 {
            return Err(RuntimeContractError::new("invalid_request_timestamp"));
        }
        if !valid_client_origin(self.actor, self.source) {
            return Err(RuntimeContractError::new("invalid_client_origin"));
        }
        self.operation.validate()?;
        Ok(ValidatedRuntimeRequest { request: self })
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn actor(&self) -> EventActor {
        self.actor
    }

    pub const fn source(&self) -> EventSource {
        self.source
    }

    pub const fn submitted_at_unix_ms(&self) -> u64 {
        self.submitted_at_unix_ms
    }

    pub const fn operation(&self) -> &RuntimeOperation {
        &self.operation
    }
}

impl fmt::Debug for RuntimeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeRequest")
            .field("schema_version", &self.schema_version)
            .field("request_id", &self.request_id)
            .field("correlation_id", &self.correlation_id)
            .field("actor", &self.actor)
            .field("source", &self.source)
            .field("submitted_at_unix_ms", &self.submitted_at_unix_ms)
            .field("operation", &self.operation)
            .finish()
    }
}

#[derive(Debug)]
pub struct ValidatedRuntimeRequest<'a> {
    request: &'a RuntimeRequest,
}

impl ValidatedRuntimeRequest<'_> {
    pub fn event_links(
        &self,
        instance_id: Option<InstanceId>,
        lease_id: Option<LeaseId>,
        action_id: Option<ActionId>,
    ) -> EventLinksDraft {
        EventLinksDraft::from_verified_runtime(
            instance_id,
            self.request.request_id,
            self.request.correlation_id,
            self.request.causation_id,
            lease_id,
            action_id,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeReceiptState {
    Admitted,
    Denied,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalEvent {
    pub sequence: u64,
    pub event_id: EventId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeErrorCode {
    InvalidRequest,
    RuntimeUnavailable,
    RuntimeFatal,
    OwnerConflict,
    ProtocolInvalid,
    InstanceUnknown,
    LeaseBusy,
    LeaseCooldown,
    LeaseExpired,
    LeaseMissing,
    StaleOwnerEpoch,
    LeaseMismatch,
    InstanceMismatch,
    HolderMismatch,
    ConnectionMismatch,
    BackendOpenFailed,
    BackendOperationFailed,
    LedgerFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeErrorProjection {
    pub code: RuntimeErrorCode,
    pub fatal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_holder_id: Option<HolderId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_lease_id: Option<LeaseId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl RuntimeErrorProjection {
    pub const fn new(code: RuntimeErrorCode, fatal: bool) -> Self {
        Self {
            code,
            fatal,
            current_holder_id: None,
            current_lease_id: None,
            retry_after_ms: None,
        }
    }

    pub const fn with_holder(mut self, holder_id: HolderId, lease_id: LeaseId) -> Self {
        self.current_holder_id = Some(holder_id);
        self.current_lease_id = Some(lease_id);
        self
    }

    pub const fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyCaptureCapability {
    instance_id: InstanceId,
}

impl ReadOnlyCaptureCapability {
    pub const fn new(instance_id: InstanceId) -> Self {
        Self { instance_id }
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeResult {
    Health {
        owner_epoch: OwnerEpoch,
    },
    LeaseGranted {
        token: LeaseToken,
    },
    LeaseRenewed {
        token: LeaseToken,
    },
    LeaseReleased {
        instance_id: InstanceId,
        lease_id: LeaseId,
    },
    ReadOnlyAdmitted {
        capability: ReadOnlyCaptureCapability,
    },
    InputCommitted {
        action_id: ActionId,
    },
    Events {
        events: Vec<ProjectedEvent>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReceipt {
    schema_version: String,
    request_id: RequestId,
    correlation_id: CorrelationId,
    state: RuntimeReceiptState,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<TerminalEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<RuntimeResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RuntimeErrorProjection>,
}

impl RuntimeReceipt {
    pub fn success(
        request: &RuntimeRequest,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
        result: RuntimeResult,
    ) -> RuntimeContractResult<Self> {
        let receipt = Self {
            schema_version: RUNTIME_RECEIPT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id,
            correlation_id: request.correlation_id,
            state,
            terminal,
            result: Some(result),
            error: None,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn error(
        request: &RuntimeRequest,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
        error: RuntimeErrorProjection,
    ) -> RuntimeContractResult<Self> {
        let receipt = Self {
            schema_version: RUNTIME_RECEIPT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id,
            correlation_id: request.correlation_id,
            state,
            terminal,
            result: None,
            error: Some(error),
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != RUNTIME_RECEIPT_SCHEMA_VERSION {
            return Err(RuntimeContractError::new("unsupported_receipt_schema"));
        }
        let success_state = matches!(
            self.state,
            RuntimeReceiptState::Admitted | RuntimeReceiptState::Completed
        );
        if success_state != (self.result.is_some() && self.error.is_none()) {
            return Err(RuntimeContractError::new("invalid_receipt_outcome"));
        }
        if !success_state && (self.error.is_none() || self.result.is_some()) {
            return Err(RuntimeContractError::new("invalid_receipt_outcome"));
        }
        if self.terminal.is_some_and(|terminal| terminal.sequence == 0) {
            return Err(RuntimeContractError::new("invalid_terminal_event"));
        }
        if let Some(RuntimeResult::LeaseGranted { token } | RuntimeResult::LeaseRenewed { token }) =
            &self.result
        {
            token.validate()?;
        }
        Ok(())
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn state(&self) -> RuntimeReceiptState {
        self.state
    }

    pub const fn terminal(&self) -> Option<TerminalEvent> {
        self.terminal
    }

    pub const fn result(&self) -> Option<&RuntimeResult> {
        self.result.as_ref()
    }

    pub const fn error_projection(&self) -> Option<&RuntimeErrorProjection> {
        self.error.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInfo {
    schema_version: String,
    pid: u32,
    host: String,
    port: u16,
    owner_epoch: OwnerEpoch,
    started_at_unix_ms: u64,
}

impl RuntimeInfo {
    pub fn new(
        pid: u32,
        host: impl Into<String>,
        port: u16,
        owner_epoch: OwnerEpoch,
        started_at_unix_ms: u64,
    ) -> RuntimeContractResult<Self> {
        let info = Self {
            schema_version: RUNTIME_INFO_SCHEMA_VERSION.to_string(),
            pid,
            host: host.into(),
            port,
            owner_epoch,
            started_at_unix_ms,
        };
        info.validate()?;
        Ok(info)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != RUNTIME_INFO_SCHEMA_VERSION {
            return Err(RuntimeContractError::new("unsupported_runtime_info_schema"));
        }
        let host = self
            .host
            .parse::<IpAddr>()
            .map_err(|_| RuntimeContractError::new("invalid_runtime_host"))?;
        if self.pid == 0 || self.port == 0 || self.started_at_unix_ms == 0 || !host.is_loopback() {
            return Err(RuntimeContractError::new("invalid_runtime_info"));
        }
        Ok(())
    }

    pub fn socket_addr(&self) -> RuntimeContractResult<SocketAddr> {
        self.validate()?;
        let host = self
            .host
            .parse::<IpAddr>()
            .map_err(|_| RuntimeContractError::new("invalid_runtime_host"))?;
        Ok(SocketAddr::new(host, self.port))
    }

    pub const fn pid(&self) -> u32 {
        self.pid
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }
}

fn validate_point(x: i32, y: i32) -> RuntimeContractResult<()> {
    if x < 0 || y < 0 {
        return Err(RuntimeContractError::new("invalid_input_coordinate"));
    }
    Ok(())
}

fn validate_duration(duration_ms: u64) -> RuntimeContractResult<()> {
    if !(1..=MAX_INPUT_DURATION_MS).contains(&duration_ms) {
        return Err(RuntimeContractError::new("invalid_input_duration"));
    }
    Ok(())
}

fn validate_instance_alias(value: &str) -> RuntimeContractResult<()> {
    validate_bounded_text(value, MAX_INSTANCE_ALIAS_BYTES, "invalid_instance_alias")
}

fn validate_bounded_text(
    value: &str,
    max_bytes: usize,
    code: &'static str,
) -> RuntimeContractResult<()> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.chars().any(|character| character == '\0')
    {
        return Err(RuntimeContractError::new(code));
    }
    Ok(())
}

fn valid_client_origin(actor: EventActor, source: EventSource) -> bool {
    matches!(
        source,
        EventSource::Cli | EventSource::Ui | EventSource::Lab | EventSource::Adapter
    ) && matches!(
        actor,
        EventActor::User | EventActor::Cli | EventActor::Ui | EventActor::Lab | EventActor::Agent
    )
}

#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
