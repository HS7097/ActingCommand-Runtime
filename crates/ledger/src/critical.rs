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

#[derive(Debug)]
pub enum CriticalAction<T, E> {
    Succeeded {
        value: T,
        outcome: ErasedSanitizedEventDraft,
    },
    Failed {
        error: E,
        outcome: ErasedSanitizedEventDraft,
    },
}

#[derive(Debug)]
pub enum CriticalExecutionError<E> {
    IntentAppend(GlobalLedgerError),
    Action {
        error: E,
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
    intent: ErasedSanitizedEventDraft,
    action: impl FnOnce() -> CriticalAction<T, E>,
) -> Result<CriticalReceipt<T>, CriticalExecutionError<E>> {
    let intent = appender
        .append_durable(intent)
        .map_err(CriticalExecutionError::IntentAppend)?;

    match action() {
        CriticalAction::Succeeded { value, outcome } => appender
            .append_durable(outcome)
            .map(|outcome| CriticalReceipt {
                value,
                intent,
                outcome,
            })
            .map_err(|source| CriticalExecutionError::Indeterminate {
                action_performed: true,
                source,
            }),
        CriticalAction::Failed { error, outcome } => match appender.append_durable(outcome) {
            Ok(outcome) => Err(CriticalExecutionError::Action {
                error,
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
    use super::{CriticalAction, CriticalExecutionError, EventAppender, execute_critical};
    use crate::{GlobalLedgerError, GlobalLedgerResult};
    use actingcommand_contract::{
        CommandPayloadDraft, CommandStage, ErasedSanitizedEventDraft, EventActor, EventDraft,
        EventLinks, EventOrigin, EventSeverity, EventSource, EventType, PersistedEvent,
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

    fn draft(event_id: &str) -> ErasedSanitizedEventDraft {
        EventDraft::new(
            event_id,
            1_752_147_200_000,
            EventType::CommandReceived,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Runtime, "runtime", EventActor::Runtime).expect("origin"),
            EventLinks::default(),
            CommandPayloadDraft::new(CommandStage::Received, "critical.test", vec![])
                .expect("payload"),
        )
        .sanitize(&crate::Sha256FieldRedactor::new(b"test-private-salt").expect("redactor"))
        .expect("sanitize")
        .erase()
        .expect("erase")
    }

    fn append_failure() -> GlobalLedgerError {
        crate::Sha256FieldRedactor::new(b"").expect_err("empty redactor salt must fail")
    }

    #[test]
    fn intent_append_failure_prevents_action() {
        let appender = InMemoryAppender::default();
        appender.fail_on_call(1, append_failure());
        let action_calls = Cell::new(0);

        let result = execute_critical(&appender, draft("intent"), || {
            action_calls.set(action_calls.get() + 1);
            CriticalAction::<_, &'static str>::Succeeded {
                value: (),
                outcome: draft("success"),
            }
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

        let receipt = execute_critical(&appender, draft("intent"), || {
            action_calls.set(action_calls.get() + 1);
            CriticalAction::<_, &'static str>::Succeeded {
                value: "done",
                outcome: draft("success"),
            }
        })
        .expect("durable success receipt");

        assert_eq!(receipt.value(), &"done");
        assert_eq!(receipt.outcome().event_id, "success");
        assert_eq!(action_calls.get(), 1);
        assert_eq!(appender.calls(), vec!["intent", "success"]);
    }

    #[test]
    fn failed_action_requires_durable_failure_outcome() {
        let appender = InMemoryAppender::default();
        let action_calls = Cell::new(0);

        let result = execute_critical(&appender, draft("intent"), || {
            action_calls.set(action_calls.get() + 1);
            CriticalAction::<(), _>::Failed {
                error: "action failed",
                outcome: draft("failure"),
            }
        });

        let CriticalExecutionError::Action { error, outcome } = result.expect_err("action failure")
        else {
            panic!("expected durable action failure");
        };
        assert_eq!(error, "action failed");
        assert_eq!(outcome.event_id, "failure");
        assert_eq!(action_calls.get(), 1);
        assert_eq!(appender.calls(), vec!["intent", "failure"]);
    }

    #[test]
    fn outcome_append_failure_returns_indeterminate_fatal_without_success_receipt() {
        let appender = InMemoryAppender::default();
        appender.fail_on_call(2, append_failure());
        let action_calls = Cell::new(0);

        let result = execute_critical(&appender, draft("intent"), || {
            action_calls.set(action_calls.get() + 1);
            CriticalAction::<_, &'static str>::Succeeded {
                value: (),
                outcome: draft("success"),
            }
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
        assert_eq!(appender.calls(), vec!["intent", "success"]);
    }

    #[test]
    fn successful_path_orders_intent_action_outcome() {
        let appender = InMemoryAppender::default();

        execute_critical(&appender, draft("intent"), || {
            appender.order.borrow_mut().push("action".to_string());
            CriticalAction::<_, &'static str>::Succeeded {
                value: (),
                outcome: draft("success"),
            }
        })
        .expect("success");

        assert_eq!(appender.order(), vec!["intent", "action", "success"]);
    }
}
