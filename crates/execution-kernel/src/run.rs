// SPDX-License-Identifier: AGPL-3.0-only

//! Pure run coordination and successor suggestion decisions.

use serde::Serialize;
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunDecisionError {
    code: &'static str,
    message: String,
}

impl RunDecisionError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "run_decision_invalid",
            message: message.into(),
        }
    }

    fn invalid_transition(message: impl Into<String>) -> Self {
        Self {
            code: "run_transition_invalid",
            message: message.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RunDecisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RunDecisionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOperationCandidate {
    id: String,
    from_page: String,
}

impl RunOperationCandidate {
    pub fn new(
        id: impl Into<String>,
        from_page: impl Into<String>,
    ) -> Result<Self, RunDecisionError> {
        let candidate = Self {
            id: id.into(),
            from_page: from_page.into(),
        };
        if candidate.id.trim().is_empty() {
            return Err(RunDecisionError::invalid(
                "run operation candidate id must not be empty",
            ));
        }
        if candidate.from_page.trim().is_empty() {
            return Err(RunDecisionError::invalid(format!(
                "run operation '{}' from_page must not be empty",
                candidate.id
            )));
        }
        Ok(candidate)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn from_page(&self) -> &str {
        &self.from_page
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOperationPolicy {
    retryable: bool,
    max_attempts: u32,
    retry_interval_ms: u64,
    recovery_task_id: Option<String>,
}

impl RunOperationPolicy {
    pub fn new(
        retryable: bool,
        max_attempts: u32,
        retry_interval_ms: u64,
        recovery_task_id: Option<String>,
    ) -> Result<Self, RunDecisionError> {
        if max_attempts == 0 {
            return Err(RunDecisionError::invalid(
                "run operation max_attempts must be greater than zero",
            ));
        }
        if retry_interval_ms == 0 {
            return Err(RunDecisionError::invalid(
                "run operation retry_interval_ms must be greater than zero",
            ));
        }
        if recovery_task_id
            .as_deref()
            .is_some_and(|task_id| task_id.trim().is_empty())
        {
            return Err(RunDecisionError::invalid(
                "run operation recovery_task_id must not be empty",
            ));
        }
        Ok(Self {
            retryable,
            max_attempts: if retryable { max_attempts } else { 1 },
            retry_interval_ms,
            recovery_task_id,
        })
    }

    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    pub const fn retry_interval_ms(&self) -> u64 {
        self.retry_interval_ms
    }

    pub fn recovery_task_id(&self) -> Option<&str> {
        self.recovery_task_id.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFailureStage {
    PreExecutionGuard,
    PostExecution { hit_error_page: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFailureObservation {
    operation_id: String,
    attempt: u32,
    reason: String,
    after_page: Option<String>,
    stage: RunFailureStage,
}

impl RunFailureObservation {
    pub fn new(
        operation_id: impl Into<String>,
        attempt: u32,
        reason: impl Into<String>,
        after_page: Option<String>,
        stage: RunFailureStage,
    ) -> Result<Self, RunDecisionError> {
        let observation = Self {
            operation_id: operation_id.into(),
            attempt,
            reason: reason.into(),
            after_page,
            stage,
        };
        if observation.operation_id.trim().is_empty() {
            return Err(RunDecisionError::invalid(
                "run failure operation_id must not be empty",
            ));
        }
        if observation.attempt == 0 {
            return Err(RunDecisionError::invalid(
                "run failure attempt must be greater than zero",
            ));
        }
        if observation.reason.trim().is_empty() {
            return Err(RunDecisionError::invalid(
                "run failure reason must not be empty",
            ));
        }
        Ok(observation)
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn after_page(&self) -> Option<&str> {
        self.after_page.as_deref()
    }

    pub const fn stage(&self) -> RunFailureStage {
        self.stage
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunRecoveryTrigger {
    pub operation_id: String,
    pub reason: String,
    pub after_page: Option<String>,
    pub attempts: u32,
    pub recovery_task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunFailure {
    pub operation_id: String,
    pub reason: String,
    pub after_page: Option<String>,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOperationFailureDecision {
    Retry { next_attempt: u32, delay_ms: u64 },
    RequestRecovery(RunRecoveryTrigger),
    Fail(RunFailure),
}

pub fn decide_run_operation_failure(
    policy: &RunOperationPolicy,
    observation: RunFailureObservation,
) -> Result<RunOperationFailureDecision, RunDecisionError> {
    if observation.attempt > policy.max_attempts {
        return Err(RunDecisionError::invalid(format!(
            "run operation '{}' attempt {} exceeds max_attempts {}",
            observation.operation_id, observation.attempt, policy.max_attempts
        )));
    }

    let request_recovery = match observation.stage {
        RunFailureStage::PreExecutionGuard => {
            observation.attempt > 1 && policy.retryable && policy.recovery_task_id.is_some()
        }
        RunFailureStage::PostExecution {
            hit_error_page: false,
        } if policy.retryable && observation.attempt < policy.max_attempts => {
            return Ok(RunOperationFailureDecision::Retry {
                next_attempt: observation.attempt + 1,
                delay_ms: policy.retry_interval_ms,
            });
        }
        RunFailureStage::PostExecution { .. } => {
            policy.retryable && policy.recovery_task_id.is_some()
        }
    };

    if request_recovery {
        let recovery_task_id = policy.recovery_task_id.clone().ok_or_else(|| {
            RunDecisionError::invalid_transition(
                "recovery decision requires a configured recovery task id",
            )
        })?;
        return Ok(RunOperationFailureDecision::RequestRecovery(
            RunRecoveryTrigger {
                operation_id: observation.operation_id,
                reason: observation.reason,
                after_page: observation.after_page,
                attempts: match observation.stage {
                    RunFailureStage::PreExecutionGuard => observation.attempt - 1,
                    RunFailureStage::PostExecution { .. } => observation.attempt,
                },
                recovery_task_id,
            },
        ));
    }

    Ok(RunOperationFailureDecision::Fail(RunFailure {
        operation_id: observation.operation_id,
        reason: observation.reason,
        after_page: observation.after_page,
        attempts: observation.attempt,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStateConfig {
    game: String,
    target_page: Option<String>,
    stop_on_confirmation: bool,
    max_task_retries: u32,
    max_steps: u32,
}

impl RunStateConfig {
    pub fn new(
        game: impl Into<String>,
        target_page: Option<String>,
        stop_on_confirmation: bool,
        max_task_retries: u32,
        max_steps: u32,
    ) -> Result<Self, RunDecisionError> {
        let config = Self {
            game: game.into(),
            target_page,
            stop_on_confirmation,
            max_task_retries,
            max_steps,
        };
        if config.game.trim().is_empty() {
            return Err(RunDecisionError::invalid(
                "run state game must not be empty",
            ));
        }
        if config.max_task_retries == 0 {
            return Err(RunDecisionError::invalid(
                "run state max_task_retries must be greater than zero",
            ));
        }
        if config.max_steps == 0 {
            return Err(RunDecisionError::invalid(
                "run state max_steps must be greater than zero",
            ));
        }
        if config
            .target_page
            .as_deref()
            .is_some_and(|page| page.trim().is_empty())
        {
            return Err(RunDecisionError::invalid(
                "run state target_page must not be empty",
            ));
        }
        Ok(config)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPauseReason {
    RecoveryExhausted,
    MaxStepsExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunPause {
    pub reason: RunPauseReason,
    pub operation_id: Option<String>,
    pub failure_reason: Option<String>,
    pub completed_task_retries: u32,
    pub max_task_retries: u32,
    pub completed_steps: u32,
    pub max_steps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunSuccessorKind {
    ReturnHomeRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunSuccessorSuggestion {
    pub kind: RunSuccessorKind,
    pub task_id: String,
    pub source_operation_id: String,
    pub failure_reason: String,
    pub after_page: Option<String>,
    pub requested_task_retry: u32,
    pub max_task_retries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunTerminal {
    Completed { current_page: Option<String> },
    SuccessorSuggested { suggestion: RunSuccessorSuggestion },
    PausedNeedsHuman { pause: RunPause },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunDirective {
    AwaitPage,
    ExecuteOperation {
        operation_id: String,
        current_page: String,
        step_index: u32,
    },
    Continue {
        current_page: String,
    },
    Terminal(RunTerminal),
}

/// Owns pure run transitions; effect adapters execute directives and return observations.
#[derive(Debug, Clone)]
pub struct RunStateMachine {
    config: RunStateConfig,
    completed_task_retries: u32,
    completed_steps: u32,
    current_page: Option<String>,
    executing_operation: Option<String>,
    terminal: Option<RunTerminal>,
}

impl RunStateMachine {
    pub fn new(
        config: RunStateConfig,
        completed_task_retries: u32,
    ) -> Result<Self, RunDecisionError> {
        if completed_task_retries > config.max_task_retries {
            return Err(RunDecisionError::invalid(format!(
                "completed_task_retries {completed_task_retries} exceeds max_task_retries {}",
                config.max_task_retries
            )));
        }
        Ok(Self {
            config,
            completed_task_retries,
            completed_steps: 0,
            current_page: None,
            executing_operation: None,
            terminal: None,
        })
    }

    pub fn observe_page(&mut self, current_page: Option<String>) -> Result<(), RunDecisionError> {
        if self.terminal.is_some() {
            return Err(RunDecisionError::invalid_transition(
                "cannot observe a page after run reached a terminal state",
            ));
        }
        if self.executing_operation.is_some() {
            return Err(RunDecisionError::invalid_transition(
                "cannot replace current page while an operation outcome is pending",
            ));
        }
        self.current_page = current_page;
        Ok(())
    }

    pub fn next_directive(
        &mut self,
        operations: &[RunOperationCandidate],
    ) -> Result<RunDirective, RunDecisionError> {
        if let Some(terminal) = &self.terminal {
            return Ok(RunDirective::Terminal(terminal.clone()));
        }
        if let Some(operation_id) = &self.executing_operation {
            return Err(RunDecisionError::invalid_transition(format!(
                "operation '{operation_id}' still requires an outcome"
            )));
        }
        let Some(current_page) = self.current_page.clone() else {
            return Ok(RunDirective::AwaitPage);
        };
        if self.config.stop_on_confirmation
            && self
                .config
                .target_page
                .as_deref()
                .is_some_and(|target| page_anchor_matches(&self.config.game, &current_page, target))
        {
            return Ok(self.finish(RunTerminal::Completed {
                current_page: Some(current_page),
            }));
        }
        if self.completed_steps >= self.config.max_steps {
            return Ok(self.finish(RunTerminal::PausedNeedsHuman {
                pause: RunPause {
                    reason: RunPauseReason::MaxStepsExhausted,
                    operation_id: None,
                    failure_reason: None,
                    completed_task_retries: self.completed_task_retries,
                    max_task_retries: self.config.max_task_retries,
                    completed_steps: self.completed_steps,
                    max_steps: self.config.max_steps,
                },
            }));
        }
        let operation = select_run_operation(&self.config.game, &current_page, operations)
            .ok_or_else(|| {
                RunDecisionError::invalid(format!(
                    "no operation can continue from page '{current_page}'"
                ))
            })?;
        let step_index = self.completed_steps;
        self.completed_steps += 1;
        self.executing_operation = Some(operation.id.clone());
        Ok(RunDirective::ExecuteOperation {
            operation_id: operation.id.clone(),
            current_page,
            step_index,
        })
    }

    pub fn operation_succeeded(
        &mut self,
        operation_id: &str,
        current_page: Option<String>,
    ) -> Result<RunDirective, RunDecisionError> {
        self.require_executing(operation_id)?;
        self.executing_operation = None;
        self.current_page = current_page.clone();
        Ok(match current_page {
            Some(current_page) => RunDirective::Continue { current_page },
            None => RunDirective::AwaitPage,
        })
    }

    pub fn operation_needs_recovery(
        &mut self,
        trigger: RunRecoveryTrigger,
    ) -> Result<RunDirective, RunDecisionError> {
        self.require_executing(&trigger.operation_id)?;
        self.executing_operation = None;
        self.current_page = trigger.after_page.clone();
        if self.completed_task_retries >= self.config.max_task_retries {
            return Ok(self.finish(RunTerminal::PausedNeedsHuman {
                pause: RunPause {
                    reason: RunPauseReason::RecoveryExhausted,
                    operation_id: Some(trigger.operation_id),
                    failure_reason: Some(trigger.reason),
                    completed_task_retries: self.completed_task_retries,
                    max_task_retries: self.config.max_task_retries,
                    completed_steps: self.completed_steps,
                    max_steps: self.config.max_steps,
                },
            }));
        }
        Ok(self.finish(RunTerminal::SuccessorSuggested {
            suggestion: RunSuccessorSuggestion {
                kind: RunSuccessorKind::ReturnHomeRecovery,
                task_id: trigger.recovery_task_id,
                source_operation_id: trigger.operation_id,
                failure_reason: trigger.reason,
                after_page: trigger.after_page,
                requested_task_retry: self.completed_task_retries + 1,
                max_task_retries: self.config.max_task_retries,
            },
        }))
    }

    pub fn terminal(&self) -> Option<&RunTerminal> {
        self.terminal.as_ref()
    }

    pub const fn completed_steps(&self) -> u32 {
        self.completed_steps
    }

    fn require_executing(&self, operation_id: &str) -> Result<(), RunDecisionError> {
        match self.executing_operation.as_deref() {
            Some(executing) if executing == operation_id => Ok(()),
            Some(executing) => Err(RunDecisionError::invalid_transition(format!(
                "operation outcome for '{operation_id}' does not match executing operation '{executing}'"
            ))),
            None => Err(RunDecisionError::invalid_transition(format!(
                "operation '{operation_id}' has no pending execution"
            ))),
        }
    }

    fn finish(&mut self, terminal: RunTerminal) -> RunDirective {
        self.terminal = Some(terminal.clone());
        RunDirective::Terminal(terminal)
    }
}

pub fn select_run_operation<'a>(
    game: &str,
    current_page: &str,
    operations: &'a [RunOperationCandidate],
) -> Option<&'a RunOperationCandidate> {
    operations
        .iter()
        .find(|operation| {
            operation.from_page != "any"
                && page_anchor_matches(game, current_page, &operation.from_page)
        })
        .or_else(|| {
            operations
                .iter()
                .find(|operation| page_anchor_matches(game, current_page, &operation.from_page))
        })
}

pub fn canonical_page_anchor(game: &str, page_id: &str) -> String {
    let prefix = format!("{game}/");
    page_id.strip_prefix(&prefix).unwrap_or(page_id).to_string()
}

pub fn page_anchor_matches(game: &str, observed_or_anchor: &str, expected_anchor: &str) -> bool {
    expected_anchor == "any"
        || canonical_page_anchor(game, observed_or_anchor)
            == canonical_page_anchor(game, expected_anchor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specific_operation_wins_over_any_fallback() {
        let operations = vec![candidate("fallback", "any"), candidate("specific", "home")];

        let selected =
            select_run_operation("fixture01", "fixture01/home", &operations).expect("operation");

        assert_eq!(selected.id(), "specific");
    }

    #[test]
    fn qualified_and_unqualified_page_anchors_match_symmetrically() {
        assert!(page_anchor_matches("fixture01", "fixture01/home", "home"));
        assert!(page_anchor_matches("fixture01", "home", "fixture01/home"));
        assert!(page_anchor_matches(
            "fixture01",
            "fixture01/home",
            "fixture01/home"
        ));
        assert!(!page_anchor_matches(
            "fixture01",
            "fixture01/home",
            "fixture01/other"
        ));
    }

    #[test]
    fn post_execution_failure_retries_only_within_bound() {
        let policy = policy(true, 3, Some("return_home"));
        let retry = decide_run_operation_failure(
            &policy,
            failure(
                1,
                RunFailureStage::PostExecution {
                    hit_error_page: false,
                },
            ),
        )
        .expect("decision");
        let recovery = decide_run_operation_failure(
            &policy,
            failure(
                3,
                RunFailureStage::PostExecution {
                    hit_error_page: false,
                },
            ),
        )
        .expect("decision");

        assert_eq!(
            retry,
            RunOperationFailureDecision::Retry {
                next_attempt: 2,
                delay_ms: 100
            }
        );
        assert!(matches!(
            recovery,
            RunOperationFailureDecision::RequestRecovery(_)
        ));
    }

    #[test]
    fn error_page_skips_local_retry() {
        let decision = decide_run_operation_failure(
            &policy(true, 3, Some("return_home")),
            failure(
                1,
                RunFailureStage::PostExecution {
                    hit_error_page: true,
                },
            ),
        )
        .expect("decision");

        assert!(matches!(
            decision,
            RunOperationFailureDecision::RequestRecovery(_)
        ));
    }

    #[test]
    fn non_retryable_side_effect_fails_without_recovery() {
        let decision = decide_run_operation_failure(
            &policy(false, 3, Some("return_home")),
            failure(
                1,
                RunFailureStage::PostExecution {
                    hit_error_page: true,
                },
            ),
        )
        .expect("decision");

        assert!(matches!(decision, RunOperationFailureDecision::Fail(_)));
    }

    #[test]
    fn guard_failure_requests_recovery_only_after_real_attempt() {
        let policy = policy(true, 3, Some("return_home"));
        let first =
            decide_run_operation_failure(&policy, failure(1, RunFailureStage::PreExecutionGuard))
                .expect("decision");
        let second =
            decide_run_operation_failure(&policy, failure(2, RunFailureStage::PreExecutionGuard))
                .expect("decision");

        assert!(matches!(first, RunOperationFailureDecision::Fail(_)));
        let RunOperationFailureDecision::RequestRecovery(trigger) = second else {
            panic!("expected recovery request");
        };
        assert_eq!(trigger.attempts, 1);
    }

    #[test]
    fn target_confirmation_completes_without_operation() {
        let mut machine = machine(1, 4, Some("home"));
        machine
            .observe_page(Some("fixture01/home".to_string()))
            .expect("page");

        let directive = machine.next_directive(&[]).expect("directive");

        assert!(matches!(
            directive,
            RunDirective::Terminal(RunTerminal::Completed { .. })
        ));
    }

    #[test]
    fn recovery_returns_idempotent_successor_suggestion() {
        let mut machine = machine(1, 4, Some("terminal"));
        let operations = [candidate("open_terminal", "home")];
        machine
            .observe_page(Some("home".to_string()))
            .expect("page");
        machine.next_directive(&operations).expect("execute");
        let trigger = recovery_trigger();

        let directive = machine
            .operation_needs_recovery(trigger)
            .expect("successor");
        let repeated = machine.next_directive(&operations).expect("terminal");

        let RunDirective::Terminal(RunTerminal::SuccessorSuggested { suggestion }) = directive
        else {
            panic!("expected successor suggestion");
        };
        assert_eq!(suggestion.kind, RunSuccessorKind::ReturnHomeRecovery);
        assert_eq!(suggestion.task_id, "return_home");
        assert_eq!(suggestion.requested_task_retry, 1);
        assert_eq!(
            repeated,
            RunDirective::Terminal(RunTerminal::SuccessorSuggested { suggestion })
        );
    }

    #[test]
    fn exhausted_recovery_pauses_for_human() {
        let config = RunStateConfig::new("fixture01", None, true, 1, 4).expect("config");
        let mut machine = RunStateMachine::new(config, 1).expect("machine");
        let operations = [candidate("open_terminal", "home")];
        machine
            .observe_page(Some("home".to_string()))
            .expect("page");
        machine.next_directive(&operations).expect("execute");

        let directive = machine
            .operation_needs_recovery(recovery_trigger())
            .expect("pause");

        assert!(matches!(
            directive,
            RunDirective::Terminal(RunTerminal::PausedNeedsHuman {
                pause: RunPause {
                    reason: RunPauseReason::RecoveryExhausted,
                    ..
                }
            })
        ));
    }

    #[test]
    fn max_steps_is_visible_terminal_not_fake_success() {
        let mut machine = machine(1, 1, Some("terminal"));
        let operations = [candidate("stay_home", "home")];
        machine
            .observe_page(Some("home".to_string()))
            .expect("page");
        machine.next_directive(&operations).expect("execute");
        machine
            .operation_succeeded("stay_home", Some("home".to_string()))
            .expect("success");

        let directive = machine.next_directive(&operations).expect("terminal");

        assert!(matches!(
            directive,
            RunDirective::Terminal(RunTerminal::PausedNeedsHuman {
                pause: RunPause {
                    reason: RunPauseReason::MaxStepsExhausted,
                    ..
                }
            })
        ));
    }

    #[test]
    fn mismatched_operation_outcome_is_rejected() {
        let mut machine = machine(1, 4, None);
        let operations = [candidate("open_terminal", "home")];
        machine
            .observe_page(Some("home".to_string()))
            .expect("page");
        machine.next_directive(&operations).expect("execute");

        let error = machine
            .operation_succeeded("another_operation", Some("terminal".to_string()))
            .expect_err("mismatch");

        assert_eq!(error.code(), "run_transition_invalid");
    }

    fn candidate(id: &str, from_page: &str) -> RunOperationCandidate {
        RunOperationCandidate::new(id, from_page).expect("candidate")
    }

    fn policy(
        retryable: bool,
        max_attempts: u32,
        recovery_task_id: Option<&str>,
    ) -> RunOperationPolicy {
        RunOperationPolicy::new(
            retryable,
            max_attempts,
            100,
            recovery_task_id.map(str::to_string),
        )
        .expect("policy")
    }

    fn failure(attempt: u32, stage: RunFailureStage) -> RunFailureObservation {
        RunFailureObservation::new(
            "open_terminal",
            attempt,
            "page_confirmation_failed",
            Some("home".to_string()),
            stage,
        )
        .expect("failure")
    }

    fn machine(
        max_task_retries: u32,
        max_steps: u32,
        target_page: Option<&str>,
    ) -> RunStateMachine {
        let config = RunStateConfig::new(
            "fixture01",
            target_page.map(str::to_string),
            true,
            max_task_retries,
            max_steps,
        )
        .expect("config");
        RunStateMachine::new(config, 0).expect("machine")
    }

    fn recovery_trigger() -> RunRecoveryTrigger {
        RunRecoveryTrigger {
            operation_id: "open_terminal".to_string(),
            reason: "page_confirmation_failed".to_string(),
            after_page: Some("home".to_string()),
            attempts: 3,
            recovery_task_id: "return_home".to_string(),
        }
    }
}
