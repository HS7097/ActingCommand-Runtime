use super::{
    CriticalActionReport, CriticalEventPlan, CriticalExecutionError, CriticalOperation,
    CriticalOutcomeStage, CriticalPlanError, DefiniteEffectDisposition, EventAppender,
    LeaseTransitionTarget, TaskTerminalTarget, execute_critical,
};
use crate::{GlobalLedgerError, GlobalLedgerResult, PersistedEvent};
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, DiagnosticCode, EffectDisposition, EventAction, EventActor,
    EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft, EventSeverity, EventSource,
    EventType, IdentifierIssuer, InputPayloadDraft, IssuedActionId, IssuedCorrelationId,
    IssuedEventId, IssuedInstanceId, IssuedLeaseId, IssuedRequestId, IssuedRunId, IssuedTaskId,
    LeasePayloadDraft, LeasePriority, OriginModule, SanitizationError, SanitizedEventDraft,
    SecretField, SecretFingerprinter, Sha256Fingerprint, TaskPayloadDraft,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn identifiers() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn event_id(value: &str) -> IssuedEventId {
    static IDS: OnceLock<Mutex<HashMap<String, IssuedEventId>>> = OnceLock::new();
    let mut ids = IDS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("event id registry");
    *ids.entry(value.to_string())
        .or_insert_with(|| identifiers().mint_event_id().expect("event id"))
}

macro_rules! cached_identifier {
    ($function:ident, $type:ty, $mint:ident, $label:literal) => {
        fn $function(value: &str) -> $type {
            static IDS: OnceLock<Mutex<HashMap<String, $type>>> = OnceLock::new();
            let mut ids = IDS
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .expect(concat!($label, " registry"));
            *ids.entry(value.to_string())
                .or_insert_with(|| identifiers().$mint().expect($label))
        }
    };
}

cached_identifier!(
    correlation_id,
    IssuedCorrelationId,
    mint_correlation_id,
    "correlation id"
);
cached_identifier!(action_id, IssuedActionId, mint_action_id, "action id");
cached_identifier!(
    instance_id,
    IssuedInstanceId,
    mint_instance_id,
    "instance id"
);
cached_identifier!(request_id, IssuedRequestId, mint_request_id, "request id");
cached_identifier!(task_id, IssuedTaskId, mint_task_id, "task id");
cached_identifier!(run_id, IssuedRunId, mint_run_id, "run id");
cached_identifier!(lease_id, IssuedLeaseId, mint_lease_id, "lease id");

fn fingerprinter() -> crate::Sha256SecretFingerprinter {
    crate::Sha256SecretFingerprinter::new(b"critical-test-private-salt").expect("fingerprinter")
}

struct FailingFingerprinter;

impl SecretFingerprinter for FailingFingerprinter {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        Err(SanitizationError::fingerprinter_failure())
    }
}

fn links() -> EventLinksDraft {
    EventLinksDraft::default()
        .with_correlation_id(correlation_id("correlation-1"))
        .with_action_id(action_id("action-1"))
        .with_instance_id(instance_id("instance-1"))
        .with_request_id(request_id("request-1"))
        .with_task_id(task_id("task-1"))
        .with_run_id(run_id("run-1"))
        .with_lease_id(lease_id("lease-1"))
}

fn raw_event_at(
    label: &str,
    timestamp_unix_ms: u64,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) -> EventDraft {
    EventDraft::new(
        event_id(label),
        timestamp_unix_ms,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        links,
        payload,
    )
}

fn raw_event(label: &str, links: EventLinksDraft, payload: EventPayloadDraft) -> EventDraft {
    raw_event_at(label, 1_752_147_200_000, links, payload)
}

fn sanitize(draft: EventDraft) -> SanitizedEventDraft {
    draft.sanitize(&fingerprinter()).expect("sanitize")
}

fn input_plan(label: &str) -> CriticalEventPlan {
    CriticalEventPlan::new(
        CriticalOperation::DeviceWrite,
        sanitize(raw_event(
            &format!("{label}-intent"),
            links(),
            InputPayloadDraft::intent(EventAction::InputTap, AuditInput::new()).into(),
        )),
    )
    .expect("input plan")
}

fn input_success(
    label: &str,
    effect: EffectDisposition,
    outcome_links: EventLinksDraft,
    audit: AuditInput,
) -> EventDraft {
    raw_event(
        &format!("{label}-success"),
        outcome_links,
        InputPayloadDraft::committed(EventAction::InputTap, effect, audit).into(),
    )
}

fn input_failure(
    label: &str,
    effect: EffectDisposition,
    outcome_links: EventLinksDraft,
    audit: AuditInput,
) -> EventDraft {
    raw_event(
        &format!("{label}-failure"),
        outcome_links,
        InputPayloadDraft::failed(
            EventAction::InputTap,
            DiagnosticCode::InputFailed,
            effect,
            audit,
        )
        .into(),
    )
}

fn command_plan(label: &str) -> CriticalEventPlan {
    CriticalEventPlan::new(
        CriticalOperation::CommandValidation,
        sanitize(raw_event(
            &format!("{label}-intent"),
            links(),
            CommandPayloadDraft::received(EventAction::CriticalTest, AuditInput::new()).into(),
        )),
    )
    .expect("command plan")
}

fn command_success(label: &str, effect: EffectDisposition) -> EventDraft {
    raw_event(
        &format!("{label}-success"),
        links(),
        CommandPayloadDraft::validated(EventAction::CriticalTest, effect, AuditInput::new()).into(),
    )
}

fn command_failure(label: &str, effect: EffectDisposition) -> EventDraft {
    raw_event(
        &format!("{label}-failure"),
        links(),
        CommandPayloadDraft::rejected(
            EventAction::CriticalTest,
            DiagnosticCode::CommandRejected,
            effect,
            AuditInput::new(),
        )
        .into(),
    )
}

fn lease_plan(label: &str, target: LeaseTransitionTarget) -> CriticalEventPlan {
    CriticalEventPlan::new(
        CriticalOperation::LeaseTransition(target),
        sanitize(raw_event(
            &format!("{label}-intent"),
            links(),
            LeasePayloadDraft::transition_intent(EventAction::CriticalTest, AuditInput::new())
                .into(),
        )),
    )
    .expect("lease plan")
}

fn lease_success(
    label: &str,
    target: LeaseTransitionTarget,
    effect: EffectDisposition,
) -> EventDraft {
    let payload = match target {
        LeaseTransitionTarget::Granted => {
            LeasePayloadDraft::granted(EventAction::CriticalTest, effect, AuditInput::new())
        }
        LeaseTransitionTarget::Transferred => {
            let ids = identifiers();
            LeasePayloadDraft::transferred(
                EventAction::CriticalTest,
                effect,
                *ids.mint_holder_id().expect("from holder").transport(),
                *ids.mint_lease_id().expect("from lease").transport(),
                *ids.mint_holder_id().expect("to holder").transport(),
                *ids.mint_lease_id().expect("to lease").transport(),
                *ids.mint_request_id().expect("queued request").transport(),
                LeasePriority::High,
                AuditInput::new(),
            )
        }
        LeaseTransitionTarget::Renewed => {
            LeasePayloadDraft::renewed(EventAction::CriticalTest, effect, AuditInput::new())
        }
        LeaseTransitionTarget::Released => {
            LeasePayloadDraft::released(EventAction::CriticalTest, effect, AuditInput::new())
        }
        LeaseTransitionTarget::Expired => {
            LeasePayloadDraft::expired(EventAction::CriticalTest, effect, AuditInput::new())
        }
    };
    raw_event(&format!("{label}-success"), links(), payload.into())
}

fn lease_failure(label: &str, effect: EffectDisposition) -> EventDraft {
    raw_event(
        &format!("{label}-failure"),
        links(),
        LeasePayloadDraft::transition_failed(
            EventAction::CriticalTest,
            DiagnosticCode::RuntimeDiagnostic,
            effect,
            AuditInput::new(),
        )
        .into(),
    )
}

fn task_plan(label: &str, target: TaskTerminalTarget) -> CriticalEventPlan {
    CriticalEventPlan::new(
        CriticalOperation::TaskTerminal(target),
        sanitize(raw_event(
            &format!("{label}-intent"),
            links(),
            TaskPayloadDraft::terminal_intent(EventAction::CriticalTest, AuditInput::new()).into(),
        )),
    )
    .expect("task plan")
}

fn task_success(label: &str, target: TaskTerminalTarget, effect: EffectDisposition) -> EventDraft {
    let payload = match target {
        TaskTerminalTarget::Completed => {
            TaskPayloadDraft::completed(EventAction::CriticalTest, effect, AuditInput::new())
        }
        TaskTerminalTarget::Failed => TaskPayloadDraft::failed(
            EventAction::CriticalTest,
            DiagnosticCode::RuntimeDiagnostic,
            effect,
            AuditInput::new(),
        ),
        TaskTerminalTarget::Cancelled => {
            TaskPayloadDraft::cancelled(EventAction::CriticalTest, effect, AuditInput::new())
        }
    };
    raw_event(&format!("{label}-success"), links(), payload.into())
}

fn task_failure(label: &str, effect: EffectDisposition) -> EventDraft {
    raw_event(
        &format!("{label}-failure"),
        links(),
        TaskPayloadDraft::terminal_commit_failed(
            EventAction::CriticalTest,
            DiagnosticCode::RuntimeDiagnostic,
            effect,
            AuditInput::new(),
        )
        .into(),
    )
}

fn event_label(event_type: EventType) -> &'static str {
    match event_type {
        EventType::CommandReceived
        | EventType::InputIntent
        | EventType::LeaseTransitionIntent
        | EventType::TaskTerminalIntent => "intent",
        EventType::CommandRejected
        | EventType::InputFailed
        | EventType::LeaseTransitionFailed
        | EventType::TaskTerminalCommitFailed => "failure",
        _ => "success",
    }
}

#[derive(Default)]
struct InMemoryAppender {
    calls: RefCell<Vec<EventType>>,
    order: RefCell<Vec<&'static str>>,
    append_count: Cell<u64>,
    failures: RefCell<Vec<(u64, GlobalLedgerError)>>,
}

impl InMemoryAppender {
    fn fail_on_call(&self, call: u64, error: GlobalLedgerError) {
        self.failures.borrow_mut().push((call, error));
    }

    fn calls(&self) -> Vec<EventType> {
        self.calls.borrow().clone()
    }

    fn order(&self) -> Vec<&'static str> {
        self.order.borrow().clone()
    }
}

impl EventAppender for InMemoryAppender {
    fn append_durable(&self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent> {
        let call = self.append_count.get() + 1;
        self.append_count.set(call);
        self.calls.borrow_mut().push(draft.event_type());
        self.order
            .borrow_mut()
            .push(event_label(draft.event_type()));
        let failure = {
            let mut failures = self.failures.borrow_mut();
            failures
                .iter()
                .position(|(failure_call, _)| *failure_call == call)
                .map(|index| failures.remove(index))
        };
        if let Some((_, error)) = failure {
            return Err(error);
        }
        Ok(PersistedEvent::from_sanitized(call, draft).expect("valid test event"))
    }
}

fn append_failure() -> GlobalLedgerError {
    crate::Sha256SecretFingerprinter::new(b"").expect_err("empty salt must fail")
}

#[test]
fn intent_append_failure_prevents_action_and_builders() {
    let appender = InMemoryAppender::default();
    appender.fail_on_call(1, append_failure());
    let action_calls = Cell::new(0);
    let success_builds = Cell::new(0);
    let failure_builds = Cell::new(0);

    let result = execute_critical::<(), &'static str, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("intent-append-failure"),
        || {
            action_calls.set(action_calls.get() + 1);
            CriticalActionReport::Succeeded {
                value: (),
                effect: DefiniteEffectDisposition::Performed,
            }
        },
        |_, effect| {
            success_builds.set(success_builds.get() + 1);
            Ok(input_success(
                "intent-append-failure",
                effect.into(),
                links(),
                AuditInput::new(),
            ))
        },
        |_, effect| {
            failure_builds.set(failure_builds.get() + 1);
            Ok(input_failure(
                "intent-append-failure",
                effect,
                links(),
                AuditInput::new(),
            ))
        },
    );

    assert!(matches!(
        result,
        Err(CriticalExecutionError::IntentAppend(_))
    ));
    assert_eq!(action_calls.get(), 0);
    assert_eq!(success_builds.get(), 0);
    assert_eq!(failure_builds.get(), 0);
    assert_eq!(appender.calls(), vec![EventType::InputIntent]);
}

#[test]
fn success_builder_observes_action_value_and_effect() {
    let appender = InMemoryAppender::default();
    let builder_observed = Cell::new(false);

    let receipt = execute_critical::<_, &'static str, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("success-observes"),
        || {
            appender.order.borrow_mut().push("action");
            CriticalActionReport::Succeeded {
                value: "actual-value",
                effect: DefiniteEffectDisposition::Performed,
            }
        },
        |value, effect| {
            assert_eq!(*value, "actual-value");
            assert_eq!(effect, DefiniteEffectDisposition::Performed);
            builder_observed.set(true);
            Ok(input_success(
                "success-observes",
                effect.into(),
                links(),
                AuditInput::new(),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    )
    .expect("success receipt");

    assert!(builder_observed.get());
    assert_eq!(receipt.value(), &"actual-value");
    assert_eq!(receipt.outcome().event_type(), EventType::InputCommitted);
    assert_eq!(
        receipt.outcome().payload().effect_disposition(),
        Some(EffectDisposition::Performed)
    );
    assert_eq!(appender.order(), vec!["intent", "action", "success"]);
}

#[test]
fn failure_builder_observes_action_error_and_effect() {
    let appender = InMemoryAppender::default();
    let builder_observed = Cell::new(false);

    let result = execute_critical::<(), _, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("failure-observes"),
        || {
            appender.order.borrow_mut().push("action");
            CriticalActionReport::Failed {
                error: "actual-error",
                effect: EffectDisposition::Indeterminate,
            }
        },
        |_, _| unreachable!("success builder must not run"),
        |error, effect| {
            assert_eq!(*error, "actual-error");
            assert_eq!(effect, EffectDisposition::Indeterminate);
            builder_observed.set(true);
            Ok(input_failure(
                "failure-observes",
                effect,
                links(),
                AuditInput::new(),
            ))
        },
    );

    let CriticalExecutionError::Action {
        error,
        effect,
        outcome,
    } = result.expect_err("durable action failure")
    else {
        panic!("expected action failure");
    };
    assert!(builder_observed.get());
    assert_eq!(error, "actual-error");
    assert_eq!(effect, EffectDisposition::Indeterminate);
    assert_eq!(outcome.event_type(), EventType::InputFailed);
    assert_eq!(appender.order(), vec!["intent", "action", "failure"]);
}

#[test]
fn outcome_sanitization_failure_is_fatal_without_receipt() {
    let appender = InMemoryAppender::default();
    let result = execute_critical::<(), &'static str, _, _>(
        &appender,
        &FailingFingerprinter,
        input_plan("sanitize-failure"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::Performed,
        },
        |_, effect| {
            Ok(raw_event_at(
                "sanitize-failure-success",
                1_752_147_200_000,
                links(),
                InputPayloadDraft::committed(
                    EventAction::InputTap,
                    effect.into(),
                    AuditInput::new().with_account("outcome-account-secret"),
                )
                .into(),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    );

    assert!(matches!(
        result,
        Err(CriticalExecutionError::OutcomeUndurable {
            effect: EffectDisposition::Performed,
            stage: CriticalOutcomeStage::Sanitize,
            code: "fingerprinter_failed",
        })
    ));
    assert_eq!(appender.calls(), vec![EventType::InputIntent]);
}

#[test]
fn outcome_build_failure_is_fatal_without_receipt() {
    let appender = InMemoryAppender::default();
    let result = execute_critical::<(), &'static str, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("build-failure"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::NotPerformed,
        },
        |_, _| Err(SanitizationError::fingerprinter_failure()),
        |_, _| unreachable!("failure builder must not run"),
    );

    assert!(matches!(
        result,
        Err(CriticalExecutionError::OutcomeUndurable {
            effect: EffectDisposition::NotPerformed,
            stage: CriticalOutcomeStage::Build,
            code: "fingerprinter_failed",
        })
    ));
}

#[test]
fn wrong_post_action_role_is_fatal_without_receipt() {
    let appender = InMemoryAppender::default();
    let result = execute_critical::<(), &'static str, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("wrong-role"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::Performed,
        },
        |_, _| {
            Ok(raw_event(
                "wrong-role-success",
                links(),
                InputPayloadDraft::completed(EventAction::InputTap, AuditInput::new()).into(),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    );

    assert!(matches!(
        result,
        Err(CriticalExecutionError::OutcomeUndurable {
            effect: EffectDisposition::Performed,
            stage: CriticalOutcomeStage::Validate,
            code: "critical_outcome_role_mismatch",
        })
    ));
    assert_eq!(appender.calls(), vec![EventType::InputIntent]);
}

#[test]
fn outcome_effect_mismatch_is_fatal_without_receipt() {
    let appender = InMemoryAppender::default();
    let result = execute_critical::<(), &'static str, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("wrong-effect"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::Performed,
        },
        |_, _| {
            Ok(input_success(
                "wrong-effect",
                EffectDisposition::NotPerformed,
                links(),
                AuditInput::new(),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    );

    assert!(matches!(
        result,
        Err(CriticalExecutionError::OutcomeUndurable {
            stage: CriticalOutcomeStage::Validate,
            code: "critical_effect_disposition_mismatch",
            ..
        })
    ));

    let appender = InMemoryAppender::default();
    let result = execute_critical::<(), &'static str, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("payload-action-mismatch"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::Performed,
        },
        |_, effect| {
            Ok(raw_event(
                "payload-action-mismatch-success",
                links(),
                InputPayloadDraft::committed(
                    EventAction::CriticalTest,
                    effect.into(),
                    AuditInput::new(),
                )
                .into(),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    );
    assert!(matches!(
        result,
        Err(CriticalExecutionError::OutcomeUndurable {
            stage: CriticalOutcomeStage::Validate,
            code: "critical_payload_action_mismatch",
            ..
        })
    ));
}

#[test]
fn selected_outcome_rejects_correlation_action_and_event_id_mismatches() {
    for (label, outcome_links, expected_code) in [
        (
            "correlation-mismatch",
            links().with_correlation_id(correlation_id("correlation-2")),
            "critical_correlation_id_mismatch",
        ),
        (
            "action-id-mismatch",
            links().with_action_id(action_id("action-2")),
            "critical_action_id_mismatch",
        ),
    ] {
        let appender = InMemoryAppender::default();
        let result = execute_critical::<(), &'static str, _, _>(
            &appender,
            &fingerprinter(),
            input_plan(label),
            || CriticalActionReport::Succeeded {
                value: (),
                effect: DefiniteEffectDisposition::Performed,
            },
            |_, effect| {
                Ok(input_success(
                    label,
                    effect.into(),
                    outcome_links,
                    AuditInput::new(),
                ))
            },
            |_, _| unreachable!("failure builder must not run"),
        );
        assert!(matches!(
            result,
            Err(CriticalExecutionError::OutcomeUndurable {
                stage: CriticalOutcomeStage::Validate,
                code,
                ..
            }) if code == expected_code
        ));
    }

    let appender = InMemoryAppender::default();
    let result = execute_critical::<(), &'static str, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("duplicate-event"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::Performed,
        },
        |_, effect| {
            Ok(EventDraft::new(
                event_id("duplicate-event-intent"),
                1_752_147_200_000,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                links(),
                InputPayloadDraft::committed(
                    EventAction::InputTap,
                    effect.into(),
                    AuditInput::new(),
                )
                .into(),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    );
    assert!(matches!(
        result,
        Err(CriticalExecutionError::OutcomeUndurable {
            stage: CriticalOutcomeStage::Validate,
            code: "critical_duplicate_event_id",
            ..
        })
    ));
}

#[test]
fn outcome_append_failure_preserves_effect_without_receipt() {
    let success_appender = InMemoryAppender::default();
    success_appender.fail_on_call(2, append_failure());
    let success = execute_critical::<(), &'static str, _, _>(
        &success_appender,
        &fingerprinter(),
        input_plan("append-success-failure"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::NotPerformed,
        },
        |_, effect| {
            Ok(input_success(
                "append-success-failure",
                effect.into(),
                links(),
                AuditInput::new(),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    );
    assert!(matches!(
        success,
        Err(CriticalExecutionError::OutcomeUndurable {
            effect: EffectDisposition::NotPerformed,
            stage: CriticalOutcomeStage::Append,
            ..
        })
    ));

    let failure_appender = InMemoryAppender::default();
    failure_appender.fail_on_call(2, append_failure());
    let failure = execute_critical::<(), &'static str, _, _>(
        &failure_appender,
        &fingerprinter(),
        input_plan("append-failure-failure"),
        || CriticalActionReport::Failed {
            error: "failed",
            effect: EffectDisposition::Indeterminate,
        },
        |_, _| unreachable!("success builder must not run"),
        |_, effect| {
            Ok(input_failure(
                "append-failure-failure",
                effect,
                links(),
                AuditInput::new(),
            ))
        },
    );
    assert!(matches!(
        failure,
        Err(CriticalExecutionError::OutcomeUndurable {
            effect: EffectDisposition::Indeterminate,
            stage: CriticalOutcomeStage::Append,
            ..
        })
    ));
}

#[test]
fn command_validation_uses_post_action_roles() {
    let success_appender = InMemoryAppender::default();
    let receipt = execute_critical::<(), &'static str, _, _>(
        &success_appender,
        &fingerprinter(),
        command_plan("command-success"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::NotPerformed,
        },
        |_, effect| Ok(command_success("command-success", effect.into())),
        |_, _| unreachable!("failure builder must not run"),
    )
    .expect("command success");
    assert_eq!(receipt.outcome().event_type(), EventType::CommandValidated);

    let failure_appender = InMemoryAppender::default();
    let result = execute_critical::<(), &'static str, _, _>(
        &failure_appender,
        &fingerprinter(),
        command_plan("command-failure"),
        || CriticalActionReport::Failed {
            error: "rejected",
            effect: EffectDisposition::NotPerformed,
        },
        |_, _| unreachable!("success builder must not run"),
        |_, effect| Ok(command_failure("command-failure", effect)),
    );
    let CriticalExecutionError::Action { outcome, .. } = result.expect_err("command failure")
    else {
        panic!("expected command action failure");
    };
    assert_eq!(outcome.event_type(), EventType::CommandRejected);
}

#[test]
fn lease_transition_role_map_is_complete() {
    for (target, expected) in [
        (LeaseTransitionTarget::Granted, EventType::LeaseGranted),
        (
            LeaseTransitionTarget::Transferred,
            EventType::LeaseTransferred,
        ),
        (LeaseTransitionTarget::Renewed, EventType::LeaseRenewed),
        (LeaseTransitionTarget::Released, EventType::LeaseReleased),
        (LeaseTransitionTarget::Expired, EventType::LeaseExpired),
    ] {
        let label = format!("lease-{target:?}");
        let success_appender = InMemoryAppender::default();
        let receipt = execute_critical::<(), &'static str, _, _>(
            &success_appender,
            &fingerprinter(),
            lease_plan(&label, target),
            || CriticalActionReport::Succeeded {
                value: (),
                effect: DefiniteEffectDisposition::Performed,
            },
            |_, effect| Ok(lease_success(&label, target, effect.into())),
            |_, _| unreachable!("failure builder must not run"),
        )
        .expect("lease success");
        assert_eq!(receipt.outcome().event_type(), expected);

        let failure_label = format!("{label}-failure-path");
        let failure_appender = InMemoryAppender::default();
        let result = execute_critical::<(), &'static str, _, _>(
            &failure_appender,
            &fingerprinter(),
            lease_plan(&failure_label, target),
            || CriticalActionReport::Failed {
                error: "lease commit failed",
                effect: EffectDisposition::Indeterminate,
            },
            |_, _| unreachable!("success builder must not run"),
            |_, effect| Ok(lease_failure(&failure_label, effect)),
        );
        let CriticalExecutionError::Action { outcome, .. } = result.expect_err("lease failure")
        else {
            panic!("expected lease action failure");
        };
        assert_eq!(outcome.event_type(), EventType::LeaseTransitionFailed);
    }
}

#[test]
fn task_terminal_role_map_is_complete() {
    for (target, expected) in [
        (TaskTerminalTarget::Completed, EventType::TaskCompleted),
        (TaskTerminalTarget::Failed, EventType::TaskFailed),
        (TaskTerminalTarget::Cancelled, EventType::TaskCancelled),
    ] {
        let label = format!("task-{target:?}");
        let success_appender = InMemoryAppender::default();
        let receipt = execute_critical::<(), &'static str, _, _>(
            &success_appender,
            &fingerprinter(),
            task_plan(&label, target),
            || CriticalActionReport::Succeeded {
                value: (),
                effect: DefiniteEffectDisposition::Performed,
            },
            |_, effect| Ok(task_success(&label, target, effect.into())),
            |_, _| unreachable!("failure builder must not run"),
        )
        .expect("task success");
        assert_eq!(receipt.outcome().event_type(), expected);

        let failure_label = format!("{label}-failure-path");
        let failure_appender = InMemoryAppender::default();
        let result = execute_critical::<(), &'static str, _, _>(
            &failure_appender,
            &fingerprinter(),
            task_plan(&failure_label, target),
            || CriticalActionReport::Failed {
                error: "task terminal commit failed",
                effect: EffectDisposition::Indeterminate,
            },
            |_, _| unreachable!("success builder must not run"),
            |_, effect| Ok(task_failure(&failure_label, effect)),
        );
        let CriticalExecutionError::Action { outcome, .. } = result.expect_err("task failure")
        else {
            panic!("expected task action failure");
        };
        assert_eq!(outcome.event_type(), EventType::TaskTerminalCommitFailed);
    }
}

#[test]
fn same_action_id_is_not_deduplicated_by_c1() {
    let appender = InMemoryAppender::default();
    let action_calls = Cell::new(0);

    for label in ["repeat-first", "repeat-second"] {
        execute_critical::<(), &'static str, _, _>(
            &appender,
            &fingerprinter(),
            input_plan(label),
            || {
                action_calls.set(action_calls.get() + 1);
                CriticalActionReport::Succeeded {
                    value: (),
                    effect: DefiniteEffectDisposition::Performed,
                }
            },
            |_, effect| {
                Ok(input_success(
                    label,
                    effect.into(),
                    links(),
                    AuditInput::new(),
                ))
            },
            |_, _| unreachable!("failure builder must not run"),
        )
        .expect("repeated call remains independently ordered");
    }

    assert_eq!(action_calls.get(), 2);
    assert_eq!(
        appender.calls(),
        vec![
            EventType::InputIntent,
            EventType::InputCommitted,
            EventType::InputIntent,
            EventType::InputCommitted,
        ]
    );
}

#[test]
fn panic_propagates_after_durable_intent_without_installing_hook() {
    let appender = InMemoryAppender::default();
    let success_builds = Cell::new(0);
    let failure_builds = Cell::new(0);

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = execute_critical::<(), &'static str, _, _>(
            &appender,
            &fingerprinter(),
            input_plan("panic-propagates"),
            || std::panic::panic_any(()),
            |_, effect| {
                success_builds.set(success_builds.get() + 1);
                Ok(input_success(
                    "panic-propagates",
                    effect.into(),
                    links(),
                    AuditInput::new(),
                ))
            },
            |_, effect| {
                failure_builds.set(failure_builds.get() + 1);
                Ok(input_failure(
                    "panic-propagates",
                    effect,
                    links(),
                    AuditInput::new(),
                ))
            },
        );
    }));

    assert!(panic.is_err());
    assert_eq!(appender.calls(), vec![EventType::InputIntent]);
    assert_eq!(success_builds.get(), 0);
    assert_eq!(failure_builds.get(), 0);
}

#[test]
fn critical_debug_does_not_disclose_value_error_payload_path_or_endpoint() {
    const VALUE_SECRET: &str = "critical-value-secret-571b";
    const ERROR_SECRET: &str = "critical-error-secret-d92a";
    const ACCOUNT_SECRET: &str = "critical-account@example.invalid";
    const PATH_SECRET: &str = r"C:\Users\Alice\critical-private.json";
    const ENDPOINT_SECRET: &str = "127.0.0.1:16384";

    struct SecretValue(&'static str);
    struct SecretError(&'static str);

    let success_report = CriticalActionReport::<SecretValue, SecretError>::Succeeded {
        value: SecretValue(VALUE_SECRET),
        effect: DefiniteEffectDisposition::Performed,
    };
    assert!(!format!("{success_report:?}").contains(VALUE_SECRET));

    let success_appender = InMemoryAppender::default();
    let receipt = execute_critical::<_, SecretError, _, _>(
        &success_appender,
        &fingerprinter(),
        input_plan("debug-success"),
        || CriticalActionReport::Succeeded {
            value: SecretValue(VALUE_SECRET),
            effect: DefiniteEffectDisposition::Performed,
        },
        |value, effect| {
            assert_eq!(value.0, VALUE_SECRET);
            Ok(input_success(
                "debug-success",
                effect.into(),
                links(),
                AuditInput::new()
                    .with_account(ACCOUNT_SECRET)
                    .with_machine_path(PATH_SECRET)
                    .with_device_endpoint(ENDPOINT_SECRET),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    )
    .expect("debug success");
    let receipt_debug = format!("{receipt:?}");
    for secret in [VALUE_SECRET, ACCOUNT_SECRET, PATH_SECRET, ENDPOINT_SECRET] {
        assert!(!receipt_debug.contains(secret));
    }

    let failure_report = CriticalActionReport::<SecretValue, SecretError>::Failed {
        error: SecretError(ERROR_SECRET),
        effect: EffectDisposition::Indeterminate,
    };
    assert!(!format!("{failure_report:?}").contains(ERROR_SECRET));

    let failure_appender = InMemoryAppender::default();
    let error = execute_critical::<SecretValue, _, _, _>(
        &failure_appender,
        &fingerprinter(),
        input_plan("debug-failure"),
        || CriticalActionReport::Failed {
            error: SecretError(ERROR_SECRET),
            effect: EffectDisposition::Indeterminate,
        },
        |_, _| unreachable!("success builder must not run"),
        |error, effect| {
            assert_eq!(error.0, ERROR_SECRET);
            Ok(input_failure(
                "debug-failure",
                effect,
                links(),
                AuditInput::new()
                    .with_account(ACCOUNT_SECRET)
                    .with_machine_path(PATH_SECRET)
                    .with_device_endpoint(ENDPOINT_SECRET),
            ))
        },
    )
    .expect_err("debug failure");
    let diagnostic = format!("{error:?} {error}");
    for secret in [ERROR_SECRET, ACCOUNT_SECRET, PATH_SECRET, ENDPOINT_SECRET] {
        assert!(!diagnostic.contains(secret));
    }
}

#[test]
fn plan_and_selected_outcome_validate_required_links() {
    let missing_correlation = sanitize(raw_event(
        "missing-correlation-intent",
        EventLinksDraft::default().with_action_id(action_id("action-1")),
        InputPayloadDraft::intent(EventAction::InputTap, AuditInput::new()).into(),
    ));
    assert_eq!(
        CriticalEventPlan::new(CriticalOperation::DeviceWrite, missing_correlation)
            .expect_err("missing correlation"),
        CriticalPlanError::MissingCorrelationId
    );

    let missing_action = sanitize(raw_event(
        "missing-action-intent",
        EventLinksDraft::default().with_correlation_id(correlation_id("correlation-1")),
        InputPayloadDraft::intent(EventAction::InputTap, AuditInput::new()).into(),
    ));
    assert_eq!(
        CriticalEventPlan::new(CriticalOperation::DeviceWrite, missing_action)
            .expect_err("missing action"),
        CriticalPlanError::MissingActionId
    );

    let wrong_intent = sanitize(raw_event(
        "wrong-operation-intent",
        links(),
        CommandPayloadDraft::received(EventAction::CriticalTest, AuditInput::new()).into(),
    ));
    assert_eq!(
        CriticalEventPlan::new(CriticalOperation::DeviceWrite, wrong_intent)
            .expect_err("wrong intent"),
        CriticalPlanError::UnsupportedIntent
    );

    let appender = InMemoryAppender::default();
    let result = execute_critical::<(), &'static str, _, _>(
        &appender,
        &fingerprinter(),
        input_plan("stable-link-mismatch"),
        || CriticalActionReport::Succeeded {
            value: (),
            effect: DefiniteEffectDisposition::Performed,
        },
        |_, effect| {
            Ok(input_success(
                "stable-link-mismatch",
                effect.into(),
                links().with_instance_id(instance_id("instance-2")),
                AuditInput::new(),
            ))
        },
        |_, _| unreachable!("failure builder must not run"),
    );
    assert!(matches!(
        result,
        Err(CriticalExecutionError::OutcomeUndurable {
            stage: CriticalOutcomeStage::Validate,
            code: "critical_stable_link_mismatch",
            ..
        })
    ));
}
