// SPDX-License-Identifier: AGPL-3.0-only

//! Builds the in-memory runtime configuration manifest from what `assemble` actually
//! applies: which subsystems the host runs and why, and every effective parameter value
//! with its source. Every value is read back from the assembled `RuntimeHostConfig`; the
//! configuration file only decides whether a value is `explicit` or `default`. Data only;
//! the host records it as the two `config.*` runtime facts.

use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSubsystem, DeviceDiagnosticMode, FactScalar,
    InstanceId, RuntimeConfigManifest,
};
use actingcommand_device::{
    MUMU_MANAGER_CONTROL_TIMEOUT, MUMU_MANAGER_STATE_WAIT_START, MUMU_MANAGER_STATE_WAIT_STOP,
};
use actingcommand_runtime_host::RuntimeHostConfig;
use std::path::Path;
use std::time::Duration;

/// Everything the manifest reports: the assembled host, whose effective values are read
/// back, and what the configuration file named, which decides each parameter's source.
pub(super) struct ManifestInputs<'a> {
    pub(super) host: &'a RuntimeHostConfig,
    pub(super) bind_port_explicit: bool,
    pub(super) device_diagnostic_mode_explicit: bool,
    /// The file's `frame_retention_enabled` as declared; the subsystem reason names it.
    pub(super) frame_retention_enabled: Option<bool>,
    pub(super) failed_run_successes_explicit: bool,
    pub(super) failed_run_days_explicit: bool,
    pub(super) capacity_thresholds_explicit: bool,
    pub(super) pressure_start_samples_explicit: bool,
    pub(super) pressure_end_samples_explicit: bool,
    pub(super) secret_fingerprint_salt_bytes: usize,
    pub(super) mumu_root: Option<&'a Path>,
    /// The daemon-level `device_paths` by manifest name; `None` is not configured.
    pub(super) device_paths: [(&'static str, Option<&'a Path>); 5],
    /// The file's `allow_env_overrides` as declared (Workflow #318 cfg3).
    pub(super) allow_env_overrides: Option<bool>,
    /// The set `ACTINGCOMMAND_*` variables ignored because the flag is off.
    pub(super) ignored_env_overrides: &'a [&'static str],
    /// Whether the file named a `governance` section (Workflow #318 cfg4).
    pub(super) governance_allowed_clients_explicit: bool,
    /// `(max_attempts, max_session_ms, max_projection_events)` of a present section.
    pub(super) agent_dispatcher: Option<(u16, u64, u16)>,
    pub(super) policy_configured: bool,
    pub(super) vision_provider_configured: bool,
    pub(super) instances_count: usize,
    pub(super) instances_deferred_count: usize,
    pub(super) instances_startup_package_count: usize,
    /// Every configured instance in declaration order (Workflow #318 cfg3).
    pub(super) instances: &'a [InstanceParameters],
}

/// What the file named for one instance's manifest keys; the values are read back from
/// the host's stuck-recovery settings under `alias`.
pub(super) struct InstanceParameters {
    pub(super) instance_id: InstanceId,
    pub(super) alias: String,
    pub(super) stuck_recovery_explicit: bool,
    pub(super) stuck_recovery_cooldown_secs_explicit: bool,
}

pub(super) fn build(inputs: &ManifestInputs<'_>) -> Result<RuntimeConfigManifest, &'static str> {
    let host = inputs.host;
    let bind_address = host.bind_address();
    let device_diagnostic_mode = host.device_diagnostic_mode();
    let frame_retention_enabled = host.frame_retention_enabled();
    let failed_run_retention = host.failed_run_retention();
    let capacity_thresholds = host.capacity_thresholds();
    let scheduler = host.scheduler();
    let policy_cadence = host.policy_cadence();
    let performance_control = host.performance_control();
    // A host without a performance monitor configuration cannot report one: that is a
    // build error, never a default written in its place.
    let performance_monitor = host
        .performance_monitor()
        .ok_or("config_manifest_incomplete")?;
    let discovery_bound = inputs.instances_deferred_count > 0;
    // Workflow #318 cfg4: the effective allow-list read back from the host; `any` when the
    // host accepts every well-formed card.
    let governance_allowed_clients = host
        .governance_policy()
        .allowed_clients
        .as_ref()
        .map(|allowed| allowed.iter().map(String::as_str).collect::<Vec<_>>());
    let governance_allowed_clients_count = governance_allowed_clients
        .as_ref()
        .map_or_else(|| "any".to_owned(), |allowed| allowed.len().to_string());
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
        ConfigSubsystem {
            name: "governance".to_owned(),
            enabled: true,
            reason: format!(
                "declarative_identity; allowed_clients={governance_allowed_clients_count}"
            ),
        },
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
        env_overrides_subsystem(inputs.allow_env_overrides, inputs.ignored_env_overrides),
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
            FactScalar::String(bind_address.ip().to_string()),
        ),
        ConfigParameter {
            key: "bind_port".to_owned(),
            value: FactScalar::Integer(i64::from(bind_address.port())),
            source: explicit_or_default(inputs.bind_port_explicit),
        },
        ConfigParameter {
            key: "device_diagnostic_mode".to_owned(),
            value: FactScalar::String(
                device_diagnostic_mode_name(device_diagnostic_mode).to_owned(),
            ),
            source: explicit_or_default(inputs.device_diagnostic_mode_explicit),
        },
        ConfigParameter {
            key: "frame_retention_enabled".to_owned(),
            value: FactScalar::Boolean(frame_retention_enabled),
            source: explicit_or_default(inputs.frame_retention_enabled.is_some()),
        },
        ConfigParameter {
            key: "frame_retention_failed_run_successes".to_owned(),
            value: FactScalar::Integer(i64::from(failed_run_retention.successor_successes)),
            source: explicit_or_default(inputs.failed_run_successes_explicit),
        },
        ConfigParameter {
            key: "frame_retention_failed_run_days".to_owned(),
            value: FactScalar::Integer(i64::from(failed_run_retention.retention_days)),
            source: explicit_or_default(inputs.failed_run_days_explicit),
        },
        explicit(
            "secret_fingerprint_salt_bytes",
            integer(inputs.secret_fingerprint_salt_bytes)?,
        ),
        ConfigParameter {
            key: "allow_env_overrides".to_owned(),
            value: FactScalar::Boolean(inputs.allow_env_overrides.unwrap_or(false)),
            source: explicit_or_default(inputs.allow_env_overrides.is_some()),
        },
        ConfigParameter {
            key: "governance.allowed_clients".to_owned(),
            value: FactScalar::String(
                governance_allowed_clients
                    .map_or_else(|| "any".to_owned(), |allowed| allowed.join(",")),
            ),
            source: explicit_or_default(inputs.governance_allowed_clients_explicit),
        },
    ];
    if let Some(mumu_root) = inputs.mumu_root {
        parameters.push(explicit(
            "mumu_root",
            FactScalar::String(mumu_root.display().to_string()),
        ));
    }
    // Only configured device paths are reported; nothing discovered is invented here.
    for (name, path) in inputs.device_paths {
        if let Some(path) = path {
            parameters.push(explicit(
                &format!("device_paths.{name}"),
                FactScalar::String(path.display().to_string()),
            ));
        }
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
    ]);
    // Workflow #318 (cfg3): each instance's stuck-recovery settings, keyed by its bounded
    // registry id (an alias may exceed the 128-byte key bound).
    for instance in inputs.instances {
        let settings = host
            .stuck_recovery()
            .get(&instance.alias)
            .ok_or("config_manifest_incomplete")?;
        let prefix = format!("instance.{}", instance_id_text(instance.instance_id)?);
        parameters.extend([
            ConfigParameter {
                key: format!("{prefix}.stuck_recovery"),
                value: FactScalar::Boolean(settings.enabled),
                source: explicit_or_default(instance.stuck_recovery_explicit),
            },
            ConfigParameter {
                key: format!("{prefix}.stuck_recovery_cooldown_secs"),
                value: FactScalar::Integer(i64::from(settings.cooldown_secs)),
                source: explicit_or_default(instance.stuck_recovery_cooldown_secs_explicit),
            },
        ]);
    }
    parameters.extend([
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
            FactScalar::DurationMs(policy_cadence.debounce_ms),
        ),
        default(
            "policy_cadence.cooldown_ms",
            FactScalar::DurationMs(policy_cadence.cooldown_ms),
        ),
        default(
            "policy_cadence.reconciliation_interval_ms",
            FactScalar::DurationMs(policy_cadence.reconciliation_interval_ms),
        ),
        default(
            "policy_cadence.clock_jump_threshold_ms",
            FactScalar::DurationMs(policy_cadence.clock_jump_threshold_ms),
        ),
        default("io_timeout_ms", duration_ms(host.io_timeout())?),
        default("maximum_frame_bytes", integer(host.maximum_frame_bytes())?),
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
            key: "performance_monitor.pressure_start_samples".to_owned(),
            value: FactScalar::Integer(i64::from(performance_monitor.pressure_start_samples())),
            source: explicit_or_default(inputs.pressure_start_samples_explicit),
        },
        ConfigParameter {
            key: "performance_monitor.pressure_end_samples".to_owned(),
            value: FactScalar::Integer(i64::from(performance_monitor.pressure_end_samples())),
            source: explicit_or_default(inputs.pressure_end_samples_explicit),
        },
        ConfigParameter {
            key: "capacity_thresholds.hard_bytes".to_owned(),
            value: integer(capacity_thresholds.hard_bytes)?,
            source: explicit_or_default(inputs.capacity_thresholds_explicit),
        },
        ConfigParameter {
            key: "capacity_thresholds.soft_bytes".to_owned(),
            value: integer(capacity_thresholds.soft_bytes)?,
            source: explicit_or_default(inputs.capacity_thresholds_explicit),
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

/// "configured" for a present section, otherwise the absence reason.
const fn present(configured: bool, absent: &'static str) -> &'static str {
    if configured { "configured" } else { absent }
}

/// `env_overrides` (Workflow #318 cfg3): enabled only when the file allows the
/// `ACTINGCOMMAND_*` fallbacks. The reason names the flag's state and, when it is off,
/// every set variable as `env_override_ignored:<VAR>`: the startup record of the warnings
/// `check-config` prints.
fn env_overrides_subsystem(declared: Option<bool>, ignored: &[&str]) -> ConfigSubsystem {
    let state = match declared {
        Some(true) => "configured",
        Some(false) => "configured off",
        None => "flag absent",
    };
    let reason = if ignored.is_empty() {
        state.to_owned()
    } else {
        let warnings = ignored
            .iter()
            .map(|name| super::env_override_warning(name))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{state}; {warnings}")
    };
    ConfigSubsystem {
        name: "env_overrides".to_owned(),
        enabled: declared == Some(true),
        reason,
    }
}

/// The registry id's canonical text (`instance_<32 hex>`), its only public spelling.
fn instance_id_text(instance_id: InstanceId) -> Result<String, &'static str> {
    match serde_json::to_value(instance_id) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err("config_manifest_invalid"),
    }
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
