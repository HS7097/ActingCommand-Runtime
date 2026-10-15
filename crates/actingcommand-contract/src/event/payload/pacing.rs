// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 SH-1, the v0.12.0 shape slice (register `contracts/v012-register.md` §1.4,
// performance pacing, Workflow #385 PERF §12.2): the new performance records and the pacing
// members of `perf.summary` and of the task timing evidence. Types, readers and validation only;
// P-m, P-s, P-k and P-h write them.

use super::PayloadDetail;
use crate::codes::SanitizationCode;
use crate::outcome::{Category, Outcome};
use crate::{DiagnosticCode, EffectDisposition, EventAction, SanitizationError, SanitizedAudit};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The largest responsiveness, in basis points.
pub const MAX_RESPONSIVENESS_BP: u16 = 10_000;
/// The largest dispatch share, in thousandths.
const MAX_SHARE_MILLI: u16 = 1_000;
/// The pacing factor is never below 1 (PERF §5: pacing only lengthens).
pub const MIN_PACING_FACTOR_MILLI: u32 = 1_000;
/// The most instance rows one `perf.summary` carries.
pub const MAX_PERFORMANCE_PACING_INSTANCES: usize = 64;

/// A pacing stage (vocabulary `pacing_stage`): `s0` is normal pacing, each later stage a worse
/// responsiveness band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePacingStage {
    S0,
    S1,
    S2,
    S3,
    S4,
}

impl PerformancePacingStage {
    /// The stage's spelling (vocabulary `pacing_stage`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S0 => "s0",
            Self::S1 => "s1",
            Self::S2 => "s2",
            Self::S3 => "s3",
            Self::S4 => "s4",
        }
    }
}

/// Workflow #385 P-m: a stage as an outcome value (`pacing_stage_from`, `pacing_stage_to`).
impl crate::outcome::VocabularyToken for PerformancePacingStage {
    const VOCABULARY: &'static str = "pacing_stage";

    fn token(self) -> &'static str {
        self.as_str()
    }
}

/// How far pacing takes effect (PERF §10.3 `performance.pacing.mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePacingMode {
    Observe,
    Pace,
    Full,
}

impl PerformancePacingMode {
    /// The mode's spelling (vocabulary `pacing_mode`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Pace => "pace",
            Self::Full => "full",
        }
    }
}

/// Workflow #385 P-m: a mode as an outcome value (`pacing_mode`).
impl crate::outcome::VocabularyToken for PerformancePacingMode {
    const VOCABULARY: &'static str = "pacing_mode";

    fn token(self) -> &'static str {
        self.as_str()
    }
}

/// The phase of `mode = full` (PERF §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePacingPhase {
    Learning,
    Normal,
}

/// The pressure level of a step's state (PERF §4.5.1): the highest severity among the pressure
/// floor kinds, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePressureLevel {
    None,
    Elevated,
    High,
    Critical,
}

/// The pacing view of one instance in `perf.summary` (register P4): the stage, share and
/// whether the instance is among those allowed, by the learning view and by the normal view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceInstancePacingViews {
    pub instance_id: String,
    pub learning_view_stage: PerformancePacingStage,
    pub learning_view_share_milli: u16,
    pub learning_view_allowed: bool,
    pub normal_view_stage: PerformancePacingStage,
    pub normal_view_share_milli: u16,
    pub normal_view_allowed: bool,
}

/// The pacing members of `perf.summary` (register P4). The host's `*_view_allowed` is the
/// number of instances that may take a new run (`allowed`, PERF §6.1); `pacing_phase` is present
/// only when `pacing_mode` is `full`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformancePacingSummary {
    pub responsiveness_bp: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_responsiveness_bp: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulator_responsiveness_bp: Option<u16>,
    pub pacing_stage: PerformancePacingStage,
    pub factor_milli: u32,
    pub dispatch_share_milli: u16,
    pub pacing_mode: PerformancePacingMode,
    pub learning: bool,
    pub learning_view_stage: PerformancePacingStage,
    pub learning_view_share_milli: u16,
    pub learning_view_allowed: u32,
    pub normal_view_stage: PerformancePacingStage,
    pub normal_view_share_milli: u16,
    pub normal_view_allowed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pacing_phase: Option<PerformancePacingPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learn_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learn_active_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instances: Vec<PerformanceInstancePacingViews>,
}

impl PerformancePacingSummary {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        let responsiveness = [
            Some(self.responsiveness_bp),
            self.host_responsiveness_bp,
            self.emulator_responsiveness_bp,
        ];
        let shares = [
            self.dispatch_share_milli,
            self.learning_view_share_milli,
            self.normal_view_share_milli,
        ];
        if responsiveness
            .into_iter()
            .flatten()
            .any(|value| value > MAX_RESPONSIVENESS_BP)
            || shares.into_iter().any(|value| value > MAX_SHARE_MILLI)
            || self.factor_milli < MIN_PACING_FACTOR_MILLI
            || self.pacing_phase.is_some() && self.pacing_mode != PerformancePacingMode::Full
            || self.instances.len() > MAX_PERFORMANCE_PACING_INSTANCES
        {
            return Err(invalid_pacing("pacing"));
        }
        let mut instances = BTreeSet::new();
        for row in &self.instances {
            crate::validate_instance_alias(&row.instance_id)
                .map_err(|_| invalid_pacing("instances"))?;
            if !instances.insert(row.instance_id.as_str())
                || row.learning_view_share_milli > MAX_SHARE_MILLI
                || row.normal_view_share_milli > MAX_SHARE_MILLI
            {
                return Err(invalid_pacing("instances"));
            }
        }
        Ok(())
    }
}

/// `perf.pacing_changed`, `perf.dispatch_share_changed` and `perf.run_hold` (register P1-P3):
/// one transition, its code and values in `outcome` (`perf_stage_changed`,
/// `perf_scheduling_paused` / `perf_scheduling_resumed`, `perf_run_held` / `perf_run_resumed`,
/// all info).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformancePacingPayload {
    action: EventAction,
    outcome: Outcome,
    audit: SanitizedAudit,
}

impl PerformancePacingPayload {
    /// Workflow #385 P-m: an observed transition (`perf.pacing_changed`), its code and values in
    /// `outcome`; the outcome must be info.
    pub(super) fn observed(
        outcome: Outcome,
        audit: SanitizedAudit,
    ) -> Result<Self, SanitizationError> {
        let payload = Self {
            action: EventAction::PerformanceObserve,
            outcome,
            audit,
        };
        payload.validate()?;
        Ok(payload)
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::PerformanceObserve
            || self.outcome.category() != Category::Info
        {
            return Err(invalid_pacing("performance_payload"));
        }
        Ok(())
    }
}

impl PayloadDetail for PerformancePacingPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    fn outcome(&self) -> Option<&Outcome> {
        Some(&self.outcome)
    }
}

fn invalid_pacing(field: &'static str) -> SanitizationError {
    SanitizationError::new(SanitizationCode::InvalidPerformancePayload, field)
}
