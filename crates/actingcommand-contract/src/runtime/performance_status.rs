// SPDX-License-Identifier: AGPL-3.0-only

//! `RuntimeOperation::PerformanceStatus` (Workflow #385 P-m, frozen model PERF v1.4.2 §4.6;
//! register §1.61 W14): what the shadow measurement holds now, one host row and one row per
//! registered instance. Both views are read from the same input: the normal view is the curve in
//! effect, the learning view caps it (§4.5.2). The read is answered from memory; it reads and
//! records nothing in the ledger, and it changes no dispatch (`mode` `observe` acts on nothing).

use super::{RuntimeContractError, RuntimeContractResult};
use crate::codes::RuntimeContractCode;
use crate::outcome::{Category, Outcome};
use crate::{
    MAX_RESPONSIVENESS_BP, MIN_PACING_FACTOR_MILLI, PerformanceMonitorHealth,
    PerformancePacingMode, PerformancePacingPhase, PerformancePacingStage,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most instance rows one status answers.
pub const MAX_PERFORMANCE_STATUS_INSTANCES: usize = 256;
/// The most warnings one status carries.
pub const MAX_PERFORMANCE_STATUS_WARNINGS: usize = 64;
/// A full dispatch share, in thousandths.
const FULL_SHARE_MILLI: u16 = 1_000;

/// The time a view spent in each stage, in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceStageDwell {
    pub s0_ms: u64,
    pub s1_ms: u64,
    pub s2_ms: u64,
    pub s3_ms: u64,
    pub s4_ms: u64,
}

impl PerformanceStageDwell {
    /// Adds `elapsed_ms` to `stage`'s time.
    pub fn add(&mut self, stage: PerformancePacingStage, elapsed_ms: u64) {
        let slot = match stage {
            PerformancePacingStage::S0 => &mut self.s0_ms,
            PerformancePacingStage::S1 => &mut self.s1_ms,
            PerformancePacingStage::S2 => &mut self.s2_ms,
            PerformancePacingStage::S3 => &mut self.s3_ms,
            PerformancePacingStage::S4 => &mut self.s4_ms,
        };
        *slot = slot.saturating_add(elapsed_ms);
    }

    /// The sum of this and `other`, stage by stage.
    pub fn plus(self, other: Self) -> Self {
        Self {
            s0_ms: self.s0_ms.saturating_add(other.s0_ms),
            s1_ms: self.s1_ms.saturating_add(other.s1_ms),
            s2_ms: self.s2_ms.saturating_add(other.s2_ms),
            s3_ms: self.s3_ms.saturating_add(other.s3_ms),
            s4_ms: self.s4_ms.saturating_add(other.s4_ms),
        }
    }
}

/// One view of a row (PERF §4.6): the stage and the dispatch share it gives now, the time it
/// spent in each stage over the last 24 h and since the Runtime started, and how often it would
/// have stopped work: for the host, its `allowed` falling to 0; for an instance, its share
/// falling to 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceViewStatus {
    pub pacing_stage: PerformancePacingStage,
    pub share_milli: u16,
    pub dwell_24h: PerformanceStageDwell,
    pub dwell_total: PerformanceStageDwell,
    pub stops_24h: u32,
    pub stops_total: u32,
}

impl PerformanceViewStatus {
    fn valid(&self) -> bool {
        self.share_milli <= FULL_SHARE_MILLI
            && self.stops_24h <= self.stops_total
            && (self.share_milli == 0) == (self.pacing_stage == PerformancePacingStage::S4)
    }
}

/// The host row (PERF §4.6): the host's own responsiveness (`R_host`, absent while no host signal
/// is known) and the tracked value the curve reads (`responsiveness_bp`), the curve's stage and
/// factor there, both views with the number of instances each would let take a new run
/// (`allowed`, §6.1), and the stage and factor that take effect (`mode` `observe`: S0 and 1000).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePerformanceHostStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_responsiveness_bp: Option<u16>,
    pub responsiveness_bp: u16,
    pub pacing_stage: PerformancePacingStage,
    pub factor_milli: u32,
    pub learning_view: PerformanceViewStatus,
    pub learning_view_allowed: u32,
    pub normal_view: PerformanceViewStatus,
    pub normal_view_allowed: u32,
    pub effective_stage: PerformancePacingStage,
    pub effective_factor_milli: u32,
}

/// One instance's row (PERF §4.6): its emulator responsiveness (`R_emu(i)`, absent while unknown)
/// and the tracked value of `R(i) = min(R_host, R_emu(i))`, the curve's stage and factor there,
/// both views with whether the instance is among those each view lets take a new run, and the
/// stage and factor that take effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePerformanceInstanceStatus {
    pub instance_alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulator_responsiveness_bp: Option<u16>,
    pub responsiveness_bp: u16,
    pub pacing_stage: PerformancePacingStage,
    pub factor_milli: u32,
    pub learning_view: PerformanceViewStatus,
    pub learning_view_allowed: bool,
    pub normal_view: PerformanceViewStatus,
    pub normal_view_allowed: bool,
    pub effective_stage: PerformancePacingStage,
    pub effective_factor_milli: u32,
}

/// The answer of `PerformanceStatus`: the mode in effect and, for `full`, the phase; whether the
/// learning view caps the normal one; the monitor's health; the host row; one row per registered
/// instance; and the measurement's warnings now (`perf_probe_unavailable` for a host probe that
/// could not run, `perf_baseline_clamped` for a baseline held at its bound).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePerformanceStatus {
    pub observed_at_unix_ms: u64,
    pub pacing_mode: PerformancePacingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pacing_phase: Option<PerformancePacingPhase>,
    pub learning: bool,
    pub monitor_health: PerformanceMonitorHealth,
    pub host: RuntimePerformanceHostStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instances: Vec<RuntimePerformanceInstanceStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Outcome>,
}

impl RuntimePerformanceStatus {
    /// The shape rules: responsiveness within 10 000 basis points, shares within 1 000, factors
    /// never below 1 000, a share of 0 exactly at S4, a phase only under `full`, each instance
    /// once by a valid alias, every warning a warning, and the bounds above.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let invalid = || RuntimeContractError::new(RuntimeContractCode::InvalidPerformanceStatus);
        let host = &self.host;
        let responsiveness_valid =
            |value: Option<u16>| value.is_none_or(|value| value <= MAX_RESPONSIVENESS_BP);
        if self.observed_at_unix_ms == 0
            || self.pacing_phase.is_some() && self.pacing_mode != PerformancePacingMode::Full
            || !responsiveness_valid(host.host_responsiveness_bp)
            || !responsiveness_valid(Some(host.responsiveness_bp))
            || host.factor_milli < MIN_PACING_FACTOR_MILLI
            || host.effective_factor_milli < MIN_PACING_FACTOR_MILLI
            || !host.learning_view.valid()
            || !host.normal_view.valid()
            || self.instances.len() > MAX_PERFORMANCE_STATUS_INSTANCES
            || self.warnings.len() > MAX_PERFORMANCE_STATUS_WARNINGS
            || self
                .warnings
                .iter()
                .any(|warning| warning.category() != Category::Warning)
        {
            return Err(invalid());
        }
        let allowed_bound = u32::try_from(self.instances.len()).map_err(|_| invalid())?;
        if host.learning_view_allowed > allowed_bound || host.normal_view_allowed > allowed_bound {
            return Err(invalid());
        }
        let mut aliases = BTreeSet::new();
        for row in &self.instances {
            if crate::validate_instance_alias(&row.instance_alias).is_err()
                || !aliases.insert(row.instance_alias.as_str())
                || !responsiveness_valid(row.emulator_responsiveness_bp)
                || !responsiveness_valid(Some(row.responsiveness_bp))
                || row.factor_milli < MIN_PACING_FACTOR_MILLI
                || row.effective_factor_milli < MIN_PACING_FACTOR_MILLI
                || !row.learning_view.valid()
                || !row.normal_view.valid()
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
