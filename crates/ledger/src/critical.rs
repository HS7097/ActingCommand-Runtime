// SPDX-License-Identifier: AGPL-3.0-only

use crate::{GlobalLedger, GlobalLedgerError, GlobalLedgerResult, PersistedEvent};
use actingcommand_contract::{EventType, SanitizedEventDraft};
use std::cell::Cell;
use std::fmt;
use std::sync::Once;

thread_local! {
    static CRITICAL_PANIC_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct CriticalPanicScope;

impl CriticalPanicScope {
    fn enter() -> Self {
        install_critical_panic_hook();
        CRITICAL_PANIC_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for CriticalPanicScope {
    fn drop(&mut self) {
        CRITICAL_PANIC_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

fn install_critical_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let suppress = CRITICAL_PANIC_DEPTH
                .try_with(|depth| depth.get() > 0)
                .unwrap_or(false);
            if !suppress {
                previous(info);
            }
        }));
    });
}

pub trait EventAppender {
    fn append_durable(&self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent>;
}

impl EventAppender for GlobalLedger {
    fn append_durable(&self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent> {
        self.append(draft)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CriticalReceipt<T> {
    value: T,
    intent: PersistedEvent,
    outcome: PersistedEvent,
}

impl<T> CriticalReceipt<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn intent(&self) -> &PersistedEvent {
        &self.intent
    }

    pub fn outcome(&self) -> &PersistedEvent {
        &self.outcome
    }

    pub fn into_value(self) -> T {
        self.value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriticalPlanError {
    EventFamilyMismatch,
    MissingCorrelationId,
    CorrelationIdMismatch,
    MissingActionId,
    ActionIdMismatch,
    StableIdentityLinkMismatch,
    DuplicateEventType,
    DuplicateEventId,
    UnsupportedIntent,
    OutcomeRoleMismatch,
}

impl fmt::Display for CriticalPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EventFamilyMismatch => "critical event families must match",
            Self::MissingCorrelationId => "critical events require correlation ids",
            Self::CorrelationIdMismatch => "critical event correlation ids must match",
            Self::MissingActionId => "critical events require action ids",
            Self::ActionIdMismatch => "critical event action ids must match",
            Self::StableIdentityLinkMismatch => "critical stable identity links must match",
            Self::DuplicateEventType => "critical event types must be distinct",
            Self::DuplicateEventId => "critical event ids must be distinct",
            Self::UnsupportedIntent => "critical intent event type is unsupported",
            Self::OutcomeRoleMismatch => "critical outcome event roles do not match the intent",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for CriticalPlanError {}

#[derive(Debug)]
pub struct CriticalEventPlan {
    intent: SanitizedEventDraft,
    success_outcome: SanitizedEventDraft,
    failure_outcome: SanitizedEventDraft,
}

impl CriticalEventPlan {
    pub fn new(
        intent: SanitizedEventDraft,
        success_outcome: SanitizedEventDraft,
        failure_outcome: SanitizedEventDraft,
    ) -> Result<Self, CriticalPlanError> {
        let event_types = [
            intent.event_type(),
            success_outcome.event_type(),
            failure_outcome.event_type(),
        ];
        if event_types
            .iter()
            .any(|event_type| event_type.family() != event_types[0].family())
        {
            return Err(CriticalPlanError::EventFamilyMismatch);
        }
        if event_types[0] == event_types[1]
            || event_types[0] == event_types[2]
            || event_types[1] == event_types[2]
        {
            return Err(CriticalPlanError::DuplicateEventType);
        }
        validate_event_roles(event_types[0], event_types[1], event_types[2])?;

        let event_ids = [
            intent.event_id(),
            success_outcome.event_id(),
            failure_outcome.event_id(),
        ];
        if event_ids[0] == event_ids[1]
            || event_ids[0] == event_ids[2]
            || event_ids[1] == event_ids[2]
        {
            return Err(CriticalPlanError::DuplicateEventId);
        }

        validate_matching_link(
            intent.links().correlation_id(),
            success_outcome.links().correlation_id(),
            failure_outcome.links().correlation_id(),
            CriticalPlanError::MissingCorrelationId,
            CriticalPlanError::CorrelationIdMismatch,
        )?;
        validate_matching_link(
            intent.links().action_id(),
            success_outcome.links().action_id(),
            failure_outcome.links().action_id(),
            CriticalPlanError::MissingActionId,
            CriticalPlanError::ActionIdMismatch,
        )?;
        validate_stable_identity_links(
            intent.links(),
            success_outcome.links(),
            failure_outcome.links(),
        )?;

        Ok(Self {
            intent,
            success_outcome,
            failure_outcome,
        })
    }
}

fn validate_event_roles(
    intent: EventType,
    success: EventType,
    failure: EventType,
) -> Result<(), CriticalPlanError> {
    match intent {
        EventType::InputIntent
            if matches!(
                success,
                EventType::InputCommitted | EventType::InputCompleted
            ) && failure == EventType::InputFailed =>
        {
            Ok(())
        }
        EventType::CommandReceived
            if success == EventType::CommandValidated && failure == EventType::CommandRejected =>
        {
            Ok(())
        }
        EventType::InputIntent | EventType::CommandReceived => {
            Err(CriticalPlanError::OutcomeRoleMismatch)
        }
        _ => Err(CriticalPlanError::UnsupportedIntent),
    }
}

fn validate_matching_link<T: PartialEq>(
    intent: Option<&T>,
    success: Option<&T>,
    failure: Option<&T>,
    missing_error: CriticalPlanError,
    mismatch_error: CriticalPlanError,
) -> Result<(), CriticalPlanError> {
    let intent = intent.ok_or(missing_error)?;
    let success = success.ok_or(missing_error)?;
    let failure = failure.ok_or(missing_error)?;
    if intent != success || intent != failure {
        return Err(mismatch_error);
    }
    Ok(())
}

fn validate_stable_identity_links(
    intent: &actingcommand_contract::EventLinks,
    success: &actingcommand_contract::EventLinks,
    failure: &actingcommand_contract::EventLinks,
) -> Result<(), CriticalPlanError> {
    if intent.instance_id() != success.instance_id()
        || intent.instance_id() != failure.instance_id()
        || intent.request_id() != success.request_id()
        || intent.request_id() != failure.request_id()
        || intent.task_id() != success.task_id()
        || intent.task_id() != failure.task_id()
        || intent.run_id() != success.run_id()
        || intent.run_id() != failure.run_id()
        || intent.lease_id() != success.lease_id()
        || intent.lease_id() != failure.lease_id()
    {
        return Err(CriticalPlanError::StableIdentityLinkMismatch);
    }
    Ok(())
}

#[derive(Debug)]
pub enum CriticalExecutionError<E> {
    IntentAppend(GlobalLedgerError),
    Action {
        error: E,
        outcome: Box<PersistedEvent>,
    },
    Panicked {
        outcome: Box<PersistedEvent>,
    },
    Indeterminate {
        action_performed: bool,
        source: GlobalLedgerError,
    },
}

impl<E> CriticalExecutionError<E> {
    pub fn is_fatal(&self) -> bool {
        match self {
            Self::IntentAppend(error) => error.is_fatal(),
            Self::Action { .. } => false,
            Self::Panicked { .. } => true,
            Self::Indeterminate { .. } => true,
        }
    }
}

impl<E> fmt::Display for CriticalExecutionError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntentAppend(error) => {
                write!(formatter, "critical intent append failed: {error}")
            }
            Self::Action { .. } => {
                write!(formatter, "critical action failed after durable outcome")
            }
            Self::Panicked { .. } => {
                write!(
                    formatter,
                    "critical action panicked after durable failure outcome"
                )
            }
            Self::Indeterminate {
                action_performed,
                source,
            } => write!(
                formatter,
                "critical action outcome persistence is indeterminate after action_performed={action_performed}: {source}"
            ),
        }
    }
}

impl<E: fmt::Debug> std::error::Error for CriticalExecutionError<E> {}

pub fn execute_critical<T, E>(
    appender: &impl EventAppender,
    plan: CriticalEventPlan,
    action: impl FnOnce() -> Result<T, E>,
) -> Result<CriticalReceipt<T>, CriticalExecutionError<E>> {
    let CriticalEventPlan {
        intent,
        success_outcome,
        failure_outcome,
    } = plan;
    let intent = appender
        .append_durable(intent)
        .map_err(CriticalExecutionError::IntentAppend)?;
    let panic_scope = CriticalPanicScope::enter();
    let action_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
    drop(panic_scope);

    match action_result {
        Ok(Ok(value)) => appender
            .append_durable(success_outcome)
            .map(|outcome| CriticalReceipt {
                value,
                intent,
                outcome,
            })
            .map_err(|source| CriticalExecutionError::Indeterminate {
                action_performed: true,
                source,
            }),
        Ok(Err(error)) => match appender.append_durable(failure_outcome) {
            Ok(outcome) => Err(CriticalExecutionError::Action {
                error,
                outcome: Box::new(outcome),
            }),
            Err(source) => Err(CriticalExecutionError::Indeterminate {
                action_performed: true,
                source,
            }),
        },
        Err(_) => match appender.append_durable(failure_outcome) {
            Ok(outcome) => Err(CriticalExecutionError::Panicked {
                outcome: Box::new(outcome),
            }),
            Err(source) => Err(CriticalExecutionError::Indeterminate {
                action_performed: true,
                source,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CriticalEventPlan, CriticalExecutionError, CriticalPlanError, EventAppender,
        execute_critical,
    };
    use crate::{GlobalLedgerError, GlobalLedgerResult, PersistedEvent};
    use actingcommand_contract::{
        ActionId, AuditInput, CommandPayloadDraft, CorrelationId, EffectDisposition, EventActor,
        EventDraft, EventId, EventLinks, EventOrigin, EventSeverity, EventSource, EventType,
        InputPayloadDraft, InstanceId, LeaseId, RequestId, RunId, SanitizedEventDraft, StaticCode,
        TaskId,
    };
    use sha2::{Digest, Sha256};
    use std::cell::{Cell, RefCell};
    use std::process::{Command, Output, Stdio};
    use std::time::{Duration, Instant};

    const PANIC_CHILD_ENV: &str = "ACTINGCOMMAND_CRITICAL_PANIC_CHILD";
    const PANIC_SECRET: &str = "critical-panic-secret-4e53e895";
    const NONCRITICAL_HOOK_MARKER: &str = "noncritical-hook-marker-c29a6953";

    #[derive(Clone, Copy)]
    enum InputKind {
        Intent,
        Committed,
        Completed,
        Failed,
    }

    fn opaque_id(label: &str) -> [u8; 16] {
        let digest = Sha256::digest(label.as_bytes());
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes
    }

    fn code(value: &'static str) -> StaticCode {
        StaticCode::new(value).expect("static code")
    }

    fn event_id(value: &str) -> EventId {
        EventId::new(opaque_id(value))
    }

    fn event_label(value: &EventId) -> String {
        ["intent", "success", "failure"]
            .into_iter()
            .find(|label| event_id(label) == *value)
            .map_or_else(|| value.to_string(), str::to_string)
    }

    fn correlation_id(value: &str) -> CorrelationId {
        CorrelationId::new(opaque_id(value))
    }

    fn action_id(value: &str) -> ActionId {
        ActionId::new(opaque_id(value))
    }

    #[derive(Default)]
    struct InMemoryAppender {
        calls: RefCell<Vec<String>>,
        order: RefCell<Vec<String>>,
        append_count: Cell<u64>,
        failures: RefCell<Vec<(u64, GlobalLedgerError)>>,
    }

    impl InMemoryAppender {
        fn fail_on_call(&self, call: u64, error: GlobalLedgerError) {
            self.failures.borrow_mut().push((call, error));
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }

        fn order(&self) -> Vec<String> {
            self.order.borrow().clone()
        }
    }

    impl EventAppender for InMemoryAppender {
        fn append_durable(&self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent> {
            let call = self.append_count.get() + 1;
            self.append_count.set(call);
            let event_id = event_label(draft.event_id());
            self.calls.borrow_mut().push(event_id.clone());
            self.order.borrow_mut().push(event_id);
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

    fn input_draft(
        event_id: &str,
        transition: InputKind,
        correlation_id: Option<&str>,
        action_id: Option<&str>,
    ) -> SanitizedEventDraft {
        let mut links = EventLinks::default();
        if let Some(value) = correlation_id {
            links = links.with_correlation_id(self::correlation_id(value));
        }
        if let Some(value) = action_id {
            links = links.with_action_id(self::action_id(value));
        }
        input_draft_with_links(event_id, transition, links)
    }

    fn input_draft_with_links(
        event_id: &str,
        transition: InputKind,
        links: EventLinks,
    ) -> SanitizedEventDraft {
        let payload = match transition {
            InputKind::Intent => InputPayloadDraft::intent(code("input.tap"), AuditInput::new()),
            InputKind::Committed => InputPayloadDraft::committed(
                code("input.tap"),
                EffectDisposition::Performed,
                AuditInput::new(),
            ),
            InputKind::Completed => {
                InputPayloadDraft::completed(code("input.tap"), AuditInput::new())
            }
            InputKind::Failed => InputPayloadDraft::failed(
                code("input.tap"),
                code("input.failed"),
                EffectDisposition::Indeterminate,
                AuditInput::new(),
            ),
        };
        EventDraft::new(
            self::event_id(event_id),
            1_752_147_200_000,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Runtime, code("runtime"), EventActor::Runtime),
            links,
            payload.into(),
        )
        .sanitize(
            &crate::Sha256SecretFingerprinter::new(b"test-private-salt").expect("fingerprinter"),
        )
        .expect("sanitize")
    }

    fn command_draft(event_id: &str, correlation_id: &str, action_id: &str) -> SanitizedEventDraft {
        EventDraft::new(
            self::event_id(event_id),
            1_752_147_200_000,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Runtime, code("runtime"), EventActor::Runtime),
            EventLinks::default()
                .with_correlation_id(self::correlation_id(correlation_id))
                .with_action_id(self::action_id(action_id)),
            CommandPayloadDraft::received(code("critical.test"), AuditInput::new()).into(),
        )
        .sanitize(
            &crate::Sha256SecretFingerprinter::new(b"test-private-salt").expect("fingerprinter"),
        )
        .expect("sanitize")
    }

    fn valid_plan() -> CriticalEventPlan {
        CriticalEventPlan::new(
            input_draft(
                "intent",
                InputKind::Intent,
                Some("correlation-1"),
                Some("action-1"),
            ),
            input_draft(
                "success",
                InputKind::Completed,
                Some("correlation-1"),
                Some("action-1"),
            ),
            input_draft(
                "failure",
                InputKind::Failed,
                Some("correlation-1"),
                Some("action-1"),
            ),
        )
        .expect("valid input critical plan")
    }

    fn input_links() -> EventLinks {
        EventLinks::default()
            .with_correlation_id(correlation_id("correlation-1"))
            .with_action_id(action_id("action-1"))
            .with_instance_id(InstanceId::new(opaque_id("instance-1")))
            .with_request_id(RequestId::new(opaque_id("request-1")))
            .with_task_id(TaskId::new(opaque_id("task-1")))
            .with_run_id(RunId::new(opaque_id("run-1")))
            .with_lease_id(LeaseId::new(opaque_id("lease-1")))
    }

    fn input_plan_with_links(
        intent_links: EventLinks,
        success_links: EventLinks,
        failure_links: EventLinks,
    ) -> Result<CriticalEventPlan, CriticalPlanError> {
        CriticalEventPlan::new(
            input_draft_with_links("intent", InputKind::Intent, intent_links),
            input_draft_with_links("success", InputKind::Completed, success_links),
            input_draft_with_links("failure", InputKind::Failed, failure_links),
        )
    }

    fn append_failure() -> GlobalLedgerError {
        crate::Sha256SecretFingerprinter::new(b"").expect_err("empty fingerprinter salt must fail")
    }

    #[test]
    fn intent_append_failure_prevents_action() {
        let appender = InMemoryAppender::default();
        appender.fail_on_call(1, append_failure());
        let action_calls = Cell::new(0);

        let result = execute_critical(&appender, valid_plan(), || {
            action_calls.set(action_calls.get() + 1);
            Ok::<_, &'static str>(())
        });

        assert!(matches!(
            result,
            Err(CriticalExecutionError::IntentAppend(_))
        ));
        assert_eq!(action_calls.get(), 0);
        assert_eq!(appender.calls(), vec!["intent"]);
    }

    #[test]
    fn successful_action_requires_durable_success_outcome() {
        let appender = InMemoryAppender::default();
        let action_calls = Cell::new(0);

        let receipt = execute_critical(&appender, valid_plan(), || {
            action_calls.set(action_calls.get() + 1);
            appender.order.borrow_mut().push("action".to_string());
            Ok::<_, &'static str>("done")
        })
        .expect("durable success receipt");

        assert_eq!(receipt.value(), &"done");
        assert_eq!(receipt.outcome().event_id(), &event_id("success"));
        assert_eq!(action_calls.get(), 1);
        assert_eq!(appender.order(), vec!["intent", "action", "success"]);
    }

    #[test]
    fn failed_action_requires_durable_failure_outcome() {
        let appender = InMemoryAppender::default();
        let action_calls = Cell::new(0);

        let result = execute_critical(&appender, valid_plan(), || {
            action_calls.set(action_calls.get() + 1);
            appender.order.borrow_mut().push("action".to_string());
            Err::<(), _>("action failed")
        });

        let CriticalExecutionError::Action { error, outcome } = result.expect_err("action failure")
        else {
            panic!("expected durable action failure");
        };
        assert_eq!(error, "action failed");
        assert_eq!(outcome.event_id(), &event_id("failure"));
        assert_eq!(action_calls.get(), 1);
        assert_eq!(appender.order(), vec!["intent", "action", "failure"]);
    }

    #[test]
    fn outcome_append_failure_returns_indeterminate_fatal_without_success_receipt() {
        let appender = InMemoryAppender::default();
        appender.fail_on_call(2, append_failure());
        let action_calls = Cell::new(0);

        let result = execute_critical(&appender, valid_plan(), || {
            action_calls.set(action_calls.get() + 1);
            appender.order.borrow_mut().push("action".to_string());
            Ok::<_, &'static str>(())
        });

        let CriticalExecutionError::Indeterminate {
            action_performed,
            source,
        } = result.expect_err("indeterminate result")
        else {
            panic!("expected indeterminate failure");
        };
        assert!(action_performed);
        assert!(source.is_fatal());
        assert!(
            CriticalExecutionError::<&'static str>::Indeterminate {
                action_performed,
                source,
            }
            .is_fatal()
        );
        assert_eq!(action_calls.get(), 1);
        assert_eq!(appender.order(), vec!["intent", "action", "success"]);
    }

    #[test]
    fn successful_path_orders_intent_action_outcome() {
        let appender = InMemoryAppender::default();

        let receipt = execute_critical(&appender, valid_plan(), || Ok::<_, &'static str>(()))
            .expect("success");

        assert_eq!(receipt.intent().event_type(), EventType::InputIntent);
        assert_eq!(receipt.outcome().event_type(), EventType::InputCompleted);
        assert_eq!(appender.calls(), vec!["intent", "success"]);
    }

    #[test]
    fn critical_plan_rejects_contract_mismatches_before_execution() {
        let appender = InMemoryAppender::default();
        let action_calls = Cell::new(0);
        let intent = || {
            input_draft(
                "intent",
                InputKind::Intent,
                Some("correlation-1"),
                Some("action-1"),
            )
        };
        let failure = || {
            input_draft(
                "failure",
                InputKind::Failed,
                Some("correlation-1"),
                Some("action-1"),
            )
        };

        assert_eq!(
            CriticalEventPlan::new(
                intent(),
                command_draft("success", "correlation-1", "action-1"),
                failure(),
            )
            .expect_err("family mismatch"),
            CriticalPlanError::EventFamilyMismatch
        );
        assert_eq!(
            CriticalEventPlan::new(
                intent(),
                input_draft("success", InputKind::Completed, None, Some("action-1")),
                failure(),
            )
            .expect_err("missing correlation"),
            CriticalPlanError::MissingCorrelationId
        );
        assert_eq!(
            CriticalEventPlan::new(
                intent(),
                input_draft(
                    "success",
                    InputKind::Completed,
                    Some("correlation-2"),
                    Some("action-1"),
                ),
                failure(),
            )
            .expect_err("correlation mismatch"),
            CriticalPlanError::CorrelationIdMismatch
        );
        assert_eq!(
            CriticalEventPlan::new(
                intent(),
                input_draft("success", InputKind::Completed, Some("correlation-1"), None),
                failure(),
            )
            .expect_err("missing action"),
            CriticalPlanError::MissingActionId
        );
        assert_eq!(
            CriticalEventPlan::new(
                intent(),
                input_draft(
                    "success",
                    InputKind::Completed,
                    Some("correlation-1"),
                    Some("action-2"),
                ),
                failure(),
            )
            .expect_err("action mismatch"),
            CriticalPlanError::ActionIdMismatch
        );
        assert_eq!(
            CriticalEventPlan::new(
                intent(),
                input_draft(
                    "success",
                    InputKind::Intent,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                failure(),
            )
            .expect_err("duplicate event type"),
            CriticalPlanError::DuplicateEventType
        );
        assert_eq!(
            CriticalEventPlan::new(
                intent(),
                input_draft(
                    "intent",
                    InputKind::Completed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                failure(),
            )
            .expect_err("duplicate event id"),
            CriticalPlanError::DuplicateEventId
        );

        assert_eq!(action_calls.get(), 0);
        assert!(appender.calls().is_empty());
    }

    #[test]
    fn failure_outcome_append_failure_returns_indeterminate_fatal() {
        let appender = InMemoryAppender::default();
        appender.fail_on_call(2, append_failure());

        let result = execute_critical(&appender, valid_plan(), || {
            appender.order.borrow_mut().push("action".to_string());
            Err::<(), _>("action failed")
        });

        assert!(matches!(
            result,
            Err(CriticalExecutionError::Indeterminate {
                action_performed: true,
                ..
            })
        ));
        assert_eq!(appender.order(), vec!["intent", "action", "failure"]);
    }

    #[test]
    fn panic_records_failure_outcome_and_returns_fatal() {
        let appender = InMemoryAppender::default();

        let result = execute_critical::<(), &'static str>(&appender, valid_plan(), || {
            appender.order.borrow_mut().push("action".to_string());
            std::panic::panic_any(())
        });

        let error = result.expect_err("panic must fail");
        assert!(error.is_fatal());
        assert_eq!(
            error.to_string(),
            "critical action panicked after durable failure outcome"
        );
        let CriticalExecutionError::Panicked { outcome } = error else {
            panic!("expected typed panic result");
        };
        assert_eq!(outcome.event_id(), &event_id("failure"));
        assert_eq!(appender.order(), vec!["intent", "action", "failure"]);
    }

    #[test]
    fn panic_failure_outcome_append_failure_returns_indeterminate() {
        let appender = InMemoryAppender::default();
        appender.fail_on_call(2, append_failure());

        let result = execute_critical::<(), &'static str>(&appender, valid_plan(), || {
            appender.order.borrow_mut().push("action".to_string());
            std::panic::panic_any(())
        });

        assert!(matches!(
            result,
            Err(CriticalExecutionError::Indeterminate {
                action_performed: true,
                ..
            })
        ));
        assert_eq!(appender.order(), vec!["intent", "action", "failure"]);
    }

    #[test]
    fn critical_plan_rejects_swapped_and_unsupported_roles_before_execution() {
        let appender = InMemoryAppender::default();
        let action_calls = Cell::new(0);
        CriticalEventPlan::new(
            input_draft(
                "intent-committed",
                InputKind::Intent,
                Some("correlation-1"),
                Some("action-1"),
            ),
            input_draft(
                "success-committed",
                InputKind::Committed,
                Some("correlation-1"),
                Some("action-1"),
            ),
            input_draft(
                "failure-committed",
                InputKind::Failed,
                Some("correlation-1"),
                Some("action-1"),
            ),
        )
        .expect("input committed triplet must be supported");
        assert_eq!(
            CriticalEventPlan::new(
                input_draft(
                    "intent",
                    InputKind::Intent,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                input_draft(
                    "success",
                    InputKind::Failed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                input_draft(
                    "failure",
                    InputKind::Completed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
            )
            .expect_err("swapped outcomes must fail"),
            CriticalPlanError::OutcomeRoleMismatch
        );
        assert_eq!(
            CriticalEventPlan::new(
                input_draft(
                    "intent",
                    InputKind::Committed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                input_draft(
                    "success",
                    InputKind::Completed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                input_draft(
                    "failure",
                    InputKind::Failed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
            )
            .expect_err("unsupported intent must fail"),
            CriticalPlanError::UnsupportedIntent
        );
        CriticalEventPlan::new(
            command_draft("intent", "correlation-1", "action-1"),
            EventDraft::new(
                event_id("success"),
                1_752_147_200_000,
                EventSeverity::Info,
                EventOrigin::new(EventSource::Runtime, code("runtime"), EventActor::Runtime),
                EventLinks::default()
                    .with_correlation_id(correlation_id("correlation-1"))
                    .with_action_id(action_id("action-1")),
                CommandPayloadDraft::validated(
                    code("critical.test"),
                    EffectDisposition::NotPerformed,
                    AuditInput::new(),
                )
                .into(),
            )
            .sanitize(
                &crate::Sha256SecretFingerprinter::new(b"test-private-salt")
                    .expect("fingerprinter"),
            )
            .expect("sanitize"),
            EventDraft::new(
                event_id("failure"),
                1_752_147_200_000,
                EventSeverity::Info,
                EventOrigin::new(EventSource::Runtime, code("runtime"), EventActor::Runtime),
                EventLinks::default()
                    .with_correlation_id(correlation_id("correlation-1"))
                    .with_action_id(action_id("action-1")),
                CommandPayloadDraft::rejected(
                    code("critical.test"),
                    code("command.rejected"),
                    EffectDisposition::NotPerformed,
                    AuditInput::new(),
                )
                .into(),
            )
            .sanitize(
                &crate::Sha256SecretFingerprinter::new(b"test-private-salt")
                    .expect("fingerprinter"),
            )
            .expect("sanitize"),
        )
        .expect("command role triplet must be supported");
        assert_eq!(action_calls.get(), 0);
        assert!(appender.calls().is_empty());
    }

    #[test]
    fn critical_plan_rejects_stable_identity_link_mismatches() {
        let intent = input_links();
        let failure = input_links();

        let success = input_links().with_instance_id(InstanceId::new(opaque_id("instance-2")));
        assert_eq!(
            input_plan_with_links(intent.clone(), success, failure.clone())
                .expect_err("instance mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );

        let success = input_links().with_request_id(RequestId::new(opaque_id("request-2")));
        assert_eq!(
            input_plan_with_links(intent.clone(), success, failure.clone())
                .expect_err("request mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );

        let success = EventLinks::default()
            .with_correlation_id(correlation_id("correlation-1"))
            .with_action_id(action_id("action-1"))
            .with_instance_id(InstanceId::new(opaque_id("instance-1")))
            .with_request_id(RequestId::new(opaque_id("request-1")))
            .with_run_id(RunId::new(opaque_id("run-1")))
            .with_lease_id(LeaseId::new(opaque_id("lease-1")));
        assert_eq!(
            input_plan_with_links(intent.clone(), success, failure.clone())
                .expect_err("task mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );

        let success = input_links().with_run_id(RunId::new(opaque_id("run-2")));
        assert_eq!(
            input_plan_with_links(intent.clone(), success, failure.clone())
                .expect_err("run mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );

        let success = input_links().with_lease_id(LeaseId::new(opaque_id("lease-2")));
        assert_eq!(
            input_plan_with_links(intent, success, failure).expect_err("lease mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );
    }

    #[test]
    fn critical_panic_payload_is_suppressed_in_subprocess() {
        let output = run_panic_child("critical");
        assert!(output.status.success(), "child status: {:?}", output.status);
        assert!(!child_output(&output).contains(PANIC_SECRET));
    }

    #[test]
    fn critical_panic_hook_forwards_noncritical_panics() {
        let output = run_panic_child("noncritical");
        assert!(output.status.success(), "child status: {:?}", output.status);
        assert!(child_output(&output).contains(NONCRITICAL_HOOK_MARKER));
    }

    #[test]
    fn panic_hook_subprocess_child() {
        match std::env::var(PANIC_CHILD_ENV).as_deref() {
            Ok("critical") => {
                let appender = InMemoryAppender::default();
                let result = execute_critical::<(), &'static str>(&appender, valid_plan(), || {
                    std::panic::panic_any(PANIC_SECRET)
                });
                let CriticalExecutionError::Panicked { outcome } =
                    result.expect_err("panic result")
                else {
                    panic!("expected typed panic result");
                };
                assert_eq!(outcome.event_id(), &event_id("failure"));
            }
            Ok("noncritical") => {
                let appender = InMemoryAppender::default();
                execute_critical::<(), &'static str>(&appender, valid_plan(), || Ok(()))
                    .expect("install critical hook");
                let _ = std::panic::catch_unwind(|| std::panic::panic_any(NONCRITICAL_HOOK_MARKER));
            }
            Ok(mode) => panic!("unknown panic child mode: {mode}"),
            Err(_) => {}
        }
    }

    fn run_panic_child(mode: &str) -> Output {
        let executable = std::env::current_exe().expect("test executable");
        let mut child = Command::new(executable)
            .args([
                "--exact",
                "critical::tests::panic_hook_subprocess_child",
                "--nocapture",
            ])
            .env(PANIC_CHILD_ENV, mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn panic child");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().expect("poll panic child").is_some() {
                return child
                    .wait_with_output()
                    .expect("collect panic child output");
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill timed-out panic child");
                let _ = child.wait();
                panic!("panic child timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn child_output(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }
}
