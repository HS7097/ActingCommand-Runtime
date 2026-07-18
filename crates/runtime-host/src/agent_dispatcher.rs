// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned durable session state for a detachable Agent Dispatcher sidecar.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    AgentAttentionState, AgentCapabilityContract, AgentPayload, AgentResponseDisposition,
    AgentSessionBudget, AgentSessionContext, AgentSessionEventData, AgentSessionId,
    AgentSessionResponse, AgentSessionStatus, AgentWakeData, AgentWakeId, CorrelationId,
    EventPayload, EventQuery, EventType, InstanceId, PolicyPayload, PolicyPlanningSignalKind,
    ProjectedEvent, ProjectionProfile, RequestId, RuntimeErrorCode, TerminalEvent,
};
use actingcommand_ledger::{GlobalLedger, PersistedEvent};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDispatcherConfig {
    budget: AgentSessionBudget,
    capabilities: AgentCapabilityContract,
}

impl AgentDispatcherConfig {
    pub fn new(
        max_attempts: u16,
        max_session_ms: u64,
        max_projection_events: u16,
    ) -> RuntimeHostResult<Self> {
        let budget = AgentSessionBudget::new(max_attempts, max_session_ms).map_err(|_| {
            request(
                "agent_dispatcher_config_invalid",
                "configure_agent_dispatcher",
            )
        })?;
        let capabilities =
            AgentCapabilityContract::read_only(max_projection_events).map_err(|_| {
                request(
                    "agent_dispatcher_config_invalid",
                    "configure_agent_dispatcher",
                )
            })?;
        Ok(Self {
            budget,
            capabilities,
        })
    }

    pub(crate) const fn budget(&self) -> AgentSessionBudget {
        self.budget
    }

    pub(crate) const fn capabilities(&self) -> &AgentCapabilityContract {
        &self.capabilities
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AgentWakeRecord {
    data: AgentWakeData,
    event_sequence: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct AgentWakeSource {
    sequence: u64,
    timestamp_unix_ms: u64,
    instance_id: InstanceId,
    kind: actingcommand_contract::AgentWakeKind,
}

impl AgentWakeRecord {
    pub(crate) const fn data(&self) -> &AgentWakeData {
        &self.data
    }
}

pub(crate) enum AgentSessionPreparation {
    New(AgentSessionEventData),
    Replay(AgentSessionStatus),
}

pub(crate) enum AgentResumePreparation {
    New(AgentSessionEventData),
    Replay {
        status: AgentSessionStatus,
        terminal: TerminalEvent,
    },
}

pub(crate) enum AgentResponsePreparation {
    Retry(AgentSessionEventData),
    Complete(AgentSessionEventData),
    Escalate(AgentSessionEventData),
    Replay(AgentSessionStatus),
}

#[derive(Default)]
pub(crate) struct AgentDispatcherState {
    wakes: BTreeMap<AgentWakeId, AgentWakeRecord>,
    wakes_by_trigger: BTreeMap<actingcommand_contract::EventId, AgentWakeId>,
    sessions: BTreeMap<AgentSessionId, AgentSessionStatus>,
    session_by_wake: BTreeMap<AgentWakeId, AgentSessionId>,
    active_by_scope: BTreeMap<(CorrelationId, InstanceId), AgentSessionId>,
    resume_requests: BTreeMap<RequestId, AgentResumeRecord>,
    response_requests: BTreeMap<RequestId, AgentSessionEventData>,
}

#[derive(Clone)]
struct AgentResumeRecord {
    session_id: AgentSessionId,
    correlation_id: CorrelationId,
    status: AgentSessionStatus,
    terminal: TerminalEvent,
}

impl AgentDispatcherState {
    pub(crate) fn recover(
        ledger: &GlobalLedger,
        instance_ids_by_alias: &BTreeMap<String, InstanceId>,
    ) -> RuntimeHostResult<Self> {
        let events = ledger
            .query(EventQuery::default())
            .map_err(|_| ledger_error("recover_agent_dispatcher"))?;
        let source_events = events
            .iter()
            .filter_map(|event| {
                let EventPayload::Policy(PolicyPayload::PlanningSignalObserved(signal)) =
                    event.payload()
                else {
                    return None;
                };
                let kind = match signal.kind() {
                    PolicyPlanningSignalKind::TimelineReached => {
                        actingcommand_contract::AgentWakeKind::TimelineReached
                    }
                    PolicyPlanningSignalKind::DriftPredicted => {
                        actingcommand_contract::AgentWakeKind::DriftPredicted
                    }
                    _ => return None,
                };
                instance_ids_by_alias
                    .get(signal.instance_id())
                    .copied()
                    .map(|instance_id| {
                        (
                            *event.event_id(),
                            AgentWakeSource {
                                sequence: event.sequence(),
                                timestamp_unix_ms: event.timestamp_unix_ms(),
                                instance_id,
                                kind,
                            },
                        )
                    })
            })
            .collect::<BTreeMap<_, _>>();
        let mut state = Self::default();
        for event in &events {
            if matches!(event.payload(), EventPayload::Agent(_)) {
                state.apply_event(event, Some(&source_events))?;
            }
        }
        Ok(state)
    }

    pub(crate) fn has_live_obligations(&self) -> bool {
        self.wakes
            .keys()
            .any(|wake_id| !self.session_by_wake.contains_key(wake_id))
            || self.sessions.values().any(|status| {
                matches!(
                    status.state(),
                    AgentAttentionState::PausedNeedsAgent | AgentAttentionState::Active
                )
            })
    }

    pub(crate) fn has_wake_for_trigger(
        &self,
        trigger_event_id: &actingcommand_contract::EventId,
    ) -> bool {
        self.wakes_by_trigger.contains_key(trigger_event_id)
    }

    pub(crate) fn wake(&self, wake_id: AgentWakeId) -> RuntimeHostResult<&AgentWakeRecord> {
        self.wakes
            .get(&wake_id)
            .ok_or_else(|| request("agent_wake_unknown", "start_agent_session"))
    }

    pub(crate) fn session(
        &self,
        session_id: AgentSessionId,
    ) -> RuntimeHostResult<&AgentSessionStatus> {
        self.sessions
            .get(&session_id)
            .ok_or_else(|| request("agent_session_unknown", "read_agent_session"))
    }

    pub(crate) fn prepare_start(
        &self,
        wake_id: AgentWakeId,
        session_id: AgentSessionId,
        started_at_unix_ms: u64,
    ) -> RuntimeHostResult<AgentSessionPreparation> {
        let wake = self.wake(wake_id)?;
        if let Some(existing) = self.session_by_wake.get(&wake_id) {
            return Ok(AgentSessionPreparation::Replay(
                self.session(*existing)?.clone(),
            ));
        }
        let scope = (wake.data.correlation_id(), wake.data.instance_id());
        if self.active_by_scope.contains_key(&scope) {
            return Err(fatal("agent_session_scope_conflict", "start_agent_session"));
        }
        let status = AgentSessionStatus::started(session_id, wake.data(), started_at_unix_ms)
            .map_err(|_| request("agent_session_invalid", "start_agent_session"))?;
        let data = AgentSessionEventData::new(status, None)
            .map_err(|_| fatal("agent_session_event_invalid", "start_agent_session"))?;
        Ok(AgentSessionPreparation::New(data))
    }

    pub(crate) fn prepare_resume(
        &self,
        request_id: RequestId,
        correlation_id: CorrelationId,
        session_id: AgentSessionId,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<AgentResumePreparation> {
        if let Some(existing) = self.resume_requests.get(&request_id) {
            return if existing.session_id == session_id && existing.correlation_id == correlation_id
            {
                Ok(AgentResumePreparation::Replay {
                    status: existing.status.clone(),
                    terminal: existing.terminal,
                })
            } else {
                Err(request_identity_conflict(AgentRequestKind::Resume))
            };
        }
        if self.response_requests.contains_key(&request_id) {
            return Err(request_identity_conflict(AgentRequestKind::Resume));
        }
        let current = self.session(session_id)?;
        if current.expired_at(observed_at_unix_ms) {
            return Err(request("agent_session_expired", "resume_agent_session"));
        }
        let status = current
            .resumed(observed_at_unix_ms)
            .map_err(|_| request("agent_session_not_active", "resume_agent_session"))?;
        AgentSessionEventData::new(status, None)
            .map(AgentResumePreparation::New)
            .map_err(|_| fatal("agent_session_event_invalid", "resume_agent_session"))
    }

    pub(crate) fn prepare_response(
        &self,
        request_id: RequestId,
        response: &AgentSessionResponse,
        runtime_observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<AgentResponsePreparation> {
        if let Some(existing) = self.response_requests.get(&request_id) {
            return if existing.response() == Some(response) {
                Ok(AgentResponsePreparation::Replay(existing.status().clone()))
            } else {
                Err(request_identity_conflict(AgentRequestKind::Response))
            };
        }
        if self.resume_requests.contains_key(&request_id) {
            return Err(request_identity_conflict(AgentRequestKind::Response));
        }
        let current = self.session(response.session_id())?;
        if current.expired_at(runtime_observed_at_unix_ms) {
            return Err(request("agent_session_expired", "record_agent_response"));
        }
        let status =
            match response.disposition() {
                AgentResponseDisposition::Completed => current
                    .completed(runtime_observed_at_unix_ms)
                    .map_err(|_| request("agent_session_not_active", "record_agent_response"))?,
                AgentResponseDisposition::RetryableFailure => current
                    .retry_or_escalate(runtime_observed_at_unix_ms)
                    .map_err(|_| request("agent_session_not_active", "record_agent_response"))?,
                AgentResponseDisposition::NeedsHuman => current
                    .escalated(runtime_observed_at_unix_ms)
                    .map_err(|_| request("agent_session_not_active", "record_agent_response"))?,
            };
        let data = AgentSessionEventData::new(status.clone(), Some(response.clone()))
            .map_err(|_| fatal("agent_session_event_invalid", "record_agent_response"))?;
        Ok(match status.state() {
            AgentAttentionState::Active => AgentResponsePreparation::Retry(data),
            AgentAttentionState::Completed => AgentResponsePreparation::Complete(data),
            AgentAttentionState::PausedNeedsHuman => AgentResponsePreparation::Escalate(data),
            AgentAttentionState::PausedNeedsAgent => {
                return Err(fatal(
                    "agent_session_transition_invalid",
                    "record_agent_response",
                ));
            }
        })
    }

    pub(crate) fn expired_sessions(
        &self,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<Vec<AgentSessionEventData>> {
        self.sessions
            .values()
            .filter(|status| {
                status.state() == AgentAttentionState::Active && status.expired_at(now_unix_ms)
            })
            .map(|status| {
                let response = AgentSessionResponse::new(
                    status.session_id(),
                    AgentResponseDisposition::NeedsHuman,
                    "session_timeout",
                    now_unix_ms,
                )
                .map_err(|_| fatal("agent_timeout_response_invalid", "expire_agent_sessions"))?;
                let status = status
                    .escalated(now_unix_ms)
                    .map_err(|_| fatal("agent_timeout_status_invalid", "expire_agent_sessions"))?;
                AgentSessionEventData::new(status, Some(response))
                    .map_err(|_| fatal("agent_session_event_invalid", "expire_agent_sessions"))
            })
            .collect()
    }

    pub(crate) fn context(
        &self,
        ledger: &GlobalLedger,
        status: AgentSessionStatus,
    ) -> RuntimeHostResult<AgentSessionContext> {
        let wake = self.wake(status.wake_id())?;
        let mut projection = Vec::<ProjectedEvent>::new();
        for sequence in [wake.data.trigger_sequence(), wake.event_sequence] {
            let mut events = ledger
                .project(
                    EventQuery {
                        from_sequence: Some(sequence),
                        to_sequence: Some(sequence),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Normal,
                )
                .map_err(|_| ledger_error("project_agent_context"))?;
            projection.append(&mut events);
        }
        projection.sort_by_key(|event| event.sequence);
        projection.dedup_by_key(|event| event.sequence);
        AgentSessionContext::new(status, projection)
            .map_err(|_| fatal("agent_context_invalid", "project_agent_context"))
    }

    pub(crate) fn apply_event(
        &mut self,
        event: &PersistedEvent,
        source_events: Option<&BTreeMap<actingcommand_contract::EventId, AgentWakeSource>>,
    ) -> RuntimeHostResult<()> {
        let EventPayload::Agent(payload) = event.payload() else {
            return Err(fatal("agent_event_family_invalid", "apply_agent_event"));
        };
        match payload {
            AgentPayload::WakeRequested(payload) => {
                let wake = payload.wake();
                if event.links().instance_id() != Some(&wake.instance_id())
                    || event.links().correlation_id() != Some(&wake.correlation_id())
                    || source_events.is_some_and(|events| {
                        events.get(&wake.trigger_event_id()).is_none_or(|source| {
                            source.sequence != wake.trigger_sequence()
                                || source.timestamp_unix_ms != wake.requested_at_unix_ms()
                                || source.instance_id != wake.instance_id()
                                || source.kind != wake.kind()
                        })
                    })
                {
                    return Err(fatal("agent_wake_links_invalid", "apply_agent_event"));
                }
                if let Some(existing) = self.wakes.get(&wake.wake_id()) {
                    if existing.data != *wake || existing.event_sequence != event.sequence() {
                        return Err(fatal("agent_wake_identity_conflict", "apply_agent_event"));
                    }
                    return Ok(());
                }
                if self.wakes_by_trigger.contains_key(&wake.trigger_event_id()) {
                    return Err(fatal("agent_wake_trigger_conflict", "apply_agent_event"));
                }
                self.wakes_by_trigger
                    .insert(wake.trigger_event_id(), wake.wake_id());
                self.wakes.insert(
                    wake.wake_id(),
                    AgentWakeRecord {
                        data: wake.clone(),
                        event_sequence: event.sequence(),
                    },
                );
            }
            AgentPayload::SessionStarted(payload)
            | AgentPayload::SessionResumed(payload)
            | AgentPayload::ResponseRecorded(payload)
            | AgentPayload::SessionCompleted(payload)
            | AgentPayload::SessionEscalated(payload) => {
                self.apply_session_event(event, payload.session())?;
            }
        }
        Ok(())
    }

    fn apply_session_event(
        &mut self,
        event: &PersistedEvent,
        data: &AgentSessionEventData,
    ) -> RuntimeHostResult<()> {
        let status = data.status();
        let wake = self.wake(status.wake_id())?;
        if status.instance_id() != wake.data.instance_id()
            || status.correlation_id() != wake.data.correlation_id()
            || event.links().instance_id() != Some(&status.instance_id())
        {
            return Err(fatal("agent_session_links_invalid", "apply_agent_event"));
        }
        let event_type = event.event_type();
        let previous = self.sessions.get(&status.session_id()).cloned();
        match event_type {
            EventType::AgentSessionStarted => {
                if previous.is_some()
                    || self.session_by_wake.contains_key(&status.wake_id())
                    || status.state() != AgentAttentionState::Active
                    || status.attempts_used() != 1
                {
                    return Err(fatal("agent_session_start_conflict", "apply_agent_event"));
                }
            }
            EventType::AgentSessionResumed => {
                let previous = previous
                    .as_ref()
                    .ok_or_else(|| fatal("agent_session_resume_missing", "apply_agent_event"))?;
                if previous.state() != AgentAttentionState::Active
                    || status.state() != AgentAttentionState::Active
                    || status.attempts_used() != previous.attempts_used()
                {
                    return Err(fatal("agent_session_resume_invalid", "apply_agent_event"));
                }
            }
            EventType::AgentResponseRecorded => {
                let previous = previous
                    .as_ref()
                    .ok_or_else(|| fatal("agent_session_response_missing", "apply_agent_event"))?;
                if previous.state() != AgentAttentionState::Active
                    || status.state() != AgentAttentionState::Active
                    || status.attempts_used() != previous.attempts_used().saturating_add(1)
                {
                    return Err(fatal("agent_session_response_invalid", "apply_agent_event"));
                }
            }
            EventType::AgentSessionCompleted | EventType::AgentSessionEscalated => {
                let previous = previous
                    .as_ref()
                    .ok_or_else(|| fatal("agent_session_terminal_missing", "apply_agent_event"))?;
                if previous.state() != AgentAttentionState::Active
                    || !matches!(
                        status.state(),
                        AgentAttentionState::Completed | AgentAttentionState::PausedNeedsHuman
                    )
                {
                    return Err(fatal("agent_session_terminal_invalid", "apply_agent_event"));
                }
            }
            _ => return Err(fatal("agent_session_event_invalid", "apply_agent_event")),
        }
        let resume_request =
            if event_type == EventType::AgentSessionResumed {
                let request_id =
                    event.links().request_id().copied().ok_or_else(|| {
                        fatal("agent_resume_request_missing", "apply_agent_event")
                    })?;
                let correlation_id = event.links().correlation_id().copied().ok_or_else(|| {
                    fatal("agent_resume_correlation_missing", "apply_agent_event")
                })?;
                if self.resume_requests.contains_key(&request_id)
                    || self.response_requests.contains_key(&request_id)
                {
                    return Err(fatal("agent_resume_request_conflict", "apply_agent_event"));
                }
                Some((request_id, correlation_id))
            } else {
                None
            };
        let response_request_id = if let Some(response) = data.response() {
            let request_id = event
                .links()
                .request_id()
                .copied()
                .ok_or_else(|| fatal("agent_response_request_missing", "apply_agent_event"))?;
            if self.response_requests.contains_key(&request_id)
                || self.resume_requests.contains_key(&request_id)
            {
                return Err(fatal(
                    "agent_response_request_conflict",
                    "apply_agent_event",
                ));
            }
            if response.session_id() != status.session_id() {
                return Err(fatal(
                    "agent_response_session_mismatch",
                    "apply_agent_event",
                ));
            }
            Some(request_id)
        } else {
            None
        };
        let scope = (status.correlation_id(), status.instance_id());
        if status.state() == AgentAttentionState::Active
            && self
                .active_by_scope
                .get(&scope)
                .is_some_and(|session_id| *session_id != status.session_id())
        {
            return Err(fatal("agent_session_scope_conflict", "apply_agent_event"));
        }
        if let Some(previous) = previous
            && previous.state() == AgentAttentionState::Active
        {
            self.active_by_scope
                .remove(&(previous.correlation_id(), previous.instance_id()));
        }
        self.sessions.insert(status.session_id(), status.clone());
        self.session_by_wake
            .insert(status.wake_id(), status.session_id());
        if let Some((request_id, correlation_id)) = resume_request {
            self.resume_requests.insert(
                request_id,
                AgentResumeRecord {
                    session_id: status.session_id(),
                    correlation_id,
                    status: status.clone(),
                    terminal: TerminalEvent {
                        sequence: event.sequence(),
                        event_id: *event.event_id(),
                    },
                },
            );
        }
        if let Some(request_id) = response_request_id {
            self.response_requests.insert(request_id, data.clone());
        }
        if status.state() == AgentAttentionState::Active {
            self.active_by_scope.insert(scope, status.session_id());
        }
        Ok(())
    }
}

fn request(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

#[derive(Clone, Copy)]
enum AgentRequestKind {
    Resume,
    Response,
}

fn request_identity_conflict(kind: AgentRequestKind) -> RuntimeHostError {
    match kind {
        AgentRequestKind::Resume => {
            request("agent_resume_request_conflict", "resume_agent_session")
        }
        AgentRequestKind::Response => {
            request("agent_response_request_conflict", "record_agent_response")
        }
    }
}

fn fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

fn ledger_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "agent_ledger_failure",
        operation,
        RuntimeErrorCode::LedgerFailure,
    )
}
