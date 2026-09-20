// SPDX-License-Identifier: AGPL-3.0-only

//! Builds the in-memory runtime configuration manifest from what `assemble` actually
//! applies: which subsystems the host runs and why, and every effective parameter value
//! with its source. Data only; the host records it as the two `config.*` runtime facts.

use actingcommand_contract::{
    CapacityThresholds, ConfigParameter, ConfigParameterSource, ConfigSubsystem,
    DeviceDiagnosticMode, FactScalar, RuntimeConfigManifest,
};
use actingcommand_device::{
    MUMU_MANAGER_CONTROL_TIMEOUT, MUMU_MANAGER_STATE_WAIT_START, MUMU_MANAGER_STATE_WAIT_STOP,
};
use actingcommand_runtime_host::{
    PerformanceControlConfig, PerformanceMonitorConfig, PolicyCadence, SchedulerConfig,
};
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

/// Everything the manifest reports, taken from the configuration file and the assembled
/// host before either is consumed. `None` in an `Option` of a defaulted field means the
/// file did not name it, so the library default applies.
pub(super) struct ManifestInputs<'a> {
    pub(super) bind_host: IpAddr,
    pub(super) bind_port: Option<u16>,
    pub(super) device_diagnostic_mode: Option<DeviceDiagnosticMode>,
    pub(super) frame_retention_enabled: Option<bool>,
    pub(super) capacity_thresholds: Option<CapacityThresholds>,
    pub(super) secret_fingerprint_salt_bytes: usize,
    pub(super) mumu_root: Option<&'a Path>,
    pub(super) governance_configured: bool,
    /// `(max_attempts, max_session_ms, max_projection_events)` of a present section.
    pub(super) agent_dispatcher: Option<(u16, u64, u16)>,
    pub(super) policy_configured: bool,
    pub(super) vision_provider_configured: bool,
    pub(super) instances_count: usize,
    pub(super) instances_deferred_count: usize,
    pub(super) instances_startup_package_count: usize,
    pub(super) policy_cadence: &'a PolicyCadence,
    pub(super) io_timeout: Duration,
    pub(super) maximum_frame_bytes: usize,
}

pub(super) fn build(inputs: &ManifestInputs<'_>) -> Result<RuntimeConfigManifest, &'static str> {
    let device_diagnostic_mode = inputs.device_diagnostic_mode.unwrap_or_default();
    let frame_retention_enabled = inputs.frame_retention_enabled.unwrap_or_default();
    let capacity_thresholds = inputs.capacity_thresholds.unwrap_or_default();
    let performance_monitor = PerformanceMonitorConfig::default();
    let performance_control = PerformanceControlConfig::default();
    let scheduler = SchedulerConfig::default();
    let discovery_bound = inputs.instances_deferred_count > 0;
    let subsystems = vec![
        subsystem(
            "frame_retention",
            frame_retention_enabled,
            match inputs.frame_retention_enabled {
                Some(true) => "configured",
                Some(false) => "configured off",
                None => "flag absent",
            },
        ),
        subsystem(
            "agent_dispatcher",
            inputs.agent_dispatcher.is_some(),
            present(inputs.agent_dispatcher.is_some(), "section absent"),
        ),
        subsystem(
            "governance",
            inputs.governance_configured,
            present(inputs.governance_configured, "capability absent"),
        ),
        subsystem(
            "policy_driver",
            inputs.policy_configured,
            present(inputs.policy_configured, "section absent"),
        ),
        subsystem(
            "vision_provider",
            inputs.vision_provider_configured,
            present(inputs.vision_provider_configured, "manifest absent"),
        ),
        ConfigSubsystem {
            name: "device_diagnostic".to_owned(),
            enabled: true,
            reason: format!(
                "always on; mode {}",
                device_diagnostic_mode_name(device_diagnostic_mode)
            ),
        },
        ConfigSubsystem {
            name: "performance_monitor".to_owned(),
            enabled: true,
            reason: format!(
                "sample interval {} ms (default)",
                performance_monitor.sample_interval().as_millis()
            ),
        },
        ConfigSubsystem {
            name: "mumu_discovery".to_owned(),
            enabled: discovery_bound,
            reason: if discovery_bound {
                format!(
                    "instances bound by instance_index or instance_name: {}",
                    inputs.instances_deferred_count
                )
            } else {
                "no instance bound by instance_index or instance_name".to_owned()
            },
        },
        ConfigSubsystem {
            name: "emulator_control".to_owned(),
            enabled: discovery_bound,
            reason: if discovery_bound {
                format!(
                    "discovery-bound instances: {}",
                    inputs.instances_deferred_count
                )
            } else {
                "no discovery-bound instance".to_owned()
            },
        },
        subsystem(
            "runtime_fact_snapshot",
            true,
            "rides the performance monitor thread",
        ),
    ];
    let explicit_or_default = |present: bool| {
        if present {
            ConfigParameterSource::Explicit
        } else {
            ConfigParameterSource::Default
        }
    };
    let mut parameters = vec![
        explicit(
            "bind_host",
            FactScalar::String(inputs.bind_host.to_string()),
        ),
        ConfigParameter {
            key: "bind_port".to_owned(),
            value: FactScalar::Integer(i64::from(inputs.bind_port.unwrap_or_default())),
            source: explicit_or_default(inputs.bind_port.is_some()),
        },
        ConfigParameter {
            key: "device_diagnostic_mode".to_owned(),
            value: FactScalar::String(
                device_diagnostic_mode_name(device_diagnostic_mode).to_owned(),
            ),
            source: explicit_or_default(inputs.device_diagnostic_mode.is_some()),
        },
        ConfigParameter {
            key: "frame_retention_enabled".to_owned(),
            value: FactScalar::Boolean(frame_retention_enabled),
            source: explicit_or_default(inputs.frame_retention_enabled.is_some()),
        },
        explicit(
            "secret_fingerprint_salt_bytes",
            integer(inputs.secret_fingerprint_salt_bytes)?,
        ),
    ];
    if let Some(mumu_root) = inputs.mumu_root {
        parameters.push(explicit(
            "mumu_root",
            FactScalar::String(mumu_root.display().to_string()),
        ));
    }
    parameters.extend([
        explicit("instances_count", integer(inputs.instances_count)?),
        explicit(
            "instances_deferred_count",
            integer(inputs.instances_deferred_count)?,
        ),
        explicit(
            "instances_startup_package_count",
            integer(inputs.instances_startup_package_count)?,
        ),
        default(
            "scheduler.maximum_client_heartbeat_interval_ms",
            FactScalar::DurationMs(scheduler.maximum_client_heartbeat_interval_ms),
        ),
        default(
            "scheduler.takeover_cooldown_ms",
            FactScalar::DurationMs(scheduler.takeover_cooldown_ms),
        ),
        default(
            "scheduler.lease_ttl_ms",
            FactScalar::DurationMs(scheduler.lease_ttl_ms),
        ),
        default(
            "scheduler.maximum_queue_timeout_ms",
            FactScalar::DurationMs(scheduler.maximum_queue_timeout_ms),
        ),
        default(
            "scheduler.max_queue_depth_per_instance",
            integer(scheduler.max_queue_depth_per_instance)?,
        ),
        default(
            "policy_cadence.debounce_ms",
            FactScalar::DurationMs(inputs.policy_cadence.debounce_ms),
        ),
        default(
            "policy_cadence.cooldown_ms",
            FactScalar::DurationMs(inputs.policy_cadence.cooldown_ms),
        ),
        default(
            "policy_cadence.reconciliation_interval_ms",
            FactScalar::DurationMs(inputs.policy_cadence.reconciliation_interval_ms),
        ),
        default(
            "policy_cadence.clock_jump_threshold_ms",
            FactScalar::DurationMs(inputs.policy_cadence.clock_jump_threshold_ms),
        ),
        default("io_timeout_ms", duration_ms(inputs.io_timeout)?),
        default("maximum_frame_bytes", integer(inputs.maximum_frame_bytes)?),
        default(
            "performance_control.escalation_samples",
            FactScalar::Integer(i64::from(performance_control.escalation_samples())),
        ),
        default(
            "performance_control.recovery_samples",
            FactScalar::Integer(i64::from(performance_control.recovery_samples())),
        ),
        default(
            "performance_control.transition_cooldown_ms",
            duration_ms(performance_control.transition_cooldown())?,
        ),
        default(
            "performance_control.clock_jump_threshold_ms",
            duration_ms(performance_control.clock_jump_threshold())?,
        ),
        default(
            "performance_control.normal_heavy_dispatch_limit",
            FactScalar::Integer(i64::from(performance_control.normal_heavy_dispatch_limit())),
        ),
        default(
            "performance_control.pressured_heavy_dispatch_limit",
            FactScalar::Integer(i64::from(
                performance_control.pressured_heavy_dispatch_limit(),
            )),
        ),
        default(
            "performance_monitor.sample_interval_ms",
            duration_ms(performance_monitor.sample_interval())?,
        ),
        ConfigParameter {
            key: "capacity_thresholds.hard_bytes".to_owned(),
            value: integer(capacity_thresholds.hard_bytes)?,
            source: explicit_or_default(inputs.capacity_thresholds.is_some()),
        },
        ConfigParameter {
            key: "capacity_thresholds.soft_bytes".to_owned(),
            value: integer(capacity_thresholds.soft_bytes)?,
            source: explicit_or_default(inputs.capacity_thresholds.is_some()),
        },
    ]);
    if let Some((max_attempts, max_session_ms, max_projection_events)) = inputs.agent_dispatcher {
        parameters.extend([
            explicit(
                "agent_dispatcher.max_attempts",
                FactScalar::Integer(i64::from(max_attempts)),
            ),
            explicit(
                "agent_dispatcher.max_session_ms",
                FactScalar::DurationMs(max_session_ms),
            ),
            explicit(
                "agent_dispatcher.max_projection_events",
                FactScalar::Integer(i64::from(max_projection_events)),
            ),
        ]);
    }
    parameters.extend([
        default(
            "mumu_manager.control_timeout_ms",
            duration_ms(MUMU_MANAGER_CONTROL_TIMEOUT)?,
        ),
        default(
            "mumu_manager.state_wait_start_ms",
            duration_ms(MUMU_MANAGER_STATE_WAIT_START)?,
        ),
        default(
            "mumu_manager.state_wait_stop_ms",
            duration_ms(MUMU_MANAGER_STATE_WAIT_STOP)?,
        ),
    ]);
    let manifest = RuntimeConfigManifest {
        subsystems,
        parameters,
    };
    manifest.validate().map_err(|_| "config_manifest_invalid")?;
    Ok(manifest)
}

fn subsystem(name: &str, enabled: bool, reason: &str) -> ConfigSubsystem {
    ConfigSubsystem {
        name: name.to_owned(),
        enabled,
        reason: reason.to_owned(),
    }
}

/// "configured" for a present section or capability, otherwise the absence reason.
const fn present(configured: bool, absent: &'static str) -> &'static str {
    if configured { "configured" } else { absent }
}

fn explicit(key: &str, value: FactScalar) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source: ConfigParameterSource::Explicit,
    }
}

fn default(key: &str, value: FactScalar) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source: ConfigParameterSource::Default,
    }
}

fn integer(value: impl TryInto<i64>) -> Result<FactScalar, &'static str> {
    value
        .try_into()
        .map(FactScalar::Integer)
        .map_err(|_| "config_manifest_value_out_of_range")
}

fn duration_ms(value: Duration) -> Result<FactScalar, &'static str> {
    u64::try_from(value.as_millis())
        .map(FactScalar::DurationMs)
        .map_err(|_| "config_manifest_value_out_of_range")
}

/// The wire spelling of the mode (`serde` snake case); a closed match so a new mode
/// cannot be reported under a stale name.
const fn device_diagnostic_mode_name(mode: DeviceDiagnosticMode) -> &'static str {
    match mode {
        DeviceDiagnosticMode::Shadow => "shadow",
    }
}
