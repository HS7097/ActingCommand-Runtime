// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn expire_agent_sessions(&self) -> RuntimeHostResult<()> {
        if self.agent_dispatcher_config.is_none() {
            return Ok(());
        }
        let now_unix_ms = unix_ms_now()?;
        let _gate = lock(&self.agent_write_gate, "expire_agent_sessions")?;
        let expired =
            lock(&self.agent_dispatcher, "expire_agent_sessions")?.expired_sessions(now_unix_ms)?;
        for data in expired {
            let links = self
                .events
                .issuer()
                .issue_agent_session_links(data.status().instance_id())
                .map_err(|_| runtime_identifier_error())?;
            let persisted = self.append_event_raw(
                EventSeverity::Warning,
                EventSource::System,
                OriginModule::AgentDispatcher,
                EventActor::Runtime,
                links.event_links(),
                AgentPayloadDraft::session_escalated(data, AuditInput::new()),
            )?;
            lock(&self.agent_dispatcher, "commit_agent_timeout")?.apply_event(&persisted, None)?;
        }
        Ok(())
    }

    pub(super) fn start_agent_session(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        wake_id: AgentWakeId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher("start_agent_session")?;
        let _gate = lock(&self.agent_write_gate, "start_agent_session")
            .map_err(RequestFailure::poison_without_terminal)?;
        let session_id = self
            .events
            .issuer()
            .mint_agent_session_id()
            .map(|issued| *issued.transport())
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let started_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let preparation = lock(&self.agent_dispatcher, "start_agent_session")?
            .prepare_start(wake_id, session_id, started_at_unix_ms)
            .map_err(agent_request_failure)?;
        let (status, terminal) = match preparation {
            AgentSessionPreparation::Replay(status) => (status, None),
            AgentSessionPreparation::New(data) => {
                let persisted = self.append_event(
                    EventSeverity::Info,
                    request.source(),
                    OriginModule::AgentDispatcher,
                    request.actor(),
                    validated.event_links(Some(data.status().instance_id()), None, None),
                    AgentPayloadDraft::session_started(data.clone(), AuditInput::new()),
                )?;
                lock(&self.agent_dispatcher, "commit_agent_session_start")?
                    .apply_event(&persisted, None)
                    .map_err(RequestFailure::poison_without_terminal)?;
                (data.status().clone(), Some(terminal(&persisted)))
            }
        };
        let context = self.agent_session_context(status)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal,
            result: RuntimeResult::AgentSessionOpened {
                context: Box::new(context),
            },
        })
    }

    pub(super) fn resume_agent_session(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        session_id: AgentSessionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher("resume_agent_session")?;
        let _gate = lock(&self.agent_write_gate, "resume_agent_session")
            .map_err(RequestFailure::poison_without_terminal)?;
        let observed_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let preparation = lock(&self.agent_dispatcher, "resume_agent_session")?
            .prepare_resume(
                request.request_id(),
                request.correlation_id(),
                session_id,
                observed_at_unix_ms,
            )
            .map_err(agent_request_failure)?;
        let (status, terminal) = match preparation {
            AgentResumePreparation::Replay { status, terminal } => (status, terminal),
            AgentResumePreparation::New(data) => {
                let persisted = self.append_event(
                    EventSeverity::Info,
                    request.source(),
                    OriginModule::AgentDispatcher,
                    request.actor(),
                    validated.event_links(Some(data.status().instance_id()), None, None),
                    AgentPayloadDraft::session_resumed(data.clone(), AuditInput::new()),
                )?;
                lock(&self.agent_dispatcher, "commit_agent_session_resume")?
                    .apply_event(&persisted, None)
                    .map_err(RequestFailure::poison_without_terminal)?;
                (data.status().clone(), terminal(&persisted))
            }
        };
        let context = self.agent_session_context(status)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal),
            result: RuntimeResult::AgentSessionObserved {
                context: Box::new(context),
            },
        })
    }

    pub(super) fn agent_session_status(
        &self,
        session_id: AgentSessionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher("read_agent_session")?;
        let status = lock(&self.agent_dispatcher, "read_agent_session")?
            .session(session_id)
            .map_err(agent_request_failure)?
            .clone();
        let context = self.agent_session_context(status)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::AgentSessionObserved {
                context: Box::new(context),
            },
        })
    }

    pub(super) fn record_agent_response(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        response: &AgentSessionResponse,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher("record_agent_response")?;
        let _gate = lock(&self.agent_write_gate, "record_agent_response")
            .map_err(RequestFailure::poison_without_terminal)?;
        let observed_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let preparation = lock(&self.agent_dispatcher, "record_agent_response")?
            .prepare_response(request.request_id(), response, observed_at_unix_ms)
            .map_err(agent_request_failure)?;
        let (data, payload, severity) = match preparation {
            AgentResponsePreparation::Replay(status) => {
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::AgentResponseRecorded { status },
                });
            }
            AgentResponsePreparation::Retry(data) => {
                let payload = AgentPayloadDraft::response_recorded(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Warning)
            }
            AgentResponsePreparation::Complete(data) => {
                let payload = AgentPayloadDraft::session_completed(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Info)
            }
            AgentResponsePreparation::Escalate(data) => {
                let payload = AgentPayloadDraft::session_escalated(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Warning)
            }
        };
        let persisted = self.append_event(
            severity,
            request.source(),
            OriginModule::AgentDispatcher,
            request.actor(),
            validated.event_links(Some(data.status().instance_id()), None, None),
            payload,
        )?;
        lock(&self.agent_dispatcher, "commit_agent_response")?
            .apply_event(&persisted, None)
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::AgentResponseRecorded {
                status: data.status().clone(),
            },
        })
    }

    fn agent_session_context(
        &self,
        status: AgentSessionStatus,
    ) -> Result<AgentSessionContext, RequestFailure> {
        lock(&self.agent_dispatcher, "project_agent_session")?
            .context(&self.ledger, status)
            .map_err(agent_request_failure)
    }

    fn require_agent_dispatcher(&self, operation: &'static str) -> Result<(), RequestFailure> {
        self.agent_dispatcher_config
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| {
                agent_request_failure(RuntimeHostError::request(
                    "agent_dispatcher_disabled",
                    operation,
                    RuntimeErrorCode::InvalidRequest,
                ))
            })
    }
}

pub(super) fn append_agent_wake(
    state: &mut AgentDispatcherState,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    config: &AgentDispatcherConfig,
    source: &PersistedEvent,
    instance_id: InstanceId,
    kind: AgentWakeKind,
) -> RuntimeHostResult<PersistedEvent> {
    let issued = events
        .issuer()
        .issue_agent_wake(
            AgentWakeTrigger::new(
                instance_id,
                kind,
                *source.event_id(),
                source.sequence(),
                source.timestamp_unix_ms(),
            )
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "agent_wake_trigger_invalid",
                    "append_agent_wake",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?,
            config.budget(),
            config.capabilities().clone(),
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                "agent_wake_issue_failed",
                "append_agent_wake",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
    let draft = events.draft(
        EventSeverity::Info,
        EventSource::Scheduler,
        OriginModule::AgentDispatcher,
        EventActor::Runtime,
        issued.event_links(),
        AgentPayloadDraft::wake_requested(issued.data().clone(), AuditInput::new()),
    )?;
    let persisted = ledger
        .append(events.sanitize(draft)?)
        .map_err(|_| ledger_error("append_agent_wake"))?;
    state.apply_event(&persisted, None)?;
    Ok(persisted)
}

fn agent_request_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}
