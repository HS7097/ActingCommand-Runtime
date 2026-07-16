// SPDX-License-Identifier: AGPL-3.0-only

use crate::{GlobalLedger, GlobalLedgerError, GlobalLedgerResult, PersistedEvent};
use actingcommand_contract::{
    EffectDisposition, EventDraft, EventLinks, EventType, SanitizationError, SanitizedEventDraft,
    SecretFingerprinter,
};
use std::fmt;

pub trait EventAppender {
    fn append_durable(&self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent>;
}

impl EventAppender for GlobalLedger {
    fn append_durable(&self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent> {
        self.append(draft)
    }
}

#[derive(Clone, PartialEq)]
pub struct CriticalReceipt<T> {
    value: T,
    intent: PersistedEvent,
    outcome: PersistedEvent,
}

impl<T> fmt::Debug for CriticalReceipt<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CriticalReceipt")
            .field("value", &"<redacted-action-value>")
            .field("intent_sequence", &self.intent.sequence())
            .field("outcome_sequence", &self.outcome.sequence())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefiniteEffectDisposition {
    NotPerformed,
    Performed,
}

impl From<DefiniteEffectDisposition> for EffectDisposition {
    fn from(value: DefiniteEffectDisposition) -> Self {
        match value {
            DefiniteEffectDisposition::NotPerformed => Self::NotPerformed,
            DefiniteEffectDisposition::Performed => Self::Performed,
        }
    }
}

pub enum CriticalActionReport<T, E> {
    Succeeded {
        value: T,
        effect: DefiniteEffectDisposition,
    },
    Failed {
        error: E,
        effect: EffectDisposition,
    },
}

impl<T, E> fmt::Debug for CriticalActionReport<T, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded { effect, .. } => formatter
                .debug_struct("Succeeded")
                .field("value", &"<redacted-action-value>")
                .field("effect", effect)
                .finish(),
            Self::Failed { effect, .. } => formatter
                .debug_struct("Failed")
                .field("error", &"<redacted-action-error>")
                .field("effect", effect)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseTransitionTarget {
    Granted,
    Transferred,
    Renewed,
    Released,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTerminalTarget {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogTransitionTarget {
    Activated,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriticalOperation {
    CommandValidation,
    DeviceWrite,
    ApplicationLifecycle,
    LeaseTransition(LeaseTransitionTarget),
    TaskTerminal(TaskTerminalTarget),
    PolicyDispatch,
    CatalogTransition(CatalogTransitionTarget),
}

impl CriticalOperation {
    fn intent_type(self) -> EventType {
        match self {
            Self::CommandValidation => EventType::CommandReceived,
            Self::DeviceWrite => EventType::InputIntent,
            Self::ApplicationLifecycle => EventType::ApplicationIntent,
            Self::LeaseTransition(_) => EventType::LeaseTransitionIntent,
            Self::TaskTerminal(_) => EventType::TaskTerminalIntent,
            Self::PolicyDispatch => EventType::PolicyDispatchIntent,
            Self::CatalogTransition(_) => EventType::CatalogTransitionIntent,
        }
    }

    fn success_type(self) -> EventType {
        match self {
            Self::CommandValidation => EventType::CommandValidated,
            Self::DeviceWrite => EventType::InputCommitted,
            Self::ApplicationLifecycle => EventType::ApplicationCompleted,
            Self::LeaseTransition(target) => match target {
                LeaseTransitionTarget::Granted => EventType::LeaseGranted,
                LeaseTransitionTarget::Transferred => EventType::LeaseTransferred,
                LeaseTransitionTarget::Renewed => EventType::LeaseRenewed,
                LeaseTransitionTarget::Released => EventType::LeaseReleased,
                LeaseTransitionTarget::Expired => EventType::LeaseExpired,
            },
            Self::TaskTerminal(target) => match target {
                TaskTerminalTarget::Completed => EventType::TaskCompleted,
                TaskTerminalTarget::Failed => EventType::TaskFailed,
                TaskTerminalTarget::Cancelled => EventType::TaskCancelled,
            },
            Self::PolicyDispatch => EventType::PolicyDispatchAdmitted,
            Self::CatalogTransition(target) => match target {
                CatalogTransitionTarget::Activated => EventType::CatalogActivated,
                CatalogTransitionTarget::RolledBack => EventType::CatalogRolledBack,
            },
        }
    }

    fn failure_type(self) -> EventType {
        match self {
            Self::CommandValidation => EventType::CommandRejected,
            Self::DeviceWrite => EventType::InputFailed,
            Self::ApplicationLifecycle => EventType::ApplicationFailed,
            Self::LeaseTransition(_) => EventType::LeaseTransitionFailed,
            Self::TaskTerminal(_) => EventType::TaskTerminalCommitFailed,
            Self::PolicyDispatch => EventType::PolicyDispatchRejected,
            Self::CatalogTransition(_) => EventType::CatalogTransitionFailed,
        }
    }
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
    EffectDispositionMismatch,
    PayloadActionMismatch,
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
            Self::EffectDispositionMismatch => {
                "critical outcome effect does not match the action report"
            }
            Self::PayloadActionMismatch => {
                "critical outcome action does not match the intent action"
            }
        };
        formatter.write_str(message)
    }
}

impl CriticalPlanError {
    const fn code(self) -> &'static str {
        match self {
            Self::EventFamilyMismatch => "critical_event_family_mismatch",
            Self::MissingCorrelationId => "critical_missing_correlation_id",
            Self::CorrelationIdMismatch => "critical_correlation_id_mismatch",
            Self::MissingActionId => "critical_missing_action_id",
            Self::ActionIdMismatch => "critical_action_id_mismatch",
            Self::StableIdentityLinkMismatch => "critical_stable_link_mismatch",
            Self::DuplicateEventType => "critical_duplicate_event_type",
            Self::DuplicateEventId => "critical_duplicate_event_id",
            Self::UnsupportedIntent => "critical_unsupported_intent",
            Self::OutcomeRoleMismatch => "critical_outcome_role_mismatch",
            Self::EffectDispositionMismatch => "critical_effect_disposition_mismatch",
            Self::PayloadActionMismatch => "critical_payload_action_mismatch",
        }
    }
}

impl std::error::Error for CriticalPlanError {}

pub struct CriticalEventPlan {
    operation: CriticalOperation,
    intent: SanitizedEventDraft,
}

impl CriticalEventPlan {
    pub fn new(
        operation: CriticalOperation,
        intent: SanitizedEventDraft,
    ) -> Result<Self, CriticalPlanError> {
        if intent.event_type() != operation.intent_type() {
            return Err(CriticalPlanError::UnsupportedIntent);
        }
        if intent.links().correlation_id().is_none() {
            return Err(CriticalPlanError::MissingCorrelationId);
        }
        if intent.links().action_id().is_none() {
            return Err(CriticalPlanError::MissingActionId);
        }
        Ok(Self { operation, intent })
    }

    pub const fn operation(&self) -> CriticalOperation {
        self.operation
    }

    pub const fn intent(&self) -> &SanitizedEventDraft {
        &self.intent
    }
}

impl fmt::Debug for CriticalEventPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CriticalEventPlan")
            .field("operation", &self.operation)
            .field("intent_event_type", &self.intent.event_type())
            .field("intent", &"<sanitized-intent>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
enum OutcomeRole {
    Success,
    Failure,
}

fn validate_outcome(
    operation: CriticalOperation,
    intent: &PersistedEvent,
    outcome: &SanitizedEventDraft,
    role: OutcomeRole,
    effect: EffectDisposition,
) -> Result<(), CriticalPlanError> {
    if outcome.event_type().family() != intent.event_type().family() {
        return Err(CriticalPlanError::EventFamilyMismatch);
    }
    let expected_type = match role {
        OutcomeRole::Success => operation.success_type(),
        OutcomeRole::Failure => operation.failure_type(),
    };
    if outcome.event_type() != expected_type {
        return Err(CriticalPlanError::OutcomeRoleMismatch);
    }
    if outcome.event_type() == intent.event_type() {
        return Err(CriticalPlanError::DuplicateEventType);
    }
    if outcome.event_id() == intent.event_id() {
        return Err(CriticalPlanError::DuplicateEventId);
    }
    validate_matching_link(
        intent.links().correlation_id(),
        outcome.links().correlation_id(),
        CriticalPlanError::MissingCorrelationId,
        CriticalPlanError::CorrelationIdMismatch,
    )?;
    validate_matching_link(
        intent.links().action_id(),
        outcome.links().action_id(),
        CriticalPlanError::MissingActionId,
        CriticalPlanError::ActionIdMismatch,
    )?;
    validate_stable_identity_links(intent.links(), outcome.links())?;
    if outcome.payload().action() != intent.payload().action() {
        return Err(CriticalPlanError::PayloadActionMismatch);
    }
    if outcome.payload().effect_disposition() != Some(effect) {
        return Err(CriticalPlanError::EffectDispositionMismatch);
    }
    Ok(())
}

fn validate_matching_link<T: PartialEq>(
    intent: Option<&T>,
    outcome: Option<&T>,
    missing_error: CriticalPlanError,
    mismatch_error: CriticalPlanError,
) -> Result<(), CriticalPlanError> {
    let intent = intent.ok_or(missing_error)?;
    let outcome = outcome.ok_or(missing_error)?;
    if intent != outcome {
        return Err(mismatch_error);
    }
    Ok(())
}

fn validate_stable_identity_links(
    intent: &EventLinks,
    outcome: &EventLinks,
) -> Result<(), CriticalPlanError> {
    if intent.instance_id() != outcome.instance_id()
        || intent.request_id() != outcome.request_id()
        || intent.task_id() != outcome.task_id()
        || intent.run_id() != outcome.run_id()
        || intent.lease_id() != outcome.lease_id()
    {
        return Err(CriticalPlanError::StableIdentityLinkMismatch);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriticalOutcomeStage {
    Build,
    Sanitize,
    Validate,
    Append,
}

pub enum CriticalExecutionError<E> {
    IntentAppend(GlobalLedgerError),
    Action {
        error: E,
        effect: EffectDisposition,
        outcome: Box<PersistedEvent>,
    },
    OutcomeUndurable {
        effect: EffectDisposition,
        stage: CriticalOutcomeStage,
        code: &'static str,
    },
}

impl<E> CriticalExecutionError<E> {
    pub fn is_fatal(&self) -> bool {
        match self {
            Self::IntentAppend(error) => error.is_fatal(),
            Self::Action { .. } => false,
            Self::OutcomeUndurable { .. } => true,
        }
    }
}

impl<E> fmt::Debug for CriticalExecutionError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntentAppend(error) => formatter
                .debug_struct("IntentAppend")
                .field("code", &error.code())
                .field("fatal", &error.is_fatal())
                .finish(),
            Self::Action {
                effect, outcome, ..
            } => formatter
                .debug_struct("Action")
                .field("error", &"<redacted-action-error>")
                .field("effect", effect)
                .field("outcome_sequence", &outcome.sequence())
                .finish(),
            Self::OutcomeUndurable {
                effect,
                stage,
                code,
            } => formatter
                .debug_struct("OutcomeUndurable")
                .field("effect", effect)
                .field("stage", stage)
                .field("code", code)
                .finish(),
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
            Self::OutcomeUndurable {
                effect,
                stage,
                code,
            } => write!(
                formatter,
                "critical action outcome is undurable at {stage:?} with {effect:?} ({code})"
            ),
        }
    }
}

impl<E> std::error::Error for CriticalExecutionError<E> {}

pub fn execute_critical<T, E, SB, FB>(
    appender: &impl EventAppender,
    fingerprinter: &dyn SecretFingerprinter,
    plan: CriticalEventPlan,
    action: impl FnOnce() -> CriticalActionReport<T, E>,
    success_builder: SB,
    failure_builder: FB,
) -> Result<CriticalReceipt<T>, CriticalExecutionError<E>>
where
    SB: FnOnce(&T, DefiniteEffectDisposition) -> Result<EventDraft, SanitizationError>,
    FB: FnOnce(&E, EffectDisposition) -> Result<EventDraft, SanitizationError>,
{
    let CriticalEventPlan { operation, intent } = plan;
    let intent = appender
        .append_durable(intent)
        .map_err(CriticalExecutionError::IntentAppend)?;

    match action() {
        CriticalActionReport::Succeeded { value, effect } => {
            let effect_disposition = effect.into();
            let outcome = persist_outcome(
                appender,
                fingerprinter,
                operation,
                &intent,
                OutcomeRole::Success,
                effect_disposition,
                success_builder(&value, effect),
            )
            .map_err(|(stage, code)| CriticalExecutionError::OutcomeUndurable {
                effect: effect_disposition,
                stage,
                code,
            })?;
            Ok(CriticalReceipt {
                value,
                intent,
                outcome,
            })
        }
        CriticalActionReport::Failed { error, effect } => {
            let outcome = persist_outcome(
                appender,
                fingerprinter,
                operation,
                &intent,
                OutcomeRole::Failure,
                effect,
                failure_builder(&error, effect),
            )
            .map_err(|(stage, code)| CriticalExecutionError::OutcomeUndurable {
                effect,
                stage,
                code,
            })?;
            Err(CriticalExecutionError::Action {
                error,
                effect,
                outcome: Box::new(outcome),
            })
        }
    }
}

fn persist_outcome(
    appender: &impl EventAppender,
    fingerprinter: &dyn SecretFingerprinter,
    operation: CriticalOperation,
    intent: &PersistedEvent,
    role: OutcomeRole,
    effect: EffectDisposition,
    built: Result<EventDraft, SanitizationError>,
) -> Result<PersistedEvent, (CriticalOutcomeStage, &'static str)> {
    let draft = built.map_err(|error| (CriticalOutcomeStage::Build, error.code()))?;
    let draft = draft
        .sanitize(fingerprinter)
        .map_err(|error| (CriticalOutcomeStage::Sanitize, error.code()))?;
    validate_outcome(operation, intent, &draft, role, effect)
        .map_err(|error| (CriticalOutcomeStage::Validate, error.code()))?;
    appender
        .append_durable(draft)
        .map_err(|error| (CriticalOutcomeStage::Append, error.code()))
}

#[cfg(test)]
#[path = "critical/tests.rs"]
mod tests;
