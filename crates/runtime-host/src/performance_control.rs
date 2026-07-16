// SPDX-License-Identifier: AGPL-3.0-only

//! Contention-driven Runtime control with bounded escalation and staged recovery.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    PerformanceControlEventData, PerformanceControlLevel, PerformanceControlReason,
    PerformanceDeadlineDisposition, RuntimeErrorCode,
};
use actingcommand_policy::{HostResourceSnapshot, LoadProfile};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const BASIS_POINTS_MAX: u16 = 10_000;

#[derive(Debug, Clone)]
pub struct PerformanceControlConfig {
    escalation_samples: u16,
    recovery_samples: u16,
    transition_cooldown: Duration,
    clock_jump_threshold: Duration,
    normal_heavy_dispatch_limit: u16,
    pressured_heavy_dispatch_limit: u16,
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

    pub fn validate(&self) -> RuntimeHostResult<()> {
        if self.escalation_samples == 0
            || self.recovery_samples == 0
            || self.transition_cooldown.is_zero()
            || self.clock_jump_threshold < self.transition_cooldown
            || self.normal_heavy_dispatch_limit == 0
            || self.pressured_heavy_dispatch_limit == 0
            || self.pressured_heavy_dispatch_limit > self.normal_heavy_dispatch_limit
        {
            return Err(control_fatal(
                "performance_control_config_invalid",
                "validate_performance_control_config",
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
                "performance_control_observation_invalid",
                "observe_performance_contention",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceControlWorkload {
    pub instance_id: String,
    pub load_profile: LoadProfile,
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
        reason: &'static str,
        deadline_disposition: Option<PerformanceDeadlineDisposition>,
        event: Option<PerformanceControlEventData>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct PerformanceBalanceController {
    config: PerformanceControlConfig,
    level: PerformanceControlLevel,
    instance_levels: BTreeMap<String, PerformanceControlLevel>,
    last_observed_at_unix_ms: Option<u64>,
    cooldown_until_unix_ms: u64,
    candidate_level: Option<PerformanceControlLevel>,
    candidate_samples: u16,
    last_observation: Option<PerformanceControlObservation>,
}

impl PerformanceBalanceController {
    pub(crate) fn new(config: PerformanceControlConfig) -> RuntimeHostResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            level: PerformanceControlLevel::Normal,
            instance_levels: BTreeMap::new(),
            last_observed_at_unix_ms: None,
            cooldown_until_unix_ms: 0,
            candidate_level: None,
            candidate_samples: 0,
            last_observation: None,
        })
    }

    pub(crate) fn observe(
        &mut self,
        observation: PerformanceControlObservation,
        workloads: &[PerformanceControlWorkload],
    ) -> RuntimeHostResult<Vec<PerformanceControlEventData>> {
        observation.validate()?;
        self.sync_workloads(workloads)?;
        if let Some(previous) = self.last_observed_at_unix_ms {
            let jump_threshold = duration_ms(self.config.clock_jump_threshold)?;
            if observation.observed_at_unix_ms < previous
                || observation.observed_at_unix_ms.saturating_sub(previous) > jump_threshold
            {
                self.candidate_level = None;
                self.candidate_samples = 0;
                self.cooldown_until_unix_ms = observation
                    .observed_at_unix_ms
                    .checked_add(duration_ms(self.config.transition_cooldown)?)
                    .ok_or_else(|| {
                        control_fatal(
                            "performance_control_time_overflow",
                            "observe_performance_clock_jump",
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
        if target == self.level {
            return self.recover_instances_if_due(target, &observation);
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
                    "performance_control_time_overflow",
                    "transition_performance_control",
                )
            })?;
        let previous = self.level;
        let recovery = target.rank() < previous.rank();
        self.level = if recovery {
            previous_level(previous)
        } else {
            next_level(previous)
        };
        if !recovery {
            for level in self.instance_levels.values_mut() {
                if level.rank() < self.level.rank() {
                    *level = self.level;
                }
            }
        }
        let reason = if recovery {
            PerformanceControlReason::Recovery
        } else {
            control_reason(&observation)
        };
        let mut events = vec![self.event(
            None,
            previous,
            self.level,
            reason,
            recovery,
            None,
            &observation,
        )];
        if recovery && let Some(event) = self.recover_one_instance(&observation) {
            events.push(event);
        }
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
                .filter(|workload| is_heavy(&workload.load_profile))
                .count(),
        )
        .map_err(|_| {
            control_fatal(
                "performance_heavy_count_overflow",
                "apply_performance_control_resources",
            )
        })?;
        let budget_factor = control_budget_factor_milli(self.level);
        for host in resources {
            if let Some(observation) = &self.last_observation {
                if let Some(value) = observation.host_responsiveness_basis_points {
                    host.host_responsiveness_basis_points = value;
                }
                if let Some(value) = observation.third_party_pressure_basis_points {
                    host.third_party_pressure_basis_points = value;
                }
            }
            host.cpu_available_milli = scaled_budget(host.cpu_available_milli, budget_factor)?;
            host.gpu_available_milli = scaled_budget(host.gpu_available_milli, budget_factor)?;
            host.io_available_milli = scaled_budget(host.io_available_milli, budget_factor)?;
            host.heavy_dispatch_limit = if self.level == PerformanceControlLevel::Normal {
                self.config.normal_heavy_dispatch_limit
            } else {
                self.config.pressured_heavy_dispatch_limit
            };
            host.active_heavy_dispatches = heavy_count.min(host.heavy_dispatch_limit);
        }
        Ok(())
    }

    pub(crate) fn gate_dispatch(
        &self,
        instance_id: &str,
        urgency_milli: u16,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<PerformanceDispatchGate> {
        if urgency_milli > 1_000 || instance_id.is_empty() || observed_at_unix_ms == 0 {
            return Err(control_fatal(
                "performance_dispatch_gate_invalid",
                "gate_performance_dispatch",
            ));
        }
        if self.level == PerformanceControlLevel::Normal {
            return Ok(PerformanceDispatchGate::Allowed);
        }
        let deadline_disposition = if urgency_milli >= 950 {
            Some(PerformanceDeadlineDisposition::CapacityFailure)
        } else if urgency_milli >= 750 {
            Some(PerformanceDeadlineDisposition::InformationWarning)
        } else {
            None
        };
        let event = deadline_disposition.map(|disposition| {
            let mut observation =
                self.last_observation
                    .clone()
                    .unwrap_or(PerformanceControlObservation {
                        observed_at_unix_ms,
                        host_responsiveness_basis_points: None,
                        third_party_pressure_basis_points: None,
                        foreground_fullscreen: false,
                    });
            observation.observed_at_unix_ms = observed_at_unix_ms;
            self.event(
                Some(instance_id.to_owned()),
                self.level,
                self.level,
                PerformanceControlReason::DeadlineConflict,
                false,
                Some(disposition),
                &observation,
            )
        });
        Ok(PerformanceDispatchGate::Deferred {
            reason: if deadline_disposition == Some(PerformanceDeadlineDisposition::CapacityFailure)
            {
                "performance_capacity_deadline_conflict"
            } else {
                "performance_contention_dispatch_deferred"
            },
            deadline_disposition,
            event,
        })
    }

    pub(crate) fn directive(
        &self,
        instance_id: &str,
    ) -> RuntimeHostResult<PerformanceControlDirective> {
        if instance_id.is_empty() {
            return Err(control_fatal(
                "performance_control_instance_invalid",
                "read_performance_control_directive",
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
                    "performance_control_workload_invalid",
                    "sync_performance_workloads",
                ));
            }
            self.instance_levels
                .entry(workload.instance_id.clone())
                .or_insert(self.level);
        }
        self.instance_levels
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
                    "performance_control_time_overflow",
                    "recover_performance_instance",
                )
            })?;
        Ok(self.recover_one_instance(observation).into_iter().collect())
    }

    fn recover_one_instance(
        &mut self,
        observation: &PerformanceControlObservation,
    ) -> Option<PerformanceControlEventData> {
        let instance_id = self
            .instance_levels
            .iter()
            .find(|(_, level)| level.rank() > self.level.rank())
            .map(|(instance_id, _)| instance_id.clone())?;
        let previous = *self.instance_levels.get(&instance_id)?;
        let level = previous_level(previous).max(self.level);
        self.instance_levels.insert(instance_id.clone(), level);
        Some(self.event(
            Some(instance_id),
            previous,
            level,
            PerformanceControlReason::Recovery,
            true,
            None,
            observation,
        ))
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

const fn control_budget_factor_milli(level: PerformanceControlLevel) -> u16 {
    match level {
        PerformanceControlLevel::Normal | PerformanceControlLevel::DispatchPaused => 1_000,
        PerformanceControlLevel::Throttled => 750,
        PerformanceControlLevel::YieldRequested => 600,
        PerformanceControlLevel::QosReduced => 500,
        PerformanceControlLevel::Suspended => 250,
        PerformanceControlLevel::ShutdownRequested => 0,
    }
}

fn scaled_budget(value: u16, factor_milli: u16) -> RuntimeHostResult<u16> {
    u16::try_from(u32::from(value).saturating_mul(u32::from(factor_milli)) / 1_000).map_err(|_| {
        control_fatal(
            "performance_budget_overflow",
            "apply_performance_control_resources",
        )
    })
}

fn is_heavy(profile: &LoadProfile) -> bool {
    match profile {
        LoadProfile::Heavy => true,
        LoadProfile::Light => false,
        LoadProfile::Weighted {
            cpu_milli,
            gpu_milli,
            io_milli,
        } => {
            u32::from(*cpu_milli)
                .saturating_add(u32::from(*gpu_milli))
                .saturating_add(u32::from(*io_milli))
                >= 1_500
        }
    }
}

fn duration_ms(duration: Duration) -> RuntimeHostResult<u64> {
    u64::try_from(duration.as_millis()).map_err(|_| {
        control_fatal(
            "performance_control_duration_overflow",
            "convert_performance_control_duration",
        )
    })
}

fn control_fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
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
    fn deadline_conflict_never_crosses_the_dispatch_gate() {
        let config = PerformanceControlConfig {
            escalation_samples: 1,
            ..PerformanceControlConfig::default()
        };
        let mut controller = PerformanceBalanceController::new(config).expect("new");
        controller
            .observe(observation(1_000, Some(7_000), Some(0)), &[])
            .expect("pressure");
        let PerformanceDispatchGate::Deferred {
            deadline_disposition,
            event,
            ..
        } = controller
            .gate_dispatch("instance-a", 1_000, 1_001)
            .expect("gate")
        else {
            panic!("deadline must not bypass contention");
        };
        assert_eq!(
            deadline_disposition,
            Some(PerformanceDeadlineDisposition::CapacityFailure)
        );
        assert_eq!(
            event.expect("deadline event").reason,
            PerformanceControlReason::DeadlineConflict
        );
    }

    #[test]
    fn resource_overlay_caps_heavy_concurrency_and_scales_budgets() {
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
        assert!(resources[0].cpu_available_milli < 1_000);
    }
}
