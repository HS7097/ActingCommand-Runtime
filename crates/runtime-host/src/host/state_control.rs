// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_ledger::{
    GlobalLedgerError, LedgerTransactionWork, TransactionStateObservation, TransactionWorkError,
};
use actingcommand_runtime_state::{
    PreparedReleaseState, RELEASE_BASELINE_STATE_KEY, ReleaseBaselineMember,
    ReleaseStateObservation,
};

impl HostShared {
    pub(super) fn stage_release_set(
        &self,
        manifest: RuntimeReleaseSet,
        sources: &ReleaseArtifactSources,
    ) -> RuntimeHostResult<RuntimeReleaseSet> {
        let result = (|| {
            let _gate = lock(&self.state_write_gate, "stage_release_set")?;
            if let Some(error) = self.fatal.current()? {
                return Err(error);
            }
            let work = self
                .state
                .prepare_release_stage(&manifest, sources)
                .map_err(release_state_error)?;
            if let Some(work) = work {
                let links = self.events.system_links()?;
                let draft = self.events.draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links.clone(),
                    ReleasePayloadDraft::staged(manifest.clone(), AuditInput::new()),
                )?;
                let draft = self.events.sanitize(draft)?;
                let fact_gate = lock(&self.fact_write_gate, "stage_release_set")?;
                if self.lifecycle_append_failed.load(Ordering::Acquire) {
                    return Err(ledger_error("stage_release_set"));
                }
                let outcome = self
                    .ledger
                    .append_transaction(draft, Box::new(ReleaseTransaction { state: Ok(work) }))
                    .map_err(release_transaction_error)?;
                self.observe_device_diagnostics_under_fact_gate(&outcome, &links)?;
                self.synchronize_fact_store_under_gate()?;
                drop(fact_gate);
                self.observe_pipeline_event(&outcome)?;
            }
            Ok(manifest)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    pub(super) fn active_release_set(&self) -> RuntimeHostResult<Option<RuntimeReleaseSet>> {
        let result = self
            .state
            .active_release()
            .map(|active| active.map(|release| release.manifest().clone()))
            .map_err(release_state_error);
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    pub(super) fn switch_release_set(
        &self,
        kind: ReleaseTransitionKind,
        release_id: &str,
    ) -> RuntimeHostResult<RuntimeReleaseSet> {
        let result = (|| {
            let _gate = lock(&self.state_write_gate, "switch_release_set")?;
            if let Some(error) = self.fatal.current()? {
                return Err(error);
            }
            self.state
                .verify_release_atomic_ready()
                .map_err(release_state_error)?;
            let preview = self
                .state
                .preview_release_transition(kind, release_id)
                .map_err(release_state_error)?;
            let transition = preview.data().clone();
            let manifest = self
                .state
                .release_generation_manifest(release_id)
                .map_err(release_state_error)?;
            let links = self.events.system_links()?;
            let draft = self.events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links.clone(),
                ReleasePayloadDraft::transition_intent(transition.clone(), AuditInput::new()),
            )?;
            let target = match kind {
                ReleaseTransitionKind::Activate => ReleaseTransitionTarget::Activated,
                ReleaseTransitionKind::Rollback => ReleaseTransitionTarget::RolledBack,
            };
            let plan = CriticalEventPlan::new(
                CriticalOperation::ReleaseTransition(target),
                self.events.sanitize(draft)?,
            )
            .map_err(|_| critical_plan_error())?;
            let intent = self
                .ledger
                .append(plan.intent().clone())
                .map_err(release_ledger_error)?;
            let state = self
                .state
                .prepare_release_transition(&preview, &intent)
                .map_err(release_work_error);
            let success = self.events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links.clone(),
                transition_payload(&transition),
            )?;
            let success = self.events.sanitize(success)?;
            actingcommand_ledger::critical::validate_release_outcome(
                target, &intent, &success, true,
            )
            .map_err(|_| critical_plan_error())?;
            let outcome = match self
                .ledger
                .append_transaction(success, Box::new(ReleaseTransaction { state }))
            {
                Ok(event) => event,
                Err(error) => {
                    let Some(rejection) = error.rolled_back_work() else {
                        return Err(release_ledger_error(error));
                    };
                    let original = release_rejection_error(rejection);
                    let failed = self
                        .events
                        .draft(
                            EventSeverity::Error,
                            EventSource::Runtime,
                            OriginModule::Runtime,
                            EventActor::Runtime,
                            links,
                            ReleasePayloadDraft::transition_failed(
                                transition,
                                EffectDisposition::NotPerformed,
                                AuditInput::new(),
                            ),
                        )
                        .and_then(|draft| self.events.sanitize(draft))
                        .map_err(|error| release_failure_error(&original, error))?;
                    actingcommand_ledger::critical::validate_release_outcome(
                        target, &intent, &failed, false,
                    )
                    .map_err(|error| release_failure_error(&original, format!("{error:?}")))?;
                    let failed = self.ledger.append(failed).map_err(|error| {
                        release_failure_error(
                            &original,
                            format!("{error}; detail={:?}", error.detail()),
                        )
                    })?;
                    self.synchronize_fact_store()
                        .and_then(|()| self.observe_pipeline_event(&failed))
                        .map_err(|error| release_failure_error(&original, error))?;
                    return Err(original);
                }
            };
            self.synchronize_fact_store()
                .and_then(|()| self.observe_pipeline_event(&outcome))
                .map_err(RuntimeHostError::into_fatal)?;
            Ok(manifest)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }
}

struct ReleaseTransaction {
    state: Result<PreparedReleaseState, TransactionWorkError>,
}

impl LedgerTransactionWork for ReleaseTransaction {
    fn apply(
        &self,
        scope: &actingcommand_runtime_database::RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> Result<(), TransactionWorkError> {
        self.state
            .as_ref()
            .map_err(Clone::clone)?
            .apply(scope, event)
            .map_err(release_work_error)
    }

    fn observe(
        &self,
        scope: &actingcommand_runtime_database::RuntimeTransaction<'_, '_>,
    ) -> Result<TransactionStateObservation, TransactionWorkError> {
        self.state
            .as_ref()
            .map_err(Clone::clone)?
            .observe(scope)
            .map(|value| match value {
                ReleaseStateObservation::Applied => TransactionStateObservation::Applied,
                ReleaseStateObservation::Unchanged => TransactionStateObservation::Unchanged,
                ReleaseStateObservation::Unknown => TransactionStateObservation::Unknown,
            })
            .map_err(release_work_error)
    }
}

pub(super) fn reconcile_release_state(
    state: &Arc<RuntimeStateStore>,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
) -> RuntimeHostResult<()> {
    let boundary = find_release_fact(
        ledger,
        EventType::StateMigrated,
        |event| matches!(event.payload(), EventPayload::State(StatePayload::Migrated(value)) if value.migration().state_key() == RELEASE_BASELINE_STATE_KEY),
    )?;
    if let Some(work) = state
        .prepare_release_boundary(boundary.as_ref())
        .map_err(release_state_error)?
    {
        let migration = work
            .migration()
            .ok_or_else(|| release_recovery_error("release_boundary_migration_missing"))?
            .clone();
        let draft = release_draft(
            events,
            StatePayloadDraft::migrated(migration, AuditInput::new()),
        )?;
        ledger
            .append_transaction(draft, Box::new(ReleaseTransaction { state: Ok(work) }))
            .map_err(release_transaction_error)?;
    }
    for member in state
        .release_baseline_members()
        .map_err(release_state_error)?
    {
        let (event_type, identity) = match &member {
            ReleaseBaselineMember::Generation { release_id, .. } => {
                (EventType::ReleaseStaged, release_id.as_str())
            }
            ReleaseBaselineMember::Transition(data) => (
                match data.kind() {
                    ReleaseTransitionKind::Activate => EventType::ReleaseActivated,
                    ReleaseTransitionKind::Rollback => EventType::ReleaseRolledBack,
                },
                data.transition_id(),
            ),
            ReleaseBaselineMember::Pointer { .. } => {
                return Err(release_recovery_error("release_recovery_member_invalid"));
            }
        };
        let fact = find_release_fact(ledger, event_type, |event| {
            release_fact_identity(event) == Some(identity)
        })?;
        if let Some(work) = state
            .prepare_release_legacy_member(&member, fact.as_ref())
            .map_err(release_state_error)?
        {
            let payload = match &member {
                ReleaseBaselineMember::Generation { release_id, .. } => {
                    ReleasePayloadDraft::staged(
                        state
                            .release_generation_manifest(release_id)
                            .map_err(release_state_error)?,
                        AuditInput::new(),
                    )
                }
                ReleaseBaselineMember::Transition(data) => {
                    transition_payload(&data.recovered_for_ledger())
                }
                ReleaseBaselineMember::Pointer { .. } => {
                    return Err(release_recovery_error("release_recovery_member_invalid"));
                }
            };
            ledger
                .append_transaction(
                    release_draft(events, payload)?,
                    Box::new(ReleaseTransaction { state: Ok(work) }),
                )
                .map_err(release_transaction_error)?;
        }
    }
    state
        .verify_release_atomic_ready()
        .map_err(release_state_error)?;
    for event_type in [
        EventType::ReleaseStaged,
        EventType::ReleaseActivated,
        EventType::ReleaseRolledBack,
    ] {
        visit_release_facts(ledger, event_type, |event| {
            state
                .verify_release_committed_fact(event)
                .map_err(release_state_error)
        })?;
    }
    visit_release_facts(ledger, EventType::ReleaseTransitionIntent, |intent| {
        let EventPayload::Release(ReleasePayload::TransitionIntent(value)) = intent.payload()
        else {
            return Err(release_recovery_error("release_intent_payload_invalid"));
        };
        if intent.origin().source() != EventSource::Runtime
            || intent.origin().module() != OriginModule::Runtime
            || intent.origin().actor() != EventActor::Runtime
        {
            return Err(release_recovery_error("release_intent_origin_invalid"));
        }
        let expected_type = match value.transition().kind() {
            ReleaseTransitionKind::Activate => EventType::ReleaseActivated,
            ReleaseTransitionKind::Rollback => EventType::ReleaseRolledBack,
        };
        let outcome = find_release_fact(ledger, expected_type, |event| {
            release_fact_identity(event) == Some(value.transition().transition_id())
        })?;
        let failure = find_release_fact(ledger, EventType::ReleaseTransitionFailed, |event| {
            release_fact_identity(event) == Some(value.transition().transition_id())
                && event.links().correlation_id() == intent.links().correlation_id()
                && event.links().action_id() == intent.links().action_id()
        })?;
        let outcome = outcome.filter(|event| {
            (event.links().correlation_id() == intent.links().correlation_id()
                && event.links().action_id() == intent.links().action_id())
                || (failure.is_none() && matches!(event.payload(),
                    EventPayload::Release(ReleasePayload::Activated(outcome) | ReleasePayload::RolledBack(outcome))
                    if outcome.transition() == &value.transition().recovered_for_ledger()))
        });
        match (outcome, failure) {
            (Some(outcome), None) => {
                if outcome.sequence() <= intent.sequence() {
                    return Err(release_recovery_error(
                        "release_intent_outcome_order_invalid",
                    ));
                }
                let EventPayload::Release(
                    ReleasePayload::Activated(data) | ReleasePayload::RolledBack(data),
                ) = outcome.payload()
                else {
                    return Err(release_recovery_error("release_outcome_payload_invalid"));
                };
                if data.transition() == value.transition() {
                    let target = match value.transition().kind() {
                        ReleaseTransitionKind::Activate => ReleaseTransitionTarget::Activated,
                        ReleaseTransitionKind::Rollback => ReleaseTransitionTarget::RolledBack,
                    };
                    actingcommand_ledger::critical::validate_release_transaction_outcome(
                        target, intent, &outcome, true,
                    )
                    .map_err(|_| release_recovery_error("release_outcome_identity_mismatch"))?;
                } else if data.transition() != &value.transition().recovered_for_ledger() {
                    return Err(release_recovery_error("release_outcome_identity_mismatch"));
                }
                state
                    .verify_release_committed_fact(&outcome)
                    .map_err(release_state_error)
            }
            (None, Some(failure)) => {
                if failure.sequence() <= intent.sequence()
                    || failure.origin() != intent.origin()
                    || failure.payload().effect_disposition()
                        != Some(EffectDisposition::NotPerformed)
                    || !matches!(failure.payload(), EventPayload::Release(ReleasePayload::TransitionFailed(failed)) if failed.transition() == value.transition())
                {
                    return Err(release_recovery_error(
                        "release_transition_effect_unresolved",
                    ));
                }
                let target = match value.transition().kind() {
                    ReleaseTransitionKind::Activate => ReleaseTransitionTarget::Activated,
                    ReleaseTransitionKind::Rollback => ReleaseTransitionTarget::RolledBack,
                };
                actingcommand_ledger::critical::validate_release_transaction_outcome(
                    target, intent, &failure, false,
                )
                .map_err(|_| release_recovery_error("release_failure_identity_mismatch"))?;
                Ok(())
            }
            _ => Err(release_recovery_error(
                "release_transition_effect_unresolved",
            )),
        }
    })
}

fn visit_release_facts(
    ledger: &GlobalLedger,
    event_type: EventType,
    mut visit: impl FnMut(&PersistedEvent) -> RuntimeHostResult<()>,
) -> RuntimeHostResult<()> {
    let through = ledger.latest_sequence().map_err(release_ledger_error)?;
    let mut after = 0;
    loop {
        let page = ledger
            .query_page(
                EventQuery {
                    event_type: Some(event_type),
                    ..EventQuery::default()
                },
                after,
                through,
                256,
            )
            .map_err(release_ledger_error)?;
        if page.is_empty() {
            return Ok(());
        }
        for event in &page {
            if event.sequence() <= after || event.sequence() > through {
                return Err(release_recovery_error("release_recovery_cursor_invalid"));
            }
            visit(event)?;
            after = event.sequence();
        }
    }
}

fn find_release_fact(
    ledger: &GlobalLedger,
    event_type: EventType,
    matches: impl Fn(&PersistedEvent) -> bool,
) -> RuntimeHostResult<Option<PersistedEvent>> {
    let mut found = None;
    visit_release_facts(ledger, event_type, |event| {
        if matches(event) {
            if found.is_some() {
                return Err(release_recovery_error("release_fact_identity_conflict"));
            }
            found = Some(event.clone());
        }
        Ok(())
    })?;
    Ok(found)
}

fn release_fact_identity(event: &PersistedEvent) -> Option<&str> {
    match event.payload() {
        EventPayload::Release(ReleasePayload::Staged(value)) => Some(value.manifest().release_id()),
        EventPayload::Release(
            ReleasePayload::Activated(value)
            | ReleasePayload::RolledBack(value)
            | ReleasePayload::TransitionFailed(value),
        ) => Some(value.transition().transition_id()),
        _ => None,
    }
}

fn transition_payload(data: &actingcommand_contract::ReleaseTransitionData) -> ReleasePayloadDraft {
    match data.kind() {
        ReleaseTransitionKind::Activate => {
            ReleasePayloadDraft::activated(data.clone(), AuditInput::new())
        }
        ReleaseTransitionKind::Rollback => {
            ReleasePayloadDraft::rolled_back(data.clone(), AuditInput::new())
        }
    }
}

fn release_draft(
    events: &RuntimeEvents,
    payload: impl Into<actingcommand_contract::EventPayloadDraft>,
) -> RuntimeHostResult<actingcommand_contract::SanitizedEventDraft> {
    let draft = events.draft(
        EventSeverity::Info,
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
        events.system_links()?,
        payload,
    )?;
    events.sanitize(draft)
}

fn release_state_error(error: actingcommand_runtime_state::RuntimeStateError) -> RuntimeHostError {
    RuntimeHostError::state(&error).with_native_detail(error.to_string())
}

fn release_work_error(
    error: actingcommand_runtime_state::RuntimeStateError,
) -> TransactionWorkError {
    TransactionWorkError {
        code: error.code(),
        operation: error.operation(),
        fatal: error.is_fatal(),
        detail: error.to_string(),
    }
}

fn release_rejection_error(error: &TransactionWorkError) -> RuntimeHostError {
    let mapped = if error.fatal {
        RuntimeHostError::fatal(error.code, error.operation, RuntimeErrorCode::RuntimeFatal)
    } else {
        RuntimeHostError::request(
            error.code,
            error.operation,
            RuntimeErrorCode::InvalidRequest,
        )
    };
    mapped.with_native_detail(error.detail.clone())
}

fn release_transaction_error(error: GlobalLedgerError) -> RuntimeHostError {
    if let Some(work) = error.rolled_back_work() {
        release_rejection_error(work)
    } else {
        release_ledger_error(error)
    }
}

fn release_ledger_error(error: GlobalLedgerError) -> RuntimeHostError {
    RuntimeHostError::fatal(
        error.code(),
        error.operation(),
        RuntimeErrorCode::LedgerFailure,
    )
    .with_native_detail(format!("{error}; detail={:?}", error.detail()))
}

fn release_failure_error(
    original: &RuntimeHostError,
    failure: impl std::fmt::Display,
) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "release_failure_fact_undurable",
        "switch_release_set",
        RuntimeErrorCode::LedgerFailure,
    )
    .with_native_detail(format!("original={original:?}; failure={failure}"))
}

fn release_recovery_error(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        code,
        "reconcile_release_state",
        RuntimeErrorCode::RuntimeFatal,
    )
}
