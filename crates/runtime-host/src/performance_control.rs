// SPDX-License-Identifier: AGPL-3.0-only

//! Contention-driven Runtime control with bounded escalation and staged recovery.

use crate::codes::{HostCode, HostLocation};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{
    ArbitrationBasis, ArbitrationBasisKind, PerformanceControlEventData, PerformanceControlLevel,
    PerformanceControlReason, PerformanceDeadlineDisposition, RuntimeErrorCode,
};
use actingcommand_policy::{HostResourceSnapshot, LoadProfile};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const BASIS_POINTS_MAX: u16 = 10_000;
const DEFAULT_HEAVY_THRESHOLD_MILLI: u64 = 1_500;

#[derive(Debug, Clone)]
pub struct PerformanceControlConfig {
    escalation_samples: u16,
    recovery_samples: u16,
    transition_cooldown: Duration,
    clock_jump_threshold: Duration,
    normal_heavy_dispatch_limit: u16,
    pressured_heavy_dispatch_limit: u16,
    /// Workflow #385 PM-CFG (`performance.load.heavy_threshold_milli`): a weighted workload whose
    /// `cpu_milli + gpu_milli + io_milli` reaches it is heavy.
    heavy_threshold_milli: u64,
}

impl Default for PerformanceControlConfig {
    fn default() -> Self {
        Self {
            escalation_samples: 2,
            recovery_samples: 3,
            transition_cooldown: Duration::from_secs(4),
            clock_jump_threshold: Duration::from_secs(30),
            normal_heavy_dispatch_limit: 2,
            pressured_heavy_dispatch_limit: 1,
            heavy_threshold_milli: DEFAULT_HEAVY_THRESHOLD_MILLI,
        }
    }
}

impl PerformanceControlConfig {
    pub fn with_hysteresis(
        mut self,
        escalation_samples: u16,
        recovery_samples: u16,
        transition_cooldown: Duration,
    ) -> Self {
        self.escalation_samples = escalation_samples;
        self.recovery_samples = recovery_samples;
        self.transition_cooldown = transition_cooldown;
        self
    }

    pub fn with_clock_jump_threshold(mut self, clock_jump_threshold: Duration) -> Self {
        self.clock_jump_threshold = clock_jump_threshold;
        self
    }

    pub fn with_heavy_dispatch_limits(mut self, normal: u16, pressured: u16) -> Self {
        self.normal_heavy_dispatch_limit = normal;
        self.pressured_heavy_dispatch_limit = pressured;
        self
    }

    /// Workflow #385 PM-CFG (PERF §10.1): `performance.load.heavy_threshold_milli` in effect.
    pub fn with_heavy_threshold_milli(mut self, heavy_threshold_milli: u64) -> Self {
        self.heavy_threshold_milli = heavy_threshold_milli;
        self
    }

    pub const fn escalation_samples(&self) -> u16 {
        self.escalation_samples
    }

    pub const fn recovery_samples(&self) -> u16 {
        self.recovery_samples
    }

    pub const fn transition_cooldown(&self) -> Duration {
        self.transition_cooldown
    }

    pub const fn clock_jump_threshold(&self) -> Duration {
        self.clock_jump_threshold
    }

    pub const fn normal_heavy_dispatch_limit(&self) -> u16 {
        self.normal_heavy_dispatch_limit
    }

    pub const fn pressured_heavy_dispatch_limit(&self) -> u16 {
        self.pressured_heavy_dispatch_limit
    }

    /// The effective heavy threshold (`with_heavy_threshold_milli`, default 1 500).
    pub const fn heavy_threshold_milli(&self) -> u64 {
        self.heavy_threshold_milli
    }

    pub fn validate(&self) -> RuntimeHostResult<()> {
        if self.escalation_samples == 0
            || self.recovery_samples == 0
            || self.transition_cooldown.is_zero()
            || self.clock_jump_threshold < self.transition_cooldown
            || self.normal_heavy_dispatch_limit == 0
            || self.pressured_heavy_dispatch_limit == 0
            || self.pressured_heavy_dispatch_limit > self.normal_heavy_dispatch_limit
            || self.heavy_threshold_milli == 0
        {
            return Err(control_fatal(
                HostCode::PerformanceControlConfigInvalid,
                HostLocation::ValidatePerformanceControlConfig,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceControlObservation {
    pub observed_at_unix_ms: u64,
    pub host_responsiveness_basis_points: Option<u16>,
    pub third_party_pressure_basis_points: Option<u16>,
    pub foreground_fullscreen: bool,
}

impl PerformanceControlObservation {
    fn validate(&self) -> RuntimeHostResult<()> {
        if self.observed_at_unix_ms == 0
            || self
                .host_responsiveness_basis_points
                .is_some_and(|value| value > BASIS_POINTS_MAX)
            || self
                .third_party_pressure_basis_points
                .is_some_and(|value| value > BASIS_POINTS_MAX)
        {
            return Err(control_fatal(
                HostCode::PerformanceControlObservationInvalid,
                HostLocation::ObservePerformanceContention,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceControlWorkload {
    pub instance_id: String,
    pub load_profile: LoadProfile,
    /// The latest policy cycle's arbitration input for this instance; `None` without one.
    pub arbitration: Option<InstanceArbitrationRank>,
}

/// One instance's highest effective utility and longest eligibility age over its Eligible or
/// Selected decisions in the latest policy cycle that had any (Workflow #308 slice 5c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceArbitrationRank {
    pub utility_milli: i64,
    pub aging_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceControlDirective {
    pub instance_id: String,
    pub level: PerformanceControlLevel,
    pub throttle_delay_ms: u64,
    pub yield_requested: bool,
    pub qos_reduced: bool,
    pub suspend_requested: bool,
    pub shutdown_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PerformanceDispatchGate {
    Allowed,
    Deferred {
        reason: Code,
        /// Always `None`: urgency no longer changes a deferral (Workflow #385 P-0).
        deadline_disposition: Option<PerformanceDeadlineDisposition>,
        /// Always `None`: a deferral records no balance event (Workflow #385 P-0).
        event: Option<PerformanceControlEventData>,
        /// Always `None`: no level opens a throttle window (Workflow #385 P-0).
        retry_after_ms: Option<u64>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct PerformanceBalanceController {
    config: PerformanceControlConfig,
    level: PerformanceControlLevel,
    instance_levels: BTreeMap<String, PerformanceControlLevel>,
    /// The arbitration input of the latest workloads; an instance without one has no entry.
    instance_arbitration: BTreeMap<String, InstanceArbitrationRank>,
    last_observed_at_unix_ms: Option<u64>,
    cooldown_until_unix_ms: u64,
    candidate_level: Option<PerformanceControlLevel>,
    candidate_samples: u16,
    last_observation: Option<PerformanceControlObservation>,
    /// The latest performance tick without a fresh sample; memory-only.
    last_tick_unix_ms: Option<u64>,
}

impl PerformanceBalanceController {
    pub(crate) fn new(config: PerformanceControlConfig) -> RuntimeHostResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            level: PerformanceControlLevel::Normal,
            instance_levels: BTreeMap::new(),
            instance_arbitration: BTreeMap::new(),
            last_observed_at_unix_ms: None,
            cooldown_until_unix_ms: 0,
            candidate_level: None,
            candidate_samples: 0,
            last_observation: None,
            last_tick_unix_ms: None,
        })
    }

    /// A level above Normal, globally or on an instance: only then can a tick without
    /// evidence recover (performance-control.md "Recovery without evidence").
    pub(crate) fn raised(&self) -> bool {
        self.level != PerformanceControlLevel::Normal
            || self
                .instance_levels
                .values()
                .any(|level| *level != PerformanceControlLevel::Normal)
    }

    pub(crate) fn observe(
        &mut self,
        observation: PerformanceControlObservation,
        workloads: &[PerformanceControlWorkload],
    ) -> RuntimeHostResult<Vec<PerformanceControlEventData>> {
        observation.validate()?;
        if let Some(previous) = self.last_observed_at_unix_ms
            && observation.observed_at_unix_ms == previous
        {
            if self.last_observation.as_ref() == Some(&observation) {
                return Ok(Vec::new());
            }
            return Err(control_fatal(
                HostCode::PerformanceControlSampleIdentityConflict,
                HostLocation::ObservePerformanceContention,
            ));
        }
        self.sync_workloads(workloads)?;
        // A forward jump is measured against the latest input, a sample or a tick without
        // one; a backward jump only against the previous sample.
        if let Some(reference) = self.last_observed_at_unix_ms.max(self.last_tick_unix_ms) {
            let jump_threshold = duration_ms(self.config.clock_jump_threshold)?;
            if self
                .last_observed_at_unix_ms
                .is_some_and(|previous| observation.observed_at_unix_ms < previous)
                || observation.observed_at_unix_ms.saturating_sub(reference) > jump_threshold
            {
                self.candidate_level = None;
                self.candidate_samples = 0;
                self.cooldown_until_unix_ms = observation
                    .observed_at_unix_ms
                    .checked_add(duration_ms(self.config.transition_cooldown)?)
                    .ok_or_else(|| {
                        control_fatal(
                            HostCode::PerformanceControlTimeOverflow,
                            HostLocation::ObservePerformanceClockJump,
                        )
                    })?;
                self.last_observed_at_unix_ms = Some(observation.observed_at_unix_ms);
                self.last_observation = Some(observation.clone());
                return Ok(vec![self.event(
                    None,
                    self.level,
                    self.level,
                    PerformanceControlReason::ClockJump,
                    false,
                    None,
                    &observation,
                )]);
            }
        }
        self.last_observed_at_unix_ms = Some(observation.observed_at_unix_ms);
        self.last_observation = Some(observation.clone());

        let Some(target) = target_level(&observation) else {
            self.candidate_level = None;
            self.candidate_samples = 0;
            return Ok(Vec::new());
        };
        self.apply_target(target, &observation)
    }

    /// One performance tick without a fresh sample (performance-control.md "Recovery without
    /// evidence"): it advances the clock-jump reference; without owned work and with a level
    /// above Normal it is one recovery sample under the unchanged hysteresis and cooldown.
    pub(crate) fn observe_without_evidence(
        &mut self,
        now_unix_ms: u64,
        owned_work: bool,
    ) -> RuntimeHostResult<Vec<PerformanceControlEventData>> {
        if now_unix_ms == 0 {
            return Err(control_fatal(
                HostCode::PerformanceControlTickInvalid,
                HostLocation::ObservePerformanceTick,
            ));
        }
        let previous_tick = self.last_tick_unix_ms;
        if previous_tick == Some(now_unix_ms) {
            // The same tick cannot advance hysteresis twice.
            return Ok(Vec::new());
        }
        let reference = self.last_observed_at_unix_ms.max(previous_tick);
        self.last_tick_unix_ms = Some(now_unix_ms);
        let tick = PerformanceControlObservation {
            observed_at_unix_ms: now_unix_ms,
            host_responsiveness_basis_points: None,
            third_party_pressure_basis_points: None,
            foreground_fullscreen: false,
        };
        if let Some(reference) = reference {
            let jump_threshold = duration_ms(self.config.clock_jump_threshold)?;
            // Backward only tick against tick: after a clock step back, every later tick
            // would otherwise report itself against a "future" sample time again.
            if previous_tick.is_some_and(|previous| now_unix_ms < previous)
                || now_unix_ms.saturating_sub(reference) > jump_threshold
            {
                self.candidate_level = None;
                self.candidate_samples = 0;
                self.cooldown_until_unix_ms = now_unix_ms
                    .checked_add(duration_ms(self.config.transition_cooldown)?)
                    .ok_or_else(|| {
                        control_fatal(
                            HostCode::PerformanceControlTimeOverflow,
                            HostLocation::ObservePerformanceTickClockJump,
                        )
                    })?;
                return Ok(vec![self.event(
                    None,
                    self.level,
                    self.level,
                    PerformanceControlReason::ClockJump,
                    false,
                    None,
                    &tick,
                )]);
            }
        }
        if owned_work || !self.raised() {
            return Ok(Vec::new());
        }
        // No lease, no admitted dispatch, no recent pipeline sample: every workload has ended
        // (contract: an instance raised above the global level keeps its level without one).
        self.sync_workloads(&[])?;
        self.apply_target(PerformanceControlLevel::Normal, &tick)
    }

    /// The transition toward `target` under hysteresis, cooldown, one level and one instance
    /// per transition; shared by measured samples and ticks without evidence.
    fn apply_target(
        &mut self,
        target: PerformanceControlLevel,
        observation: &PerformanceControlObservation,
    ) -> RuntimeHostResult<Vec<PerformanceControlEventData>> {
        if target == self.level {
            return self.recover_instances_if_due(target, observation);
        }
        if self.candidate_level == Some(target) {
            self.candidate_samples = self.candidate_samples.saturating_add(1);
        } else {
            self.candidate_level = Some(target);
            self.candidate_samples = 1;
        }
        let required = if target.rank() > self.level.rank() {
            self.config.escalation_samples
        } else {
            self.config.recovery_samples
        };
        if self.candidate_samples < required
            || observation.observed_at_unix_ms < self.cooldown_until_unix_ms
        {
            return Ok(Vec::new());
        }

        self.candidate_level = None;
        self.candidate_samples = 0;
        self.cooldown_until_unix_ms = observation
            .observed_at_unix_ms
            .checked_add(duration_ms(self.config.transition_cooldown)?)
            .ok_or_else(|| {
                control_fatal(
                    HostCode::PerformanceControlTimeOverflow,
                    HostLocation::TransitionPerformanceControl,
                )
            })?;
        let previous = self.level;
        if target.rank() < previous.rank() {
            self.level = previous_level(previous);
            let mut events = vec![self.event(
                None,
                previous,
                self.level,
                PerformanceControlReason::Recovery,
                true,
                None,
                observation,
            )];
            if let Some(event) = self.recover_one_instance(observation)? {
                events.push(event);
            }
            return Ok(events);
        }
        let step = next_level(previous);
        let reason = control_reason(observation);
        let mut events = Vec::new();
        if step.rank() >= PerformanceControlLevel::Suspended.rank() {
            // Suspension and shutdown reach one instance per transition; the global level
            // follows only once every instance has been raised on its own.
            if let Some(event) = self.suspend_one_instance(step, reason, observation)? {
                events.push(event);
            }
            if self
                .instance_levels
                .values()
                .any(|level| level.rank() < step.rank())
            {
                return Ok(events);
            }
        } else {
            for level in self.instance_levels.values_mut() {
                if level.rank() < step.rank() {
                    *level = step;
                }
            }
        }
        self.level = step;
        events.push(self.event(None, previous, step, reason, false, None, observation));
        Ok(events)
    }

    pub(crate) fn apply_to_resources(
        &self,
        resources: &mut [HostResourceSnapshot],
        workloads: &[PerformanceControlWorkload],
    ) -> RuntimeHostResult<()> {
        let heavy_count = u16::try_from(
            workloads
                .iter()
                .filter(|workload| {
                    is_heavy(&workload.load_profile, self.config.heavy_threshold_milli)
                })
                .count(),
        )
        .map_err(|_| {
            control_fatal(
                HostCode::PerformanceHeavyCountOverflow,
                HostLocation::ApplyPerformanceControlResources,
            )
        })?;
        for host in resources {
            if let Some(observation) = &self.last_observation {
                if let Some(value) = observation.host_responsiveness_basis_points {
                    host.host_responsiveness_basis_points = value;
                }
                if let Some(value) = observation.third_party_pressure_basis_points {
                    host.third_party_pressure_basis_points = value;
                }
            }
            // The host budgets only express capacity: no level scales them (Workflow #385 P-0).
            host.heavy_dispatch_limit = if self.level == PerformanceControlLevel::Normal {
                self.config.normal_heavy_dispatch_limit
            } else {
                self.config.pressured_heavy_dispatch_limit
            };
            host.active_heavy_dispatches = heavy_count.min(host.heavy_dispatch_limit);
        }
        Ok(())
    }

    /// Gates one policy dispatch on the instance's own directive (its level falls back to the
    /// global level when it has no entry). Normal through QosReduced allow, with no throttle
    /// window and no wait; Suspended and ShutdownRequested defer with
    /// `performance_contention_dispatch_deferred` at any urgency (Workflow #385 P-0). The
    /// urgency is validated only: it orders candidates in the evaluator, never this gate.
    pub(crate) fn gate_dispatch(
        &self,
        instance_id: &str,
        urgency_milli: u16,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<PerformanceDispatchGate> {
        if urgency_milli > 1_000 || instance_id.is_empty() || observed_at_unix_ms == 0 {
            return Err(control_fatal(
                HostCode::PerformanceDispatchGateInvalid,
                HostLocation::GatePerformanceDispatch,
            ));
        }
        Ok(match self.directive(instance_id)?.level {
            PerformanceControlLevel::Normal
            | PerformanceControlLevel::DispatchPaused
            | PerformanceControlLevel::Throttled
            | PerformanceControlLevel::YieldRequested
            | PerformanceControlLevel::QosReduced => PerformanceDispatchGate::Allowed,
            PerformanceControlLevel::Suspended | PerformanceControlLevel::ShutdownRequested => {
                PerformanceDispatchGate::Deferred {
                    reason: HostCode::PerformanceContentionDispatchDeferred.code(),
                    deadline_disposition: None,
                    event: None,
                    retry_after_ms: None,
                }
            }
        })
    }

    pub(crate) fn directive(
        &self,
        instance_id: &str,
    ) -> RuntimeHostResult<PerformanceControlDirective> {
        if instance_id.is_empty() {
            return Err(control_fatal(
                HostCode::PerformanceControlInstanceInvalid,
                HostLocation::ReadPerformanceControlDirective,
            ));
        }
        let level = self
            .instance_levels
            .get(instance_id)
            .copied()
            .unwrap_or(self.level)
            .max(self.level);
        let rank = u64::from(level.rank());
        Ok(PerformanceControlDirective {
            instance_id: instance_id.to_owned(),
            level,
            throttle_delay_ms: rank.saturating_sub(1).saturating_mul(50),
            yield_requested: level.rank() >= PerformanceControlLevel::YieldRequested.rank(),
            qos_reduced: level.rank() >= PerformanceControlLevel::QosReduced.rank(),
            suspend_requested: level.rank() >= PerformanceControlLevel::Suspended.rank(),
            shutdown_requested: level.rank() >= PerformanceControlLevel::ShutdownRequested.rank(),
        })
    }

    fn sync_workloads(
        &mut self,
        workloads: &[PerformanceControlWorkload],
    ) -> RuntimeHostResult<()> {
        let mut active = BTreeSet::new();
        for workload in workloads {
            if workload.instance_id.is_empty() || !active.insert(workload.instance_id.clone()) {
                return Err(control_fatal(
                    HostCode::PerformanceControlWorkloadInvalid,
                    HostLocation::SyncPerformanceWorkloads,
                ));
            }
            self.instance_levels
                .entry(workload.instance_id.clone())
                .or_insert(self.level);
            match workload.arbitration {
                Some(rank) => {
                    self.instance_arbitration
                        .insert(workload.instance_id.clone(), rank);
                }
                None => {
                    self.instance_arbitration.remove(&workload.instance_id);
                }
            }
        }
        // An instance raised above the global level on its own keeps that level without a
        // workload until recovery lowers it; only one at or below the global level leaves.
        let global = self.level;
        self.instance_levels.retain(|instance_id, level| {
            active.contains(instance_id) || level.rank() > global.rank()
        });
        self.instance_arbitration
            .retain(|instance_id, _| active.contains(instance_id));
        Ok(())
    }

    fn recover_instances_if_due(
        &mut self,
        target: PerformanceControlLevel,
        observation: &PerformanceControlObservation,
    ) -> RuntimeHostResult<Vec<PerformanceControlEventData>> {
        self.candidate_level = None;
        if target != PerformanceControlLevel::Normal
            || observation.observed_at_unix_ms < self.cooldown_until_unix_ms
            || !self
                .instance_levels
                .values()
                .any(|level| level.rank() > self.level.rank())
        {
            self.candidate_samples = 0;
            return Ok(Vec::new());
        }
        self.candidate_samples = self.candidate_samples.saturating_add(1);
        if self.candidate_samples < self.config.recovery_samples {
            return Ok(Vec::new());
        }
        self.candidate_samples = 0;
        self.cooldown_until_unix_ms = observation
            .observed_at_unix_ms
            .checked_add(duration_ms(self.config.transition_cooldown)?)
            .ok_or_else(|| {
                control_fatal(
                    HostCode::PerformanceControlTimeOverflow,
                    HostLocation::RecoverPerformanceInstance,
                )
            })?;
        Ok(self
            .recover_one_instance(observation)?
            .into_iter()
            .collect())
    }

    /// Lowers the instance above the global level with the longest aging by one level (ties:
    /// highest utility, then instance id).
    fn recover_one_instance(
        &mut self,
        observation: &PerformanceControlObservation,
    ) -> RuntimeHostResult<Option<PerformanceControlEventData>> {
        let global = self.level;
        let candidates = self
            .instance_levels
            .iter()
            .filter(|(_, level)| level.rank() > global.rank())
            .map(|(instance_id, _)| instance_id.as_str())
            .collect::<Vec<_>>();
        let Some((instance_id, arbitration)) = self.arbitrate(&candidates, |left, right| {
            right
                .aging_ms
                .cmp(&left.aging_ms)
                .then_with(|| right.utility_milli.cmp(&left.utility_milli))
        })?
        else {
            return Ok(None);
        };
        let previous = *self.instance_levels.get(&instance_id).ok_or_else(|| {
            control_fatal(
                HostCode::PerformanceControlInstanceMissing,
                HostLocation::RecoverPerformanceInstance,
            )
        })?;
        let level = previous_level(previous).max(global);
        self.instance_levels.insert(instance_id.clone(), level);
        let mut event = self.event(
            Some(instance_id),
            previous,
            level,
            PerformanceControlReason::Recovery,
            true,
            None,
            observation,
        );
        event.arbitration = Some(arbitration);
        Ok(Some(event))
    }

    /// Raises the instance below `step` with the lowest utility to `step` (ties: shortest
    /// aging, then instance id).
    fn suspend_one_instance(
        &mut self,
        step: PerformanceControlLevel,
        reason: PerformanceControlReason,
        observation: &PerformanceControlObservation,
    ) -> RuntimeHostResult<Option<PerformanceControlEventData>> {
        let candidates = self
            .instance_levels
            .iter()
            .filter(|(_, level)| level.rank() < step.rank())
            .map(|(instance_id, _)| instance_id.as_str())
            .collect::<Vec<_>>();
        let Some((instance_id, arbitration)) = self.arbitrate(&candidates, |left, right| {
            left.utility_milli
                .cmp(&right.utility_milli)
                .then_with(|| left.aging_ms.cmp(&right.aging_ms))
        })?
        else {
            return Ok(None);
        };
        let previous = self
            .instance_levels
            .insert(instance_id.clone(), step)
            .ok_or_else(|| {
                control_fatal(
                    HostCode::PerformanceControlInstanceMissing,
                    HostLocation::SuspendPerformanceInstance,
                )
            })?;
        let mut event = self.event(
            Some(instance_id),
            previous,
            step,
            reason,
            false,
            None,
            observation,
        );
        event.arbitration = Some(arbitration);
        Ok(Some(event))
    }

    /// Picks the first candidate under `order` (then instance id) when every candidate has an
    /// arbitration input, otherwise the first candidate in instance id order.
    fn arbitrate(
        &self,
        candidates: &[&str],
        order: impl Fn(&InstanceArbitrationRank, &InstanceArbitrationRank) -> std::cmp::Ordering,
    ) -> RuntimeHostResult<Option<(String, ArbitrationBasis)>> {
        let count = u16::try_from(candidates.len()).map_err(|_| {
            control_fatal(
                HostCode::PerformanceControlCandidatesOverflow,
                HostLocation::ArbitratePerformanceInstances,
            )
        })?;
        let ranked = candidates
            .iter()
            .map(|instance_id| {
                self.instance_arbitration
                    .get(*instance_id)
                    .map(|rank| (*instance_id, rank))
            })
            .collect::<Option<Vec<_>>>();
        let (instance_id, rank, basis) = match ranked {
            Some(ranked) => {
                let Some((instance_id, rank)) = ranked
                    .into_iter()
                    .min_by(|left, right| order(left.1, right.1).then_with(|| left.0.cmp(right.0)))
                else {
                    return Ok(None);
                };
                (instance_id, Some(*rank), ArbitrationBasisKind::Utility)
            }
            None => {
                let Some(instance_id) = candidates.iter().min() else {
                    return Ok(None);
                };
                (
                    *instance_id,
                    self.instance_arbitration.get(*instance_id).copied(),
                    ArbitrationBasisKind::LexicalFallback,
                )
            }
        };
        Ok(Some((
            instance_id.to_owned(),
            ArbitrationBasis {
                utility_milli: rank.map_or(0, |rank| rank.utility_milli),
                aging_ms: rank.map_or(0, |rank| rank.aging_ms),
                candidates: count,
                basis,
            },
        )))
    }

    #[allow(clippy::too_many_arguments)]
    fn event(
        &self,
        instance_id: Option<String>,
        previous_level: PerformanceControlLevel,
        level: PerformanceControlLevel,
        reason: PerformanceControlReason,
        recovery: bool,
        deadline_disposition: Option<PerformanceDeadlineDisposition>,
        observation: &PerformanceControlObservation,
    ) -> PerformanceControlEventData {
        PerformanceControlEventData {
            observed_at_unix_ms: observation.observed_at_unix_ms,
            instance_id,
            previous_level,
            level,
            reason,
            host_responsiveness_basis_points: observation.host_responsiveness_basis_points,
            third_party_pressure_basis_points: observation.third_party_pressure_basis_points,
            recovery,
            deadline_disposition,
            arbitration: None,
        }
    }
}

fn target_level(observation: &PerformanceControlObservation) -> Option<PerformanceControlLevel> {
    let responsiveness = observation
        .host_responsiveness_basis_points
        .map(responsiveness_level);
    let third_party = observation
        .third_party_pressure_basis_points
        .map(third_party_level);
    let mut target = match (responsiveness, third_party) {
        (Some(left), Some(right)) => left.max(right),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => return None,
    };
    if observation.foreground_fullscreen
        && observation
            .third_party_pressure_basis_points
            .is_some_and(|value| value > 0)
        && observation.host_responsiveness_basis_points.is_some()
    {
        target = next_level(target);
    }
    Some(target)
}

fn responsiveness_level(value: u16) -> PerformanceControlLevel {
    match value {
        8_500..=10_000 => PerformanceControlLevel::Normal,
        7_500..=8_499 => PerformanceControlLevel::DispatchPaused,
        6_500..=7_499 => PerformanceControlLevel::Throttled,
        5_500..=6_499 => PerformanceControlLevel::YieldRequested,
        4_500..=5_499 => PerformanceControlLevel::QosReduced,
        3_000..=4_499 => PerformanceControlLevel::Suspended,
        _ => PerformanceControlLevel::ShutdownRequested,
    }
}

fn third_party_level(value: u16) -> PerformanceControlLevel {
    match value {
        0..=1_000 => PerformanceControlLevel::Normal,
        1_001..=2_500 => PerformanceControlLevel::DispatchPaused,
        2_501..=4_000 => PerformanceControlLevel::Throttled,
        4_001..=5_500 => PerformanceControlLevel::YieldRequested,
        5_501..=7_000 => PerformanceControlLevel::QosReduced,
        7_001..=8_500 => PerformanceControlLevel::Suspended,
        _ => PerformanceControlLevel::ShutdownRequested,
    }
}

fn control_reason(observation: &PerformanceControlObservation) -> PerformanceControlReason {
    if observation.foreground_fullscreen
        && observation
            .third_party_pressure_basis_points
            .is_some_and(|value| value > 0)
        && observation.host_responsiveness_basis_points.is_some()
    {
        return PerformanceControlReason::FullscreenContention;
    }
    let response_level = observation
        .host_responsiveness_basis_points
        .map(responsiveness_level)
        .unwrap_or(PerformanceControlLevel::Normal);
    let third_party_level = observation
        .third_party_pressure_basis_points
        .map(third_party_level)
        .unwrap_or(PerformanceControlLevel::Normal);
    if third_party_level.rank() > response_level.rank() {
        PerformanceControlReason::ThirdPartyContention
    } else {
        PerformanceControlReason::ResponsivenessPressure
    }
}

const fn next_level(level: PerformanceControlLevel) -> PerformanceControlLevel {
    match level {
        PerformanceControlLevel::Normal => PerformanceControlLevel::DispatchPaused,
        PerformanceControlLevel::DispatchPaused => PerformanceControlLevel::Throttled,
        PerformanceControlLevel::Throttled => PerformanceControlLevel::YieldRequested,
        PerformanceControlLevel::YieldRequested => PerformanceControlLevel::QosReduced,
        PerformanceControlLevel::QosReduced => PerformanceControlLevel::Suspended,
        PerformanceControlLevel::Suspended | PerformanceControlLevel::ShutdownRequested => {
            PerformanceControlLevel::ShutdownRequested
        }
    }
}

const fn previous_level(level: PerformanceControlLevel) -> PerformanceControlLevel {
    match level {
        PerformanceControlLevel::Normal | PerformanceControlLevel::DispatchPaused => {
            PerformanceControlLevel::Normal
        }
        PerformanceControlLevel::Throttled => PerformanceControlLevel::DispatchPaused,
        PerformanceControlLevel::YieldRequested => PerformanceControlLevel::Throttled,
        PerformanceControlLevel::QosReduced => PerformanceControlLevel::YieldRequested,
        PerformanceControlLevel::Suspended => PerformanceControlLevel::QosReduced,
        PerformanceControlLevel::ShutdownRequested => PerformanceControlLevel::Suspended,
    }
}

fn is_heavy(profile: &LoadProfile, _heavy_threshold_milli: u64) -> bool {
    match profile {
        LoadProfile::Heavy => true,
        LoadProfile::Light => false,
        LoadProfile::Weighted {
            cpu_milli,
            gpu_milli,
            io_milli,
        } => {
            u64::from(*cpu_milli)
                .saturating_add(u64::from(*gpu_milli))
                .saturating_add(u64::from(*io_milli))
                >= 1_500
        }
    }
}

fn duration_ms(duration: Duration) -> RuntimeHostResult<u64> {
    u64::try_from(duration.as_millis()).map_err(|_| {
        control_fatal(
            HostCode::PerformanceControlDurationOverflow,
            HostLocation::ConvertPerformanceControlDuration,
        )
    })
}

fn control_fatal(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeHalted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        at: u64,
        responsiveness: Option<u16>,
        third_party: Option<u16>,
    ) -> PerformanceControlObservation {
        PerformanceControlObservation {
            observed_at_unix_ms: at,
            host_responsiveness_basis_points: responsiveness,
            third_party_pressure_basis_points: third_party,
            foreground_fullscreen: false,
        }
    }

    fn workload(instance_id: &str, load_profile: LoadProfile) -> PerformanceControlWorkload {
        PerformanceControlWorkload {
            instance_id: instance_id.to_owned(),
            load_profile,
            arbitration: None,
        }
    }

    #[test]
    fn escalation_is_bounded_by_hysteresis_and_cooldown() {
        let mut controller =
            PerformanceBalanceController::new(PerformanceControlConfig::default()).expect("new");
        let workloads = [workload("instance-a", LoadProfile::Heavy)];
        assert!(
            controller
                .observe(observation(1_000, Some(5_000), Some(0)), &workloads)
                .expect("first")
                .is_empty()
        );
        let events = controller
            .observe(observation(3_000, Some(5_000), Some(0)), &workloads)
            .expect("second");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].level, PerformanceControlLevel::DispatchPaused);
        assert!(
            controller
                .observe(observation(4_000, Some(5_000), Some(0)), &workloads)
                .expect("cooldown")
                .is_empty()
        );
    }

    #[test]
    fn duplicate_sample_identity_cannot_advance_hysteresis() {
        let mut controller =
            PerformanceBalanceController::new(PerformanceControlConfig::default()).expect("new");
        let pressure = observation(1_000, Some(5_000), Some(0));
        assert!(
            controller
                .observe(pressure.clone(), &[])
                .expect("first")
                .is_empty()
        );
        assert_eq!(controller.candidate_samples, 1);
        assert!(
            controller
                .observe(pressure.clone(), &[])
                .expect("idempotent duplicate")
                .is_empty()
        );
        assert_eq!(controller.candidate_samples, 1);
        assert_eq!(controller.level, PerformanceControlLevel::Normal);

        let conflict = controller
            .observe(observation(1_000, Some(10_000), Some(0)), &[])
            .expect_err("same sample identity cannot carry different evidence");
        assert_eq!(
            conflict.code(),
            "performance_control_sample_identity_conflict"
        );
        assert_eq!(controller.candidate_samples, 1);
        assert_eq!(controller.level, PerformanceControlLevel::Normal);

        let events = controller
            .observe(observation(3_000, Some(5_000), Some(0)), &[])
            .expect("second fresh sample");
        assert_eq!(events.len(), 1);
        assert_eq!(controller.level, PerformanceControlLevel::DispatchPaused);
    }

    #[test]
    fn third_party_contention_is_independent_and_presence_alone_is_not_pressure() {
        let mut controller =
            PerformanceBalanceController::new(PerformanceControlConfig::default()).expect("new");
        assert!(
            controller
                .observe(observation(1_000, None, Some(0)), &[])
                .expect("idle")
                .is_empty()
        );
        controller
            .observe(observation(3_000, None, Some(8_000)), &[])
            .expect("first pressure");
        let events = controller
            .observe(observation(5_000, None, Some(8_000)), &[])
            .expect("second pressure");
        assert_eq!(
            events[0].reason,
            PerformanceControlReason::ThirdPartyContention
        );
    }

    #[test]
    fn recovery_releases_only_one_instance_per_transition() {
        let config = PerformanceControlConfig {
            transition_cooldown: Duration::from_millis(1),
            escalation_samples: 1,
            recovery_samples: 1,
            ..PerformanceControlConfig::default()
        };
        let mut controller = PerformanceBalanceController::new(config).expect("new");
        let workloads = [
            workload("instance-a", LoadProfile::Heavy),
            workload("instance-b", LoadProfile::Heavy),
        ];
        controller
            .observe(observation(1_000, Some(5_000), Some(0)), &workloads)
            .expect("escalate");
        controller
            .observe(observation(1_002, Some(5_000), Some(0)), &workloads)
            .expect("escalate again");
        let events = controller
            .observe(observation(1_004, Some(10_000), Some(0)), &workloads)
            .expect("recover");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.instance_id.is_some())
                .count(),
            1
        );
        let levels = [
            controller.directive("instance-a").expect("a").level,
            controller.directive("instance-b").expect("b").level,
        ];
        assert_ne!(levels[0], levels[1]);
    }

    #[test]
    fn clock_jump_preserves_control_state_and_resets_transition_evidence() {
        let config = PerformanceControlConfig {
            escalation_samples: 1,
            ..PerformanceControlConfig::default()
        };
        let mut controller = PerformanceBalanceController::new(config).expect("new");
        controller
            .observe(observation(1_000, Some(5_000), Some(0)), &[])
            .expect("escalate");
        let level = controller.level;
        let events = controller
            .observe(observation(100_000, Some(10_000), Some(0)), &[])
            .expect("clock jump");
        assert_eq!(controller.level, level);
        assert_eq!(events[0].reason, PerformanceControlReason::ClockJump);
    }

    #[test]
    fn resource_overlay_caps_heavy_concurrency() {
        let config = PerformanceControlConfig {
            escalation_samples: 1,
            ..PerformanceControlConfig::default()
        };
        let mut controller = PerformanceBalanceController::new(config).expect("new");
        controller
            .observe(observation(1_000, Some(6_000), Some(0)), &[])
            .expect("pressure");
        controller
            .observe(observation(6_000, Some(6_000), Some(0)), &[])
            .expect("pressure two");
        let mut resources = [HostResourceSnapshot {
            host_id: "host-a".to_owned(),
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 2,
            active_heavy_dispatches: 0,
        }];
        controller
            .apply_to_resources(
                &mut resources,
                &[workload("instance-a", LoadProfile::Heavy)],
            )
            .expect("overlay");
        assert_eq!(resources[0].heavy_dispatch_limit, 1);
        assert_eq!(resources[0].active_heavy_dispatches, 1);
    }
}
