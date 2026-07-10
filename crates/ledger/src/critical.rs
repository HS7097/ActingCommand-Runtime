// SPDX-License-Identifier: AGPL-3.0-only

use crate::{GlobalLedger, GlobalLedgerError, GlobalLedgerResult};
use actingcommand_contract::{ErasedSanitizedEventDraft, EventType, PersistedEvent};
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
    fn append_durable(
        &self,
        draft: ErasedSanitizedEventDraft,
    ) -> GlobalLedgerResult<PersistedEvent>;
}

impl EventAppender for GlobalLedger {
    fn append_durable(
        &self,
        draft: ErasedSanitizedEventDraft,
    ) -> GlobalLedgerResult<PersistedEvent> {
        self.append_erased(draft)
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
    intent: ErasedSanitizedEventDraft,
    success_outcome: ErasedSanitizedEventDraft,
    failure_outcome: ErasedSanitizedEventDraft,
}

impl CriticalEventPlan {
    pub fn new(
        intent: ErasedSanitizedEventDraft,
        success_outcome: ErasedSanitizedEventDraft,
        failure_outcome: ErasedSanitizedEventDraft,
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
            intent.links().correlation_id.as_deref(),
            success_outcome.links().correlation_id.as_deref(),
            failure_outcome.links().correlation_id.as_deref(),
            CriticalPlanError::MissingCorrelationId,
            CriticalPlanError::CorrelationIdMismatch,
        )?;
        validate_matching_link(
            intent.links().action_id.as_deref(),
            success_outcome.links().action_id.as_deref(),
            failure_outcome.links().action_id.as_deref(),
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

fn validate_matching_link(
    intent: Option<&str>,
    success: Option<&str>,
    failure: Option<&str>,
    missing_error: CriticalPlanError,
    mismatch_error: CriticalPlanError,
) -> Result<(), CriticalPlanError> {
    let intent = intent
        .filter(|value| !value.is_empty())
        .ok_or(missing_error)?;
    let success = success
        .filter(|value| !value.is_empty())
        .ok_or(missing_error)?;
    let failure = failure
        .filter(|value| !value.is_empty())
        .ok_or(missing_error)?;
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
    for (intent, success, failure) in [
        (
            intent.instance_id.as_deref(),
            success.instance_id.as_deref(),
            failure.instance_id.as_deref(),
        ),
        (
            intent.request_id.as_deref(),
            success.request_id.as_deref(),
            failure.request_id.as_deref(),
        ),
        (
            intent.task_id.as_deref(),
            success.task_id.as_deref(),
            failure.task_id.as_deref(),
        ),
        (
            intent.run_id.as_deref(),
            success.run_id.as_deref(),
            failure.run_id.as_deref(),
        ),
        (
            intent.lease_id.as_deref(),
            success.lease_id.as_deref(),
            failure.lease_id.as_deref(),
        ),
    ] {
        if intent != success || intent != failure {
            return Err(CriticalPlanError::StableIdentityLinkMismatch);
        }
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
    use crate::{GlobalLedgerError, GlobalLedgerResult};
    use actingcommand_contract::{
        CommandPayloadDraft, CommandStage, ErasedSanitizedEventDraft, EventActor, EventDraft,
        EventLinks, EventOrigin, EventSeverity, EventSource, EventType, InputPayloadDraft,
        InputTransition, PersistedEvent,
    };
    use std::cell::{Cell, RefCell};
    use std::process::{Command, Output, Stdio};
    use std::time::{Duration, Instant};

    const PANIC_CHILD_ENV: &str = "ACTINGCOMMAND_CRITICAL_PANIC_CHILD";
    const PANIC_SECRET: &str = "critical-panic-secret-4e53e895";
    const NONCRITICAL_HOOK_MARKER: &str = "noncritical-hook-marker-c29a6953";

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
        fn append_durable(
            &self,
            draft: ErasedSanitizedEventDraft,
        ) -> GlobalLedgerResult<PersistedEvent> {
            let call = self.append_count.get() + 1;
            self.append_count.set(call);
            let event_id = PersistedEvent::from_draft(0, draft.clone()).event_id;
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
            Ok(PersistedEvent::from_draft(call, draft))
        }
    }

    fn input_draft(
        event_id: &str,
        transition: InputTransition,
        correlation_id: Option<&str>,
        action_id: Option<&str>,
    ) -> ErasedSanitizedEventDraft {
        input_draft_with_links(
            event_id,
            transition,
            EventLinks {
                correlation_id: correlation_id.map(str::to_string),
                action_id: action_id.map(str::to_string),
                ..EventLinks::default()
            },
        )
    }

    fn input_draft_with_links(
        event_id: &str,
        transition: InputTransition,
        links: EventLinks,
    ) -> ErasedSanitizedEventDraft {
        let event_type = match transition {
            InputTransition::Intent => EventType::InputIntent,
            InputTransition::Committed => EventType::InputCommitted,
            InputTransition::Completed => EventType::InputCompleted,
            InputTransition::Failed => EventType::InputFailed,
        };
        EventDraft::new(
            event_id,
            1_752_147_200_000,
            event_type,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Runtime, "runtime", EventActor::Runtime).expect("origin"),
            links,
            InputPayloadDraft::new(transition, "input.tap", vec![]).expect("payload"),
        )
        .sanitize(&crate::Sha256FieldRedactor::new(b"test-private-salt").expect("redactor"))
        .expect("sanitize")
        .erase()
        .expect("erase")
    }

    fn command_draft(
        event_id: &str,
        correlation_id: &str,
        action_id: &str,
    ) -> ErasedSanitizedEventDraft {
        EventDraft::new(
            event_id,
            1_752_147_200_000,
            EventType::CommandReceived,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Runtime, "runtime", EventActor::Runtime).expect("origin"),
            EventLinks {
                correlation_id: Some(correlation_id.to_string()),
                action_id: Some(action_id.to_string()),
                ..EventLinks::default()
            },
            CommandPayloadDraft::new(CommandStage::Received, "critical.test", vec![])
                .expect("payload"),
        )
        .sanitize(&crate::Sha256FieldRedactor::new(b"test-private-salt").expect("redactor"))
        .expect("sanitize")
        .erase()
        .expect("erase")
    }

    fn valid_plan() -> CriticalEventPlan {
        CriticalEventPlan::new(
            input_draft(
                "intent",
                InputTransition::Intent,
                Some("correlation-1"),
                Some("action-1"),
            ),
            input_draft(
                "success",
                InputTransition::Completed,
                Some("correlation-1"),
                Some("action-1"),
            ),
            input_draft(
                "failure",
                InputTransition::Failed,
                Some("correlation-1"),
                Some("action-1"),
            ),
        )
        .expect("valid input critical plan")
    }

    fn input_links() -> EventLinks {
        EventLinks {
            correlation_id: Some("correlation-1".to_string()),
            action_id: Some("action-1".to_string()),
            instance_id: Some("instance-1".to_string()),
            request_id: Some("request-1".to_string()),
            task_id: Some("task-1".to_string()),
            run_id: Some("run-1".to_string()),
            lease_id: Some("lease-1".to_string()),
            ..EventLinks::default()
        }
    }

    fn input_plan_with_links(
        intent_links: EventLinks,
        success_links: EventLinks,
        failure_links: EventLinks,
    ) -> Result<CriticalEventPlan, CriticalPlanError> {
        CriticalEventPlan::new(
            input_draft_with_links("intent", InputTransition::Intent, intent_links),
            input_draft_with_links("success", InputTransition::Completed, success_links),
            input_draft_with_links("failure", InputTransition::Failed, failure_links),
        )
    }

    fn append_failure() -> GlobalLedgerError {
        crate::Sha256FieldRedactor::new(b"").expect_err("empty redactor salt must fail")
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
        assert_eq!(receipt.outcome().event_id, "success");
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
        assert_eq!(outcome.event_id, "failure");
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

        assert_eq!(receipt.intent().event_type, EventType::InputIntent);
        assert_eq!(receipt.outcome().event_type, EventType::InputCompleted);
        assert_eq!(appender.calls(), vec!["intent", "success"]);
    }

    #[test]
    fn critical_plan_rejects_contract_mismatches_before_execution() {
        let appender = InMemoryAppender::default();
        let action_calls = Cell::new(0);
        let intent = || {
            input_draft(
                "intent",
                InputTransition::Intent,
                Some("correlation-1"),
                Some("action-1"),
            )
        };
        let failure = || {
            input_draft(
                "failure",
                InputTransition::Failed,
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
                input_draft(
                    "success",
                    InputTransition::Completed,
                    None,
                    Some("action-1")
                ),
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
                    InputTransition::Completed,
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
                input_draft(
                    "success",
                    InputTransition::Completed,
                    Some("correlation-1"),
                    None
                ),
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
                    InputTransition::Completed,
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
                    InputTransition::Intent,
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
                    InputTransition::Completed,
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
        assert_eq!(outcome.event_id, "failure");
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
                InputTransition::Intent,
                Some("correlation-1"),
                Some("action-1"),
            ),
            input_draft(
                "success-committed",
                InputTransition::Committed,
                Some("correlation-1"),
                Some("action-1"),
            ),
            input_draft(
                "failure-committed",
                InputTransition::Failed,
                Some("correlation-1"),
                Some("action-1"),
            ),
        )
        .expect("input committed triplet must be supported");
        assert_eq!(
            CriticalEventPlan::new(
                input_draft(
                    "intent",
                    InputTransition::Intent,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                input_draft(
                    "success",
                    InputTransition::Failed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                input_draft(
                    "failure",
                    InputTransition::Completed,
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
                    InputTransition::Committed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                input_draft(
                    "success",
                    InputTransition::Completed,
                    Some("correlation-1"),
                    Some("action-1"),
                ),
                input_draft(
                    "failure",
                    InputTransition::Failed,
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
                "success",
                1_752_147_200_000,
                EventType::CommandValidated,
                EventSeverity::Info,
                EventOrigin::new(EventSource::Runtime, "runtime", EventActor::Runtime)
                    .expect("origin"),
                EventLinks {
                    correlation_id: Some("correlation-1".to_string()),
                    action_id: Some("action-1".to_string()),
                    ..EventLinks::default()
                },
                CommandPayloadDraft::new(CommandStage::Validated, "critical.test", vec![])
                    .expect("payload"),
            )
            .sanitize(&crate::Sha256FieldRedactor::new(b"test-private-salt").expect("redactor"))
            .expect("sanitize")
            .erase()
            .expect("erase"),
            EventDraft::new(
                "failure",
                1_752_147_200_000,
                EventType::CommandRejected,
                EventSeverity::Info,
                EventOrigin::new(EventSource::Runtime, "runtime", EventActor::Runtime)
                    .expect("origin"),
                EventLinks {
                    correlation_id: Some("correlation-1".to_string()),
                    action_id: Some("action-1".to_string()),
                    ..EventLinks::default()
                },
                CommandPayloadDraft::new(CommandStage::Rejected, "critical.test", vec![])
                    .expect("payload"),
            )
            .sanitize(&crate::Sha256FieldRedactor::new(b"test-private-salt").expect("redactor"))
            .expect("sanitize")
            .erase()
            .expect("erase"),
        )
        .expect("command role triplet must be supported");
        assert_eq!(action_calls.get(), 0);
        assert!(appender.calls().is_empty());
    }

    #[test]
    fn critical_plan_rejects_stable_identity_link_mismatches() {
        let intent = input_links();
        let failure = input_links();

        let mut success = input_links();
        success.instance_id = Some("instance-2".to_string());
        assert_eq!(
            input_plan_with_links(intent.clone(), success, failure.clone())
                .expect_err("instance mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );

        let mut success = input_links();
        success.request_id = Some("request-2".to_string());
        assert_eq!(
            input_plan_with_links(intent.clone(), success, failure.clone())
                .expect_err("request mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );

        let mut success = input_links();
        success.task_id = None;
        assert_eq!(
            input_plan_with_links(intent.clone(), success, failure.clone())
                .expect_err("task mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );

        let mut success = input_links();
        success.run_id = Some("run-2".to_string());
        assert_eq!(
            input_plan_with_links(intent.clone(), success, failure.clone())
                .expect_err("run mismatch"),
            CriticalPlanError::StableIdentityLinkMismatch
        );

        let mut success = input_links();
        success.lease_id = Some("lease-2".to_string());
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
                assert_eq!(outcome.event_id, "failure");
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
