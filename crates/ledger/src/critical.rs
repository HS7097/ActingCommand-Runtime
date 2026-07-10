// SPDX-License-Identifier: AGPL-3.0-only

use crate::{GlobalLedger, GlobalLedgerError, GlobalLedgerResult};
use actingcommand_contract::{ErasedSanitizedEventDraft, PersistedEvent};
use std::fmt;

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
    DuplicateEventType,
    DuplicateEventId,
}

impl fmt::Display for CriticalPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EventFamilyMismatch => "critical event families must match",
            Self::MissingCorrelationId => "critical events require correlation ids",
            Self::CorrelationIdMismatch => "critical event correlation ids must match",
            Self::MissingActionId => "critical events require action ids",
            Self::ActionIdMismatch => "critical event action ids must match",
            Self::DuplicateEventType => "critical event types must be distinct",
            Self::DuplicateEventId => "critical event ids must be distinct",
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

        Ok(Self {
            intent,
            success_outcome,
            failure_outcome,
        })
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

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
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
            EventLinks {
                correlation_id: correlation_id.map(str::to_string),
                action_id: action_id.map(str::to_string),
                ..EventLinks::default()
            },
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
}
