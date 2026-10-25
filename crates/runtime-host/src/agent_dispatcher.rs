// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned durable session state for a detachable Agent Dispatcher sidecar.

use crate::codes::{HostCode, HostLocation};
use crate::recovery_read::{RECOVERY_PAGE_EVENTS, read_event_types, read_event_types_after};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{
    AgentAttentionState, AgentCapabilityContract, AgentPayload, AgentResponseDisposition,
    AgentSessionBudget, AgentSessionContext, AgentSessionEventData, AgentSessionId,
    AgentSessionResponse, AgentSessionStatus, AgentWakeData, AgentWakeId, CorrelationId,
    EventPayload, EventQuery, EventType, InstanceId, PolicyPayload, PolicyPlanningSignalKind,
    ProjectedEvent, ProjectionProfile, RequestId, RuntimeErrorCode, TerminalEvent,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerResult, LedgerIndexRole, PersistedEvent};
use std::collections::BTreeMap;

/// Workflow #422 LN-4: the `state` keys of the wake obligations (`wake/<wake id>`). The row names
/// the obligation's latest record: its value record while no session has started, then its
/// session's latest record (`contracts/ledger-observer.md` §11.1).
pub(crate) const WAKE_KEY_PREFIX: &str = "wake/";

/// The event types of the Agent records the dispatcher replays; the Ledger checks every record's
/// type against its payload, so these types select exactly those records. A wake obligation's
/// value record (`agent.wake_obligation_recorded`, Workflow #383 register L8) is loaded from the
/// tagged state (AO-6) and never replayed.
const AGENT_EVENT_TYPES: [EventType; 6] = [
    EventType::AgentWakeRequested,
    EventType::AgentSessionStarted,
    EventType::AgentSessionResumed,
    EventType::AgentResponseRecorded,
    EventType::AgentSessionCompleted,
    EventType::AgentSessionEscalated,
];

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
                SharedCode::AgentDispatcherConfigInvalid,
                HostLocation::ConfigureAgentDispatcher,
            )
        })?;
        let capabilities =
            AgentCapabilityContract::read_only(max_projection_events).map_err(|_| {
                request(
                    SharedCode::AgentDispatcherConfigInvalid,
                    HostLocation::ConfigureAgentDispatcher,
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentDispatcherState {
    wakes: BTreeMap<AgentWakeId, AgentWakeRecord>,
    wakes_by_trigger: BTreeMap<actingcommand_contract::EventId, AgentWakeId>,
    sessions: BTreeMap<AgentSessionId, AgentSessionStatus>,
    session_by_wake: BTreeMap<AgentWakeId, AgentSessionId>,
    active_by_scope: BTreeMap<(CorrelationId, InstanceId), AgentSessionId>,
    resume_requests: BTreeMap<RequestId, AgentResumeRecord>,
    response_requests: BTreeMap<RequestId, AgentSessionEventData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
        Self::recover_paged(ledger, instance_ids_by_alias, RECOVERY_PAGE_EVENTS)
    }

    /// Workflow #381 R4a: reads only the planning signals and the Agent records, each type in
    /// pages of at most `page_events` events pinned to one ledger position, and folds them as
    /// the whole-ledger read did.
    pub(crate) fn recover_paged(
        ledger: &GlobalLedger,
        instance_ids_by_alias: &BTreeMap<String, InstanceId>,
        page_events: usize,
    ) -> RuntimeHostResult<Self> {
        let (signals, agent_events) = read_recovery_events(ledger, page_events)
            .map_err(|_| ledger_error(HostLocation::RecoverAgentDispatcher))?;
        Self::fold(&signals, &agent_events, instance_ids_by_alias)
    }

    /// Workflow #422 LN-4 (LEAN-L model §3.2 step 5, §3.6; `contracts/ledger-observer.md` §11.5):
    /// the start's load from the tagged records. The `wake/` rows name each obligation's latest
    /// record:
    /// - an obligation whose session completed is not loaded;
    /// - a session escalated to a person keeps its status and its wake, as the session's end
    ///   leaves them (`end_session`), read from the row's record and the wake's own record;
    /// - for the live obligations (no session yet, or an active one) the Agent records are read
    ///   from the earliest live wake's own record on, in pages, and only the records of live
    ///   wakes are folded, so their sessions and request records are rebuilt as a whole-ledger
    ///   fold rebuilds them.
    ///
    /// With no live obligation nothing but the rows and the escalated wakes' records is read.
    pub(crate) fn load(ledger: &GlobalLedger, page_events: usize) -> RuntimeHostResult<Self> {
        let rows =
            crate::tagged_state::index_records(ledger, LedgerIndexRole::State, &[WAKE_KEY_PREFIX])?;
        let mut live = BTreeMap::<AgentWakeId, CorrelationId>::new();
        let mut escalated = Vec::new();
        for record in &rows.records {
            match record.event.payload() {
                EventPayload::Agent(AgentPayload::WakeObligationRecorded(obligation)) => {
                    let wake = obligation.wake();
                    live.insert(wake.wake_id(), wake.correlation_id());
                }
                EventPayload::Agent(
                    AgentPayload::SessionStarted(payload)
                    | AgentPayload::SessionResumed(payload)
                    | AgentPayload::ResponseRecorded(payload)
                    | AgentPayload::SessionCompleted(payload)
                    | AgentPayload::SessionEscalated(payload),
                ) => {
                    let status = payload.session().status();
                    match status.state() {
                        AgentAttentionState::Completed => {}
                        AgentAttentionState::PausedNeedsHuman => escalated.push(status.clone()),
                        AgentAttentionState::Active | AgentAttentionState::PausedNeedsAgent => {
                            live.insert(status.wake_id(), status.correlation_id());
                        }
                    }
                }
                _ => {
                    return Err(fatal(
                        HostCode::AgentEventFamilyInvalid,
                        HostLocation::RecoverAgentDispatcher,
                    ));
                }
            }
        }
        let mut state = Self::default();
        for status in escalated {
            let requested = wake_record(ledger, status.correlation_id())?;
            state.apply_event(&requested, None)?;
            state
                .session_by_wake
                .insert(status.wake_id(), status.session_id());
            state.sessions.insert(status.session_id(), status);
        }
        let mut from = None::<u64>;
        for correlation_id in live.values() {
            let sequence = wake_record(ledger, *correlation_id)?.sequence();
            from = Some(from.map_or(sequence, |from| from.min(sequence)));
        }
        let Some(from) = from else {
            return Ok(state);
        };
        let events = read_event_types_after(
            ledger,
            &AGENT_EVENT_TYPES,
            from.saturating_sub(1),
            rows.through_sequence,
            page_events,
        )
        .map_err(|_| ledger_error(HostLocation::RecoverAgentDispatcher))?;
        for event in &events {
            if wake_of(event).is_some_and(|wake_id| live.contains_key(&wake_id)) {
                state.apply_event(event, None)?;
            }
        }
        Ok(state)
    }

    /// Workflow #422 LN-4: the live wakes, for the seeding of their value records.
    pub(crate) fn live_wakes(&self) -> impl Iterator<Item = &AgentWakeRecord> {
        self.wakes.values()
    }

    /// Workflow #422 LN-4, tests: the sizes of the seven maps.
    #[cfg(test)]
    pub(crate) fn map_sizes_for_test(&self) -> [usize; 7] {
        [
            self.wakes.len(),
            self.wakes_by_trigger.len(),
            self.sessions.len(),
            self.session_by_wake.len(),
            self.active_by_scope.len(),
            self.resume_requests.len(),
            self.response_requests.len(),
        ]
    }

    /// The fold before Workflow #381 R4a, over one whole-ledger read; the reference the paged
    /// recovery is checked against.
    #[cfg(test)]
    pub(crate) fn recover_whole_ledger_for_test(
        ledger: &GlobalLedger,
        instance_ids_by_alias: &BTreeMap<String, InstanceId>,
    ) -> RuntimeHostResult<Self> {
        let events = ledger
            .query(EventQuery::default())
            .map_err(|_| ledger_error(crate::codes::HostLocation::RecoverAgentDispatcher))?;
        Self::fold(&events, &events, instance_ids_by_alias)
    }

    /// Folds the Agent records of `agent_events` in ledger order, checking each wake against
    /// the planning signals of `signal_events`.
    fn fold(
        signal_events: &[PersistedEvent],
        agent_events: &[PersistedEvent],
        instance_ids_by_alias: &BTreeMap<String, InstanceId>,
    ) -> RuntimeHostResult<Self> {
        let source_events = signal_events
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
        // Workflow #422 LN-3: the fold takes the replayed types only, so the whole-ledger
        // reference skips the wake obligations' value records as the paged read does.
        for event in agent_events {
            if AGENT_EVENT_TYPES.contains(&event.event_type()) {
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
        self.wakes.get(&wake_id).ok_or_else(|| {
            request(
                HostCode::AgentWakeUnknown,
                ContractLocation::StartAgentSession,
            )
        })
    }

    pub(crate) fn session(
        &self,
        session_id: AgentSessionId,
    ) -> RuntimeHostResult<&AgentSessionStatus> {
        self.sessions.get(&session_id).ok_or_else(|| {
            request(
                HostCode::AgentSessionUnknown,
                HostLocation::ReadAgentSession,
            )
        })
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
            return Err(fatal(
                HostCode::AgentSessionScopeConflict,
                ContractLocation::StartAgentSession,
            ));
        }
        let status = AgentSessionStatus::started(session_id, wake.data(), started_at_unix_ms)
            .map_err(|_| {
                request(
                    HostCode::AgentSessionInvalid,
                    ContractLocation::StartAgentSession,
                )
            })?;
        let data = AgentSessionEventData::new(status, None).map_err(|_| {
            fatal(
                HostCode::AgentSessionEventInvalid,
                ContractLocation::StartAgentSession,
            )
        })?;
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
                Err(fatal(
                    HostCode::AgentResumeRequestConflict,
                    ContractLocation::ResumeAgentSession,
                ))
            };
        }
        if self.response_requests.contains_key(&request_id) {
            return Err(request(
                HostCode::AgentResumeRequestConflict,
                ContractLocation::ResumeAgentSession,
            ));
        }
        let current = self.session(session_id)?;
        if current.expired_at(observed_at_unix_ms) {
            return Err(request(
                HostCode::AgentSessionExpired,
                ContractLocation::ResumeAgentSession,
            ));
        }
        let status = current.resumed(observed_at_unix_ms).map_err(|_| {
            request(
                SharedCode::AgentSessionNotActive,
                ContractLocation::ResumeAgentSession,
            )
        })?;
        AgentSessionEventData::new(status, None)
            .map(AgentResumePreparation::New)
            .map_err(|_| {
                fatal(
                    HostCode::AgentSessionEventInvalid,
                    ContractLocation::ResumeAgentSession,
                )
            })
    }

    pub(crate) fn prepare_response(
        &self,
        ledger: &GlobalLedger,
        request_id: RequestId,
        response: &AgentSessionResponse,
        runtime_observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<AgentResponsePreparation> {
        if let Some(existing) = self.response_requests.get(&request_id) {
            return if existing.response() == Some(response) {
                Ok(AgentResponsePreparation::Replay(existing.status().clone()))
            } else {
                Err(fatal(
                    HostCode::AgentResponseRequestConflict,
                    ContractLocation::RecordAgentResponse,
                ))
            };
        }
        // Workflow #422 LN-4: the maps keep no request of an ended session, so a resent response
        // of one is answered from the ledger's record of the request.
        if self
            .sessions
            .get(&response.session_id())
            .is_none_or(|session| session.state() != AgentAttentionState::Active)
            && let Some(status) = ended_response_replay(ledger, request_id, response)?
        {
            return Ok(AgentResponsePreparation::Replay(status));
        }
        if self.resume_requests.contains_key(&request_id) {
            return Err(request(
                HostCode::AgentResponseRequestConflict,
                ContractLocation::RecordAgentResponse,
            ));
        }
        let current = self.session(response.session_id())?;
        if current.expired_at(runtime_observed_at_unix_ms) {
            return Err(request(
                HostCode::AgentSessionExpired,
                ContractLocation::RecordAgentResponse,
            ));
        }
        let status = match response.disposition() {
            AgentResponseDisposition::Completed => current
                .completed(runtime_observed_at_unix_ms)
                .map_err(|_| {
                request(
                    SharedCode::AgentSessionNotActive,
                    ContractLocation::RecordAgentResponse,
                )
            })?,
            AgentResponseDisposition::RetryableFailure => current
                .retry_or_escalate(runtime_observed_at_unix_ms)
                .map_err(|_| {
                    request(
                        SharedCode::AgentSessionNotActive,
                        ContractLocation::RecordAgentResponse,
                    )
                })?,
            AgentResponseDisposition::NeedsHuman => current
                .escalated(runtime_observed_at_unix_ms)
                .map_err(|_| {
                    request(
                        SharedCode::AgentSessionNotActive,
                        ContractLocation::RecordAgentResponse,
                    )
                })?,
        };
        let data =
            AgentSessionEventData::new(status.clone(), Some(response.clone())).map_err(|_| {
                fatal(
                    HostCode::AgentSessionEventInvalid,
                    ContractLocation::RecordAgentResponse,
                )
            })?;
        Ok(match status.state() {
            AgentAttentionState::Active => AgentResponsePreparation::Retry(data),
            AgentAttentionState::Completed => AgentResponsePreparation::Complete(data),
            AgentAttentionState::PausedNeedsHuman => AgentResponsePreparation::Escalate(data),
            AgentAttentionState::PausedNeedsAgent => {
                return Err(fatal(
                    HostCode::AgentSessionTransitionInvalid,
                    ContractLocation::RecordAgentResponse,
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
                    actingcommand_contract::outcome::CodeStr::from(HostCode::SessionTimeout.code()),
                    now_unix_ms,
                )
                .map_err(|_| {
                    fatal(
                        HostCode::AgentTimeoutResponseInvalid,
                        HostLocation::ExpireAgentSessions,
                    )
                })?;
                let status = status.escalated(now_unix_ms).map_err(|_| {
                    fatal(
                        HostCode::AgentTimeoutStatusInvalid,
                        HostLocation::ExpireAgentSessions,
                    )
                })?;
                AgentSessionEventData::new(status, Some(response)).map_err(|_| {
                    fatal(
                        HostCode::AgentSessionEventInvalid,
                        HostLocation::ExpireAgentSessions,
                    )
                })
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
        // Workflow #383 register X3: a wake without a trigger sequence projects its own record.
        for sequence in wake
            .data
            .trigger_sequence()
            .into_iter()
            .chain([wake.event_sequence])
        {
            let mut events = ledger
                .project(
                    EventQuery {
                        from_sequence: Some(sequence),
                        to_sequence: Some(sequence),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Normal,
                )
                // #411 A51 (R3): a read the writer did not answer in time fails only the
                // request.
                .map_err(|error| {
                    crate::host::ledger_read_error(&error, HostLocation::ProjectAgentContext)
                })?;
            projection.append(&mut events);
        }
        projection.sort_by_key(|event| event.sequence);
        projection.dedup_by_key(|event| event.sequence);
        AgentSessionContext::new(status, projection).map_err(|_| {
            fatal(
                HostCode::AgentContextInvalid,
                HostLocation::ProjectAgentContext,
            )
        })
    }

    pub(crate) fn apply_event(
        &mut self,
        event: &PersistedEvent,
        source_events: Option<&BTreeMap<actingcommand_contract::EventId, AgentWakeSource>>,
    ) -> RuntimeHostResult<()> {
        let EventPayload::Agent(payload) = event.payload() else {
            return Err(fatal(
                HostCode::AgentEventFamilyInvalid,
                HostLocation::ApplyAgentEvent,
            ));
        };
        match payload {
            AgentPayload::WakeRequested(payload) => {
                let wake = payload.wake();
                if event.links().instance_id() != Some(&wake.instance_id())
                    || event.links().correlation_id() != Some(&wake.correlation_id())
                    || source_events.is_some_and(|events| {
                        // A replayed wake names its planning signal; its sequence is compared
                        // while the record carries it (register X3).
                        wake.trigger_event_id()
                            .and_then(|trigger| events.get(&trigger))
                            .is_none_or(|source| {
                                wake.trigger_sequence()
                                    .is_some_and(|sequence| sequence != source.sequence)
                                    || source.timestamp_unix_ms != wake.requested_at_unix_ms()
                                    || source.instance_id != wake.instance_id()
                                    || source.kind != wake.kind()
                            })
                    })
                {
                    return Err(fatal(
                        HostCode::AgentWakeLinksInvalid,
                        HostLocation::ApplyAgentEvent,
                    ));
                }
                if let Some(existing) = self.wakes.get(&wake.wake_id()) {
                    if existing.data != *wake || existing.event_sequence != event.sequence() {
                        return Err(fatal(
                            HostCode::AgentWakeIdentityConflict,
                            HostLocation::ApplyAgentEvent,
                        ));
                    }
                    return Ok(());
                }
                if let Some(trigger) = wake.trigger_event_id() {
                    if self.wakes_by_trigger.contains_key(&trigger) {
                        return Err(fatal(
                            HostCode::AgentWakeTriggerConflict,
                            HostLocation::ApplyAgentEvent,
                        ));
                    }
                    self.wakes_by_trigger.insert(trigger, wake.wake_id());
                }
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
            // Never replayed (see `AGENT_EVENT_TYPES`): AO-6 loads it from the tagged state.
            AgentPayload::WakeObligationRecorded(_) => {
                return Err(fatal(
                    HostCode::AgentEventFamilyInvalid,
                    HostLocation::ApplyAgentEvent,
                ));
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
            return Err(fatal(
                HostCode::AgentSessionLinksInvalid,
                HostLocation::ApplyAgentEvent,
            ));
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
                    return Err(fatal(
                        HostCode::AgentSessionStartConflict,
                        HostLocation::ApplyAgentEvent,
                    ));
                }
            }
            EventType::AgentSessionResumed => {
                let previous = previous.as_ref().ok_or_else(|| {
                    fatal(
                        HostCode::AgentSessionResumeMissing,
                        HostLocation::ApplyAgentEvent,
                    )
                })?;
                if previous.state() != AgentAttentionState::Active
                    || status.state() != AgentAttentionState::Active
                    || status.attempts_used() != previous.attempts_used()
                {
                    return Err(fatal(
                        HostCode::AgentSessionResumeInvalid,
                        HostLocation::ApplyAgentEvent,
                    ));
                }
            }
            EventType::AgentResponseRecorded => {
                let previous = previous.as_ref().ok_or_else(|| {
                    fatal(
                        HostCode::AgentSessionResponseMissing,
                        HostLocation::ApplyAgentEvent,
                    )
                })?;
                if previous.state() != AgentAttentionState::Active
                    || status.state() != AgentAttentionState::Active
                    || status.attempts_used() != previous.attempts_used().saturating_add(1)
                {
                    return Err(fatal(
                        HostCode::AgentSessionResponseInvalid,
                        HostLocation::ApplyAgentEvent,
                    ));
                }
            }
            EventType::AgentSessionCompleted | EventType::AgentSessionEscalated => {
                let previous = previous.as_ref().ok_or_else(|| {
                    fatal(
                        HostCode::AgentSessionTerminalMissing,
                        HostLocation::ApplyAgentEvent,
                    )
                })?;
                if previous.state() != AgentAttentionState::Active
                    || !matches!(
                        status.state(),
                        AgentAttentionState::Completed | AgentAttentionState::PausedNeedsHuman
                    )
                {
                    return Err(fatal(
                        HostCode::AgentSessionTerminalInvalid,
                        HostLocation::ApplyAgentEvent,
                    ));
                }
            }
            _ => {
                return Err(fatal(
                    HostCode::AgentSessionEventInvalid,
                    HostLocation::ApplyAgentEvent,
                ));
            }
        }
        let resume_request = if event_type == EventType::AgentSessionResumed {
            let request_id = event.links().request_id().copied().ok_or_else(|| {
                fatal(
                    HostCode::AgentResumeRequestMissing,
                    HostLocation::ApplyAgentEvent,
                )
            })?;
            let correlation_id = event.links().correlation_id().copied().ok_or_else(|| {
                fatal(
                    HostCode::AgentResumeCorrelationMissing,
                    HostLocation::ApplyAgentEvent,
                )
            })?;
            if self.resume_requests.contains_key(&request_id)
                || self.response_requests.contains_key(&request_id)
            {
                return Err(fatal(
                    HostCode::AgentResumeRequestConflict,
                    HostLocation::ApplyAgentEvent,
                ));
            }
            Some((request_id, correlation_id))
        } else {
            None
        };
        let response_request_id = if let Some(response) = data.response() {
            let request_id = event.links().request_id().copied().ok_or_else(|| {
                fatal(
                    HostCode::AgentResponseRequestMissing,
                    HostLocation::ApplyAgentEvent,
                )
            })?;
            if self.response_requests.contains_key(&request_id)
                || self.resume_requests.contains_key(&request_id)
            {
                return Err(fatal(
                    HostCode::AgentResponseRequestConflict,
                    HostLocation::ApplyAgentEvent,
                ));
            }
            if response.session_id() != status.session_id() {
                return Err(fatal(
                    SharedCode::AgentResponseSessionMismatch,
                    HostLocation::ApplyAgentEvent,
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
            return Err(fatal(
                HostCode::AgentSessionScopeConflict,
                HostLocation::ApplyAgentEvent,
            ));
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
        self.end_session(status);
        Ok(())
    }

    /// Workflow #422 LN-4 (LEAN-L model §1.2 H-i; `contracts/ledger-observer.md` §11.2: sessions
    /// are not kept): at a session's end its request records leave memory, and a resent request
    /// of it is answered from the ledger (`prepare_response`). A completed session ends its wake's
    /// obligation, so the wake, its trigger and the session leave memory too, and the start loads
    /// no such obligation (`load`). A session escalated to a person keeps its status and its wake
    /// for the person who reads them.
    fn end_session(&mut self, status: &AgentSessionStatus) {
        let session_id = status.session_id();
        match status.state() {
            AgentAttentionState::Completed => {
                let wake_id = status.wake_id();
                if let Some(wake) = self.wakes.remove(&wake_id)
                    && let Some(trigger) = wake.data.trigger_event_id()
                {
                    self.wakes_by_trigger.remove(&trigger);
                }
                self.sessions.remove(&session_id);
                self.session_by_wake.remove(&wake_id);
            }
            AgentAttentionState::PausedNeedsHuman => {}
            AgentAttentionState::Active | AgentAttentionState::PausedNeedsAgent => return,
        }
        self.active_by_scope
            .retain(|_, active| *active != session_id);
        self.resume_requests
            .retain(|_, resume| resume.session_id != session_id);
        self.response_requests
            .retain(|_, response| response.status().session_id() != session_id);
    }
}

/// Workflow #422 LN-4: the wake's own record, by the correlation link it shares with the wake
/// (`apply_event` checks it); exactly one.
fn wake_record(
    ledger: &GlobalLedger,
    correlation_id: CorrelationId,
) -> RuntimeHostResult<PersistedEvent> {
    let mut requested = ledger
        .query(EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error(HostLocation::RecoverAgentDispatcher))?;
    match (requested.pop(), requested.is_empty()) {
        (Some(requested), true) => Ok(requested),
        _ => Err(fatal(
            HostCode::AgentWakeIdentityConflict,
            HostLocation::RecoverAgentDispatcher,
        )),
    }
}

/// Workflow #422 LN-4: the wake an Agent record belongs to.
fn wake_of(event: &PersistedEvent) -> Option<AgentWakeId> {
    match event.payload() {
        EventPayload::Agent(AgentPayload::WakeRequested(payload)) => Some(payload.wake().wake_id()),
        EventPayload::Agent(
            AgentPayload::SessionStarted(payload)
            | AgentPayload::SessionResumed(payload)
            | AgentPayload::ResponseRecorded(payload)
            | AgentPayload::SessionCompleted(payload)
            | AgentPayload::SessionEscalated(payload),
        ) => Some(payload.session().status().wake_id()),
        _ => None,
    }
}

/// Workflow #422 LN-4: the status the ledger recorded for the response request `request_id` of
/// an ended session, if it recorded `response` under it; another response under the same request
/// is a conflict.
fn ended_response_replay(
    ledger: &GlobalLedger,
    request_id: RequestId,
    response: &AgentSessionResponse,
) -> RuntimeHostResult<Option<AgentSessionStatus>> {
    let recorded = ledger
        .query(EventQuery {
            request_id: Some(request_id),
            event_types: vec![
                EventType::AgentResponseRecorded,
                EventType::AgentSessionCompleted,
                EventType::AgentSessionEscalated,
            ],
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error(ContractLocation::RecordAgentResponse))?;
    for event in &recorded {
        let EventPayload::Agent(
            AgentPayload::ResponseRecorded(payload)
            | AgentPayload::SessionCompleted(payload)
            | AgentPayload::SessionEscalated(payload),
        ) = event.payload()
        else {
            continue;
        };
        return if payload.session().response() == Some(response) {
            Ok(Some(payload.session().status().clone()))
        } else {
            Err(fatal(
                HostCode::AgentResponseRequestConflict,
                ContractLocation::RecordAgentResponse,
            ))
        };
    }
    Ok(None)
}

/// The planning signals and the Agent records through one ledger position, each in ledger order.
fn read_recovery_events(
    ledger: &GlobalLedger,
    page_events: usize,
) -> GlobalLedgerResult<(Vec<PersistedEvent>, Vec<PersistedEvent>)> {
    let through = ledger.latest_sequence()?;
    let signals = read_event_types(
        ledger,
        &[EventType::PolicyPlanningSignalObserved],
        through,
        page_events,
    )?;
    let agent_events = read_event_types(ledger, &AGENT_EVENT_TYPES, through, page_events)?;
    Ok((signals, agent_events))
}

fn request(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

fn fatal(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeHalted)
}

fn ledger_error(operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::AgentLedgerFailure,
        operation,
        RuntimeErrorCode::LedgerFailure,
    )
}
