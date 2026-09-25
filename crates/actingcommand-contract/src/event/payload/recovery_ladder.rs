// SPDX-License-Identifier: AGPL-3.0-only

// Stuck-recovery ladder facts (Runtime slice #316-B4): the rung vocabulary of the four
// `runtime.lifecycle_observed` phases a ladder records, and the trigger rule the host and
// the payload validation share.

use super::{RuntimeLifecyclePhase, SanitizationError, validate_diagnostic_detail_token};
use crate::{RunId, TaskId};
use serde::{Deserialize, Serialize};

/// A contained task terminal starts a ladder when its `failure_code` is
/// `contained_task_page_unknown`, any `contained_task_recovery_*` code or any
/// `contained_task_home_recovery_*` (entry recovery / return home) code.
pub fn is_stuck_recovery_trigger(failure_code: &str) -> bool {
    failure_code == "contained_task_page_unknown"
        || failure_code.starts_with("contained_task_recovery_")
        || failure_code.starts_with("contained_task_home_recovery_")
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
}

impl RecoveryRungSkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoRecoveryPackage => "no_recovery_package",
            Self::NoStartupPackage => "no_startup_package",
            Self::NoEmulatorControl => "no_emulator_control",
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

/// The `task.failed` terminal that started the ladder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryLadderTrigger {
    pub run_id: RunId,
    pub task_id: TaskId,
    pub failure_code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRungOutcome {
    Recovered,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryLadderOutcome {
    Recovered,
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryLadderSuppression {
    Cooldown,
    AlreadyRunning,
}

/// Shape rules of the four ladder phases; every other phase passes unchecked here.
pub(super) fn validate_recovery_ladder_phase(
    phase: &RuntimeLifecyclePhase,
) -> Result<(), SanitizationError> {
    let invalid = || SanitizationError::new("invalid_recovery_ladder_phase", "runtime_payload");
    match phase {
        RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, rungs } => {
            validate_diagnostic_detail_token(&trigger.failure_code, "recovery_ladder_trigger")?;
            if !is_stuck_recovery_trigger(&trigger.failure_code)
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
            run_id,
            reason,
            ..
        } => {
            if let Some(reason) = reason {
                validate_diagnostic_detail_token(reason, "recovery_rung_reason")?;
            }
            let consistent = match outcome {
                RecoveryRungOutcome::Recovered => run_id.is_some() && reason.is_none(),
                RecoveryRungOutcome::Failed => reason.is_some(),
                RecoveryRungOutcome::Skipped => run_id.is_none() && reason.is_some(),
            };
            if !consistent {
                return Err(invalid());
            }
        }
        RuntimeLifecyclePhase::RecoveryLadderFinished {
            outcome,
            rungs_tried,
        } if usize::from(*rungs_tried) > RecoveryRung::LADDER.len()
            || (*outcome == RecoveryLadderOutcome::Recovered && *rungs_tried == 0) =>
        {
            return Err(invalid());
        }
        _ => {}
    }
    Ok(())
}
