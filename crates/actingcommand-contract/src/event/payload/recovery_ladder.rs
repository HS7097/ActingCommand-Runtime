// SPDX-License-Identifier: AGPL-3.0-only

// Stuck-recovery ladder facts (Runtime slice #316-B4; Workflow #387 D1c, frozen model M387
// v1.2 §6): the rung vocabulary of the `runtime.lifecycle_observed` phases a ladder records and
// their validation. Which codes start a ladder, in which order its rungs climb and how long each
// may take is the `recovery_ladder` configuration's (`crates/runtime-config`); every record
// carries what it needs to be read on its own (the trigger item that matched, how it matched,
// its rung plan and the digest of the configuration it climbed by), so validation checks only
// that a record agrees with itself (§6.2).

use super::{RuntimeLifecyclePhase, SanitizationError};
use crate::codes::SanitizationCode;
use crate::outcome::{CodeStr, LadderClass};
use crate::{RunId, TaskId, TerminalEvent};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The longest trigger item id, and ladder profile name, a ladder record names.
pub const MAX_RECOVERY_LADDER_TOKEN_BYTES: usize = 64;

/// Workflow #387 D1c: the form of a trigger item's id (`recovery_ladder.triggers[].id`, the
/// `rule` of a ladder record) and of a ladder profile's name (M398 §6.2): 1 to 64 bytes of
/// lowercase ASCII letters, digits and `_`, starting with a letter.
pub fn valid_recovery_ladder_token(text: &str) -> bool {
    let bytes = text.as_bytes();
    (1..=MAX_RECOVERY_LADDER_TOKEN_BYTES).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

/// Workflow #387 D1c: a `config_digest`, the SHA-256 of the configuration a ladder climbed by,
/// as 64 lowercase hexadecimal digits (§4.7).
pub fn valid_recovery_ladder_config_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The ladder class of the token that spells it (vocabulary `ladder_class`).
pub fn parse_ladder_class(token: &str) -> Option<LadderClass> {
    LadderClass::ALL
        .iter()
        .copied()
        .find(|class| class.as_str() == token)
}

impl Serialize for LadderClass {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LadderClass {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let token = String::deserialize(deserializer)?;
        parse_ladder_class(&token).ok_or_else(|| {
            serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(&token),
                &"a token of the vocabulary ladder_class",
            )
        })
    }
}

/// Where a trigger arose (M387 §3.1 `stages`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryTriggerStage {
    #[default]
    Task,
    StartupPreparation,
    ConnectionPreparation,
    RecoveryPreparation,
    /// Workflow #403: an unconfirmed close of the instance's own resources fenced it; the
    /// ladder may lift the fence only through a rung whose preparation proves the close.
    ResourceFence,
    /// Workflow #387 D1c (direction ①, M387 §3.4): records of one instance in a short window;
    /// only a ladder's own records name it, never a preparation.
    StabilityWarnings,
}

impl RecoveryTriggerStage {
    pub const ALL: [Self; 6] = [
        Self::Task,
        Self::StartupPreparation,
        Self::ConnectionPreparation,
        Self::RecoveryPreparation,
        Self::ResourceFence,
        Self::StabilityWarnings,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::StartupPreparation => "startup_preparation",
            Self::ConnectionPreparation => "connection_preparation",
            Self::RecoveryPreparation => "recovery_preparation",
            Self::ResourceFence => "resource_fence",
            Self::StabilityWarnings => "stability_warnings",
        }
    }
}

/// The rung actions a ladder can take (M387 §4.1): the closed set its configured `rungs` order.
/// `ALL` is the vocabulary, not an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRung {
    ReturnHome,
    ApplicationRestart,
    EmulatorRestart,
}

impl RecoveryRung {
    pub const ALL: [Self; 3] = [
        Self::ReturnHome,
        Self::ApplicationRestart,
        Self::EmulatorRestart,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReturnHome => "return_home",
            Self::ApplicationRestart => "application_restart",
            Self::EmulatorRestart => "emulator_restart",
        }
    }
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
    /// Workflow #403: the ladder is a resource fence's, and this rung cannot prove the close.
    ResourceFence,
    /// Workflow #387 D1c (M387 §4.1): the trigger item starts the climb at a later rung.
    BeforeStartRung,
    /// Workflow #398 (M398 §6.4): the device-state gate holds this rung's entry (a locked
    /// screen, heat, a low battery or a device in use). RD-5 records it.
    DeviceNotReady,
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
            Self::ResourceFence => "resource_fence",
            Self::BeforeStartRung => "before_start_rung",
            Self::DeviceNotReady => "device_not_ready",
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

/// Workflow #387 D1c (M387 §3.7): when a ladder a trigger item admits starts to climb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryTriggerStart {
    /// At once: a failed run's lease end hands its key to the ladder.
    Immediately,
    /// The ladder's claim waits in the instance's queue until performance control releases the
    /// instance; the failed run's lease end hands nothing on.
    AfterPerformanceRelease,
}

impl RecoveryTriggerStart {
    pub const ALL: [Self; 2] = [Self::Immediately, Self::AfterPerformanceRelease];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Immediately => "immediately",
            Self::AfterPerformanceRelease => "after_performance_release",
        }
    }
}

/// Workflow #387 D1c (M387 §6.1): how the trigger item `rule` matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryTriggerMatch {
    /// The failure code is one of the item's `codes` (or under one of its prefixes).
    Code,
    /// The failure code's class is one of the item's `classes`.
    Class { ladder_class: LadderClass },
    /// Direction ①: the `count`-th record of the item's classes or codes on the instance within
    /// `window_ms`, the first at `first_unix_ms`, the last at `last_unix_ms`.
    WarningBurst {
        count: u32,
        window_ms: u64,
        first_unix_ms: u64,
        last_unix_ms: u64,
    },
}

/// A task terminal, the safely completed preparation attempt, the records of a warning burst
/// or the resource fence that started the ladder; `failure_code` is the code of that terminal's
/// or attempt's outcome, as recorded, of the burst's last record, or of the fence's unconfirmed
/// failure. Workflow #387 D1c: `rule` is the trigger item that matched and `matched` how; a
/// resource fence's ladder (Workflow #403) matched no trigger item and carries neither.
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
    pub failure_code: CodeStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<RecoveryTriggerMatch>,
    pub start: RecoveryTriggerStart,
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

/// §6.2: the trigger of `recovery_ladder_started` agrees with its stage.
fn trigger_consistent(trigger: &RecoveryLadderTrigger) -> bool {
    let rule = trigger
        .rule
        .as_deref()
        .is_some_and(valid_recovery_ladder_token);
    let by_code_or_class = matches!(
        trigger.matched,
        Some(RecoveryTriggerMatch::Code | RecoveryTriggerMatch::Class { .. })
    );
    let unlinked =
        trigger.run_id.is_none() && trigger.task_id.is_none() && trigger.preparation.is_none();
    match trigger.stage {
        RecoveryTriggerStage::Task => {
            trigger.run_id.is_some()
                && trigger.task_id.is_some()
                && trigger.preparation.is_none()
                && rule
                && by_code_or_class
        }
        RecoveryTriggerStage::StartupPreparation | RecoveryTriggerStage::ConnectionPreparation => {
            trigger.run_id.is_none()
                && trigger.task_id.is_none()
                && trigger.preparation.is_some_and(|event| event.sequence > 0)
                && rule
                && by_code_or_class
        }
        RecoveryTriggerStage::StabilityWarnings => {
            unlinked
                && rule
                && matches!(
                    trigger.matched,
                    Some(RecoveryTriggerMatch::WarningBurst {
                        count,
                        window_ms,
                        first_unix_ms,
                        last_unix_ms,
                    }) if count >= 1 && window_ms > 0 && first_unix_ms <= last_unix_ms
                )
        }
        // M387 §2.2b A2: the ladder's own preparation never starts a ladder.
        RecoveryTriggerStage::RecoveryPreparation => false,
        // Workflow #403: a fence names the unconfirmed failure's code; a run or a preparation
        // it arose in is not its trigger, and no trigger item matched it.
        RecoveryTriggerStage::ResourceFence => {
            unlinked && trigger.rule.is_none() && trigger.matched.is_none()
        }
    }
}

/// §6.2: one to three rungs, no action twice, and a reason exactly on a skipped rung.
fn rungs_consistent(rungs: &[RecoveryRungPlan]) -> bool {
    (1..=RecoveryRung::ALL.len()).contains(&rungs.len())
        && rungs.iter().enumerate().all(|(index, plan)| {
            plan.reason.is_some() == (plan.state == RecoveryRungState::Skipped)
                && rungs[..index]
                    .iter()
                    .all(|earlier| earlier.rung != plan.rung)
        })
}

/// Shape rules of the ladder phases; every other phase passes unchecked here (M387 §6.2).
pub(super) fn validate_recovery_ladder_phase(
    phase: &RuntimeLifecyclePhase,
) -> Result<(), SanitizationError> {
    let invalid = || {
        SanitizationError::new(
            SanitizationCode::InvalidRecoveryLadderPhase,
            "runtime_payload",
        )
    };
    match phase {
        RuntimeLifecyclePhase::RecoveryLadderStarted {
            trigger,
            rungs,
            config_digest,
            profile,
        } => {
            if !trigger_consistent(trigger)
                || !rungs_consistent(rungs)
                || !valid_recovery_ladder_config_digest(config_digest)
                || profile
                    .as_deref()
                    .is_some_and(|profile| !valid_recovery_ladder_token(profile))
            {
                return Err(invalid());
            }
        }
        RuntimeLifecyclePhase::RecoveryRungFinished {
            rung_result,
            rung,
            run_id,
            environment,
            outcome,
        } => {
            let consistent = match rung_result {
                RecoveryRungOutcome::Recovered => run_id.is_some() && outcome.is_none(),
                RecoveryRungOutcome::EnvironmentReady => {
                    *rung == RecoveryRung::EmulatorRestart
                        && run_id.is_none()
                        && outcome.is_none()
                        && environment.is_some()
                }
                RecoveryRungOutcome::Failed => outcome.is_some(),
                RecoveryRungOutcome::Skipped => run_id.is_none() && outcome.is_some(),
            };
            if !consistent
                || environment.is_some_and(|event| event.sequence == 0)
                || (environment.is_some() && *rung != RecoveryRung::EmulatorRestart)
            {
                return Err(invalid());
            }
        }
        RuntimeLifecyclePhase::RecoveryLadderFinished {
            ladder_result,
            rungs_tried,
            outcome,
        } if usize::from(*rungs_tried) > RecoveryRung::ALL.len()
            || (*ladder_result != RecoveryLadderOutcome::Exhausted && *rungs_tried == 0)
            || (*ladder_result == RecoveryLadderOutcome::Exhausted) != outcome.is_some() =>
        {
            return Err(invalid());
        }
        // A preparation record names one of the preparation stages: never a task, and never a
        // warning burst (M387 §6.2, RVC L3).
        RuntimeLifecyclePhase::InstancePreparationFinished { stage, .. } => {
            if matches!(
                stage,
                RecoveryTriggerStage::Task | RecoveryTriggerStage::StabilityWarnings
            ) {
                return Err(invalid());
            }
        }
        RuntimeLifecyclePhase::RecoveryLadderSuppressed {
            rule: Some(rule), ..
        } if !valid_recovery_ladder_token(rule) => {
            return Err(invalid());
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

#[cfg(test)]
mod tests;
