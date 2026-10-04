// SPDX-License-Identifier: AGPL-3.0-only

// Stuck-recovery ladder facts (Runtime slice #316-B4): the rung vocabulary of the four
// `runtime.lifecycle_observed` phases a ladder records, and the trigger rule the host and
// the payload validation share.

use super::{RuntimeLifecyclePhase, SanitizationError, validate_diagnostic_detail_token};
use crate::{RunId, TaskId, TerminalEvent};
use serde::{Deserialize, Serialize};

/// A contained task terminal starts a ladder when its `failure_code` is
/// `contained_task_page_unknown`, any `contained_task_recovery_*` code or any
/// `contained_task_home_recovery_*` (entry recovery / return home) code.
pub fn is_stuck_recovery_trigger(failure_code: &str) -> bool {
    failure_code == "contained_task_page_unknown"
        || failure_code.starts_with("contained_task_recovery_")
        || failure_code.starts_with("contained_task_home_recovery_")
}

/// Ordinary preparation failures eligible only after their resource disposal is confirmed.
pub fn is_preparation_recovery_trigger(failure_code: &str) -> bool {
    matches!(
        failure_code,
        "input_backend_open_failed"
            | "capture_backend_open_failed"
            | "paired_backend_open_failed"
            | "capture_backend_operation_failed"
    )
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryTriggerStage {
    #[default]
    Task,
    StartupPreparation,
    ConnectionPreparation,
    RecoveryPreparation,
}

/// The rungs of the ladder; `LADDER` is the fixed default order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRung {
    ReturnHome,
    ApplicationRestart,
    EmulatorRestart,
}

impl RecoveryRung {
    pub const LADDER: [Self; 3] = [
        Self::ReturnHome,
        Self::ApplicationRestart,
        Self::EmulatorRestart,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRungState {
    Pending,
    Skipped,
}

/// Why a rung is skipped: its prerequisite, or one a later rung cannot complete without,
/// does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRungSkipReason {
    NoRecoveryPackage,
    NoStartupPackage,
    NoEmulatorControl,
    CaptureUnavailable,
    InputUnavailable,
    AdbUnavailable,
}

impl RecoveryRungSkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoRecoveryPackage => "no_recovery_package",
            Self::NoStartupPackage => "no_startup_package",
            Self::NoEmulatorControl => "no_emulator_control",
            Self::CaptureUnavailable => "capture_unavailable",
            Self::InputUnavailable => "input_unavailable",
            Self::AdbUnavailable => "adb_unavailable",
        }
    }
}

/// One rung of `recovery_ladder_started`: `reason` is present exactly when it is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRungPlan {
    pub rung: RecoveryRung,
    pub state: RecoveryRungState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RecoveryRungSkipReason>,
}

/// A task terminal or the safely completed preparation attempt that started the ladder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryLadderTrigger {
    #[serde(default)]
    pub stage: RecoveryTriggerStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<TerminalEvent>,
    pub failure_code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRungOutcome {
    Recovered,
    EnvironmentReady,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryLadderOutcome {
    Recovered,
    EnvironmentReady,
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryLadderSuppression {
    Cooldown,
    AlreadyRunning,
    PreparationSuperseded,
    AdmissionDenied,
}

/// Shape rules of the four ladder phases; every other phase passes unchecked here.
pub(super) fn validate_recovery_ladder_phase(
    phase: &RuntimeLifecyclePhase,
) -> Result<(), SanitizationError> {
    let invalid = || SanitizationError::new("invalid_recovery_ladder_phase", "runtime_payload");
    match phase {
        RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, rungs } => {
            validate_diagnostic_detail_token(&trigger.failure_code, "recovery_ladder_trigger")?;
            let valid_trigger = match trigger.stage {
                RecoveryTriggerStage::Task => {
                    trigger.run_id.is_some()
                        && trigger.task_id.is_some()
                        && trigger.preparation.is_none()
                        && is_stuck_recovery_trigger(&trigger.failure_code)
                }
                RecoveryTriggerStage::StartupPreparation
                | RecoveryTriggerStage::ConnectionPreparation => {
                    trigger.run_id.is_none()
                        && trigger.task_id.is_none()
                        && trigger.preparation.is_some_and(|event| event.sequence > 0)
                        && is_preparation_recovery_trigger(&trigger.failure_code)
                }
                RecoveryTriggerStage::RecoveryPreparation => false,
            };
            if !valid_trigger
                || rungs.len() != RecoveryRung::LADDER.len()
                || rungs.iter().zip(RecoveryRung::LADDER).any(|(plan, rung)| {
                    plan.rung != rung
                        || plan.reason.is_some() != (plan.state == RecoveryRungState::Skipped)
                })
            {
                return Err(invalid());
            }
        }
        RuntimeLifecyclePhase::RecoveryRungFinished {
            outcome,
            rung,
            run_id,
            environment,
            reason,
            ..
        } => {
            if let Some(reason) = reason {
                validate_diagnostic_detail_token(reason, "recovery_rung_reason")?;
            }
            let consistent = match outcome {
                RecoveryRungOutcome::Recovered => run_id.is_some() && reason.is_none(),
                RecoveryRungOutcome::EnvironmentReady => {
                    *rung == RecoveryRung::EmulatorRestart
                        && run_id.is_none()
                        && reason.is_none()
                        && environment.is_some()
                }
                RecoveryRungOutcome::Failed => reason.is_some(),
                RecoveryRungOutcome::Skipped => run_id.is_none() && reason.is_some(),
            };
            if !consistent
                || environment.is_some_and(|event| event.sequence == 0)
                || (environment.is_some() && *rung != RecoveryRung::EmulatorRestart)
            {
                return Err(invalid());
            }
        }
        RuntimeLifecyclePhase::RecoveryLadderFinished {
            outcome,
            rungs_tried,
        } if usize::from(*rungs_tried) > RecoveryRung::LADDER.len()
            || (*outcome != RecoveryLadderOutcome::Exhausted && *rungs_tried == 0) =>
        {
            return Err(invalid());
        }
        RuntimeLifecyclePhase::InstancePreparationFinished {
            stage,
            failure_code,
            ..
        } => {
            if *stage == RecoveryTriggerStage::Task {
                return Err(invalid());
            }
            if let Some(code) = failure_code {
                validate_diagnostic_detail_token(code, "preparation_failure_code")?;
            }
        }
        RuntimeLifecyclePhase::RecoveryEnvironmentReady {
            stop,
            start,
            preparation,
        } => {
            if stop.sequence == 0
                || start.sequence <= stop.sequence
                || preparation.sequence <= start.sequence
            {
                return Err(invalid());
            }
        }
        RuntimeLifecyclePhase::RecoveryInstanceStopped { stop } if stop.sequence == 0 => {
            return Err(invalid());
        }
        _ => {}
    }
    Ok(())
}
