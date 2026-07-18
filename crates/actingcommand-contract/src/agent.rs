// SPDX-License-Identifier: AGPL-3.0-only

//! Typed Runtime boundary for a detachable Agent Dispatcher sidecar.

use crate::{
    AgentSessionId, AgentWakeId, CorrelationId, EventId, EventLinksDraft, IdentifierIssuanceError,
    IdentifierIssuer, InstanceId, IssuedActionId, IssuedCorrelationId, IssuedInstanceId,
    IssuedRequestId, ProjectedEvent, ProjectionPayload, SanitizationError,
};
use serde::{Deserialize, Serialize};

pub const MAX_AGENT_ATTEMPTS: u16 = 8;
pub const MAX_AGENT_SESSION_MS: u64 = 3_600_000;
pub const MAX_AGENT_PROJECTION_EVENTS: u16 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWakeKind {
    TimelineReached,
    DriftPredicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAttentionState {
    PausedNeedsAgent,
    Active,
    Completed,
    PausedNeedsHuman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    LedgerProjectionRead,
    StructuredReceiptWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentResponseDisposition {
    Completed,
    RetryableFailure,
    NeedsHuman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentWakeTrigger {
    instance_id: InstanceId,
    kind: AgentWakeKind,
    event_id: EventId,
    event_sequence: u64,
    observed_at_unix_ms: u64,
}

impl AgentWakeTrigger {
    pub fn new(
        instance_id: InstanceId,
        kind: AgentWakeKind,
        event_id: EventId,
        event_sequence: u64,
        observed_at_unix_ms: u64,
    ) -> Result<Self, SanitizationError> {
        if event_sequence == 0 || observed_at_unix_ms == 0 {
            return Err(SanitizationError::new(
                "invalid_agent_wake_trigger",
                "agent_wake",
            ));
        }
        Ok(Self {
            instance_id,
            kind,
            event_id,
            event_sequence,
            observed_at_unix_ms,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionBudget {
    max_attempts: u16,
    max_session_ms: u64,
}

impl AgentSessionBudget {
    pub fn new(max_attempts: u16, max_session_ms: u64) -> Result<Self, SanitizationError> {
        let value = Self {
            max_attempts,
            max_session_ms,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.max_attempts == 0
            || self.max_attempts > MAX_AGENT_ATTEMPTS
            || self.max_session_ms == 0
            || self.max_session_ms > MAX_AGENT_SESSION_MS
        {
            return Err(SanitizationError::new(
                "invalid_agent_session_budget",
                "agent_budget",
            ));
        }
        Ok(())
    }

    pub const fn max_attempts(&self) -> u16 {
        self.max_attempts
    }

    pub const fn max_session_ms(&self) -> u64 {
        self.max_session_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCapabilityContract {
    capabilities: Vec<AgentCapability>,
    max_projection_events: u16,
}

impl AgentCapabilityContract {
    pub fn read_only(max_projection_events: u16) -> Result<Self, SanitizationError> {
        let value = Self {
            capabilities: vec![
                AgentCapability::LedgerProjectionRead,
                AgentCapability::StructuredReceiptWrite,
            ],
            max_projection_events,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        let expected = [
            AgentCapability::LedgerProjectionRead,
            AgentCapability::StructuredReceiptWrite,
        ];
        if self.capabilities != expected
            || self.max_projection_events == 0
            || self.max_projection_events > MAX_AGENT_PROJECTION_EVENTS
        {
            return Err(SanitizationError::new(
                "invalid_agent_capability_contract",
                "agent_capabilities",
            ));
        }
        Ok(())
    }

    pub fn capabilities(&self) -> &[AgentCapability] {
        &self.capabilities
    }

    pub const fn max_projection_events(&self) -> u16 {
        self.max_projection_events
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeData {
    wake_id: AgentWakeId,
    instance_id: InstanceId,
    correlation_id: CorrelationId,
    kind: AgentWakeKind,
    trigger_event_id: EventId,
    trigger_sequence: u64,
    requested_at_unix_ms: u64,
    attention_state: AgentAttentionState,
    budget: AgentSessionBudget,
    capabilities: AgentCapabilityContract,
}

impl AgentWakeData {
    pub fn new(
        wake_id: AgentWakeId,
        correlation_id: CorrelationId,
        trigger: AgentWakeTrigger,
        budget: AgentSessionBudget,
        capabilities: AgentCapabilityContract,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            wake_id,
            instance_id: trigger.instance_id,
            correlation_id,
            kind: trigger.kind,
            trigger_event_id: trigger.event_id,
            trigger_sequence: trigger.event_sequence,
            requested_at_unix_ms: trigger.observed_at_unix_ms,
            attention_state: AgentAttentionState::PausedNeedsAgent,
            budget,
            capabilities,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.trigger_sequence == 0
            || self.requested_at_unix_ms == 0
            || self.attention_state != AgentAttentionState::PausedNeedsAgent
        {
            return Err(SanitizationError::new("invalid_agent_wake", "agent_wake"));
        }
        self.budget.validate()?;
        self.capabilities.validate()
    }

    pub const fn wake_id(&self) -> AgentWakeId {
        self.wake_id
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn kind(&self) -> AgentWakeKind {
        self.kind
    }

    pub const fn trigger_event_id(&self) -> EventId {
        self.trigger_event_id
    }

    pub const fn trigger_sequence(&self) -> u64 {
        self.trigger_sequence
    }

    pub const fn requested_at_unix_ms(&self) -> u64 {
        self.requested_at_unix_ms
    }

    pub const fn attention_state(&self) -> AgentAttentionState {
        self.attention_state
    }

    pub const fn budget(&self) -> AgentSessionBudget {
        self.budget
    }

    pub const fn capabilities(&self) -> &AgentCapabilityContract {
        &self.capabilities
    }
}

/// Producer capability for a Runtime-authored wake event and its correlation boundary.
pub struct IssuedAgentWake {
    data: AgentWakeData,
    instance_id: IssuedInstanceId,
    request_id: IssuedRequestId,
    correlation_id: IssuedCorrelationId,
    action_id: IssuedActionId,
}

impl IssuedAgentWake {
    pub const fn data(&self) -> &AgentWakeData {
        &self.data
    }

    pub fn event_links(&self) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_instance_id(self.instance_id)
            .with_request_id(self.request_id)
            .with_correlation_id(self.correlation_id)
            .with_action_id(self.action_id)
    }
}

impl IdentifierIssuer {
    pub fn issue_agent_wake(
        &self,
        trigger: AgentWakeTrigger,
        budget: AgentSessionBudget,
        capabilities: AgentCapabilityContract,
    ) -> Result<IssuedAgentWake, IdentifierIssuanceError> {
        let wake_id = self.mint_agent_wake_id()?;
        let request_id = self.mint_request_id()?;
        let correlation_id = self.mint_correlation_id()?;
        let action_id = self.mint_action_id()?;
        let data = AgentWakeData::new(
            *wake_id.transport(),
            *correlation_id.transport(),
            trigger,
            budget,
            capabilities,
        )
        .map_err(|_| IdentifierIssuanceError::contract_invalid())?;
        Ok(IssuedAgentWake {
            data,
            instance_id: IssuedInstanceId::from_verified_transport(trigger.instance_id),
            request_id,
            correlation_id,
            action_id,
        })
    }

    pub fn issue_agent_session_links(
        &self,
        instance_id: InstanceId,
    ) -> Result<IssuedAgentSessionLinks, IdentifierIssuanceError> {
        Ok(IssuedAgentSessionLinks {
            instance_id: IssuedInstanceId::from_verified_transport(instance_id),
            request_id: self.mint_request_id()?,
            correlation_id: self.mint_correlation_id()?,
            action_id: self.mint_action_id()?,
        })
    }
}

pub struct IssuedAgentSessionLinks {
    instance_id: IssuedInstanceId,
    request_id: IssuedRequestId,
    correlation_id: IssuedCorrelationId,
    action_id: IssuedActionId,
}

impl IssuedAgentSessionLinks {
    pub fn event_links(&self) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_instance_id(self.instance_id)
            .with_request_id(self.request_id)
            .with_correlation_id(self.correlation_id)
            .with_action_id(self.action_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionStatus {
    session_id: AgentSessionId,
    wake_id: AgentWakeId,
    instance_id: InstanceId,
    correlation_id: CorrelationId,
    state: AgentAttentionState,
    attempts_used: u16,
    started_at_unix_ms: u64,
    updated_at_unix_ms: u64,
    budget: AgentSessionBudget,
    capabilities: AgentCapabilityContract,
}

impl AgentSessionStatus {
    pub fn started(
        session_id: AgentSessionId,
        wake: &AgentWakeData,
        started_at_unix_ms: u64,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            session_id,
            wake_id: wake.wake_id,
            instance_id: wake.instance_id,
            correlation_id: wake.correlation_id,
            state: AgentAttentionState::Active,
            attempts_used: 1,
            started_at_unix_ms,
            updated_at_unix_ms: started_at_unix_ms,
            budget: wake.budget,
            capabilities: wake.capabilities.clone(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn resumed(&self, observed_at_unix_ms: u64) -> Result<Self, SanitizationError> {
        if self.state != AgentAttentionState::Active {
            return Err(SanitizationError::new(
                "agent_session_not_active",
                "agent_session",
            ));
        }
        let mut value = self.clone();
        value.updated_at_unix_ms = observed_at_unix_ms;
        value.validate()?;
        Ok(value)
    }

    pub fn retry_or_escalate(&self, observed_at_unix_ms: u64) -> Result<Self, SanitizationError> {
        if self.state != AgentAttentionState::Active {
            return Err(SanitizationError::new(
                "agent_session_not_active",
                "agent_session",
            ));
        }
        let mut value = self.clone();
        value.updated_at_unix_ms = observed_at_unix_ms;
        if value.attempts_used >= value.budget.max_attempts {
            value.state = AgentAttentionState::PausedNeedsHuman;
        } else {
            value.attempts_used += 1;
        }
        value.validate()?;
        Ok(value)
    }

    pub fn completed(&self, observed_at_unix_ms: u64) -> Result<Self, SanitizationError> {
        self.terminal(AgentAttentionState::Completed, observed_at_unix_ms)
    }

    pub fn escalated(&self, observed_at_unix_ms: u64) -> Result<Self, SanitizationError> {
        self.terminal(AgentAttentionState::PausedNeedsHuman, observed_at_unix_ms)
    }

    fn terminal(
        &self,
        state: AgentAttentionState,
        observed_at_unix_ms: u64,
    ) -> Result<Self, SanitizationError> {
        if self.state != AgentAttentionState::Active {
            return Err(SanitizationError::new(
                "agent_session_not_active",
                "agent_session",
            ));
        }
        let mut value = self.clone();
        value.state = state;
        value.updated_at_unix_ms = observed_at_unix_ms;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.budget.validate()?;
        self.capabilities.validate()?;
        let deadline = self
            .started_at_unix_ms
            .checked_add(self.budget.max_session_ms)
            .ok_or_else(|| SanitizationError::new("invalid_agent_session_time", "agent_session"))?;
        if self.started_at_unix_ms == 0
            || self.updated_at_unix_ms < self.started_at_unix_ms
            || self.attempts_used == 0
            || self.attempts_used > self.budget.max_attempts
            || self.state == AgentAttentionState::PausedNeedsAgent
            || (self.state == AgentAttentionState::Active && self.updated_at_unix_ms > deadline)
        {
            return Err(SanitizationError::new(
                "invalid_agent_session",
                "agent_session",
            ));
        }
        Ok(())
    }

    pub const fn session_id(&self) -> AgentSessionId {
        self.session_id
    }

    pub const fn wake_id(&self) -> AgentWakeId {
        self.wake_id
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn state(&self) -> AgentAttentionState {
        self.state
    }

    pub const fn attempts_used(&self) -> u16 {
        self.attempts_used
    }

    pub const fn started_at_unix_ms(&self) -> u64 {
        self.started_at_unix_ms
    }

    pub const fn updated_at_unix_ms(&self) -> u64 {
        self.updated_at_unix_ms
    }

    pub const fn budget(&self) -> AgentSessionBudget {
        self.budget
    }

    pub const fn capabilities(&self) -> &AgentCapabilityContract {
        &self.capabilities
    }

    pub fn expired_at(&self, now_unix_ms: u64) -> bool {
        self.started_at_unix_ms
            .checked_add(self.budget.max_session_ms)
            .is_none_or(|deadline| now_unix_ms > deadline)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionResponse {
    session_id: AgentSessionId,
    disposition: AgentResponseDisposition,
    code: String,
    observed_at_unix_ms: u64,
}

impl AgentSessionResponse {
    pub fn new(
        session_id: AgentSessionId,
        disposition: AgentResponseDisposition,
        code: impl Into<String>,
        observed_at_unix_ms: u64,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            session_id,
            disposition,
            code: code.into(),
            observed_at_unix_ms,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.observed_at_unix_ms == 0 || !valid_agent_code(&self.code) {
            return Err(SanitizationError::new(
                "invalid_agent_response",
                "agent_response",
            ));
        }
        Ok(())
    }

    pub const fn session_id(&self) -> AgentSessionId {
        self.session_id
    }

    pub const fn disposition(&self) -> AgentResponseDisposition {
        self.disposition
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    /// Sidecar-reported evidence time; Runtime-owned clocks govern session deadlines.
    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionEventData {
    status: AgentSessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<AgentSessionResponse>,
}

impl AgentSessionEventData {
    pub fn new(
        status: AgentSessionStatus,
        response: Option<AgentSessionResponse>,
    ) -> Result<Self, SanitizationError> {
        let value = Self { status, response };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.status.validate()?;
        if let Some(response) = &self.response {
            response.validate()?;
            if response.session_id != self.status.session_id {
                return Err(SanitizationError::new(
                    "agent_response_session_mismatch",
                    "agent_response",
                ));
            }
        }
        Ok(())
    }

    pub const fn status(&self) -> &AgentSessionStatus {
        &self.status
    }

    pub const fn response(&self) -> Option<&AgentSessionResponse> {
        self.response.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionContext {
    status: AgentSessionStatus,
    projection: Vec<ProjectedEvent>,
}

impl AgentSessionContext {
    pub fn new(
        status: AgentSessionStatus,
        projection: Vec<ProjectedEvent>,
    ) -> Result<Self, SanitizationError> {
        let value = Self { status, projection };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.status.validate()?;
        if self.projection.is_empty()
            || self.projection.len() > usize::from(self.status.capabilities.max_projection_events)
            || self
                .projection
                .iter()
                .any(|event| !matches!(event.payload, ProjectionPayload::Public(_)))
        {
            return Err(SanitizationError::new(
                "invalid_agent_ledger_projection",
                "agent_projection",
            ));
        }
        Ok(())
    }

    pub const fn status(&self) -> &AgentSessionStatus {
        &self.status
    }

    pub fn projection(&self) -> &[ProjectedEvent] {
        &self.projection
    }
}

fn valid_agent_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IdentifierIssuer;

    #[test]
    fn capability_contract_has_no_device_authority() {
        let capabilities = AgentCapabilityContract::read_only(4).expect("capabilities");
        assert_eq!(
            capabilities.capabilities(),
            &[
                AgentCapability::LedgerProjectionRead,
                AgentCapability::StructuredReceiptWrite,
            ]
        );
    }

    #[test]
    fn retry_budget_escalates_only_after_paused_needs_agent() {
        let ids = IdentifierIssuer::new().expect("issuer");
        let wake = AgentWakeData::new(
            *ids.mint_agent_wake_id().expect("wake id").transport(),
            *ids.mint_correlation_id()
                .expect("correlation id")
                .transport(),
            AgentWakeTrigger::new(
                *ids.mint_instance_id().expect("instance id").transport(),
                AgentWakeKind::DriftPredicted,
                *ids.mint_event_id().expect("event id").transport(),
                1,
                10,
            )
            .expect("wake trigger"),
            AgentSessionBudget::new(2, 1_000).expect("budget"),
            AgentCapabilityContract::read_only(2).expect("capabilities"),
        )
        .expect("wake");
        assert_eq!(
            wake.attention_state(),
            AgentAttentionState::PausedNeedsAgent
        );
        let session = AgentSessionStatus::started(
            *ids.mint_agent_session_id().expect("session id").transport(),
            &wake,
            20,
        )
        .expect("session");
        let retry = session.retry_or_escalate(30).expect("retry");
        assert_eq!(retry.state(), AgentAttentionState::Active);
        assert_eq!(retry.attempts_used(), 2);
        assert_eq!(
            retry.retry_or_escalate(40).expect("escalation").state(),
            AgentAttentionState::PausedNeedsHuman
        );
    }
}
