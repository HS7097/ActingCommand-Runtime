// SPDX-License-Identifier: AGPL-3.0-only

//! Builds the in-memory runtime configuration manifest from what `assemble` actually
//! applies: which subsystems the host runs and why, and every effective parameter value
//! with its source. Every value of the keys the assembly parses itself is read back from the
//! assembled `RuntimeHostConfig`; the configuration file only decides whether a value is
//! `explicit` or `default`. The keys the configuration mechanism resolves itself (Workflow #387
//! D1b: the per-instance section) come rendered from `actingcommand-runtime-config`, and every
//! row says whether an instruction may change it (`settable`, from the key table). Data only;
//! the host records it as the `config.subsystems` and `config.parameters.<section>` facts.

use crate::codes::ActingdCode;
use crate::monitor_retry::PolicyMonitorSettings;
use actingcommand_contract::outcome::{CauseRelation, Code, Outcome};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSubsystem, DeviceDiagnosticMode, FactScalar,
    InstanceId, RuntimeConfigManifest,
};
use actingcommand_device::{
    MUMU_MANAGER_CONTROL_TIMEOUT, MUMU_MANAGER_STATE_WAIT_START, MUMU_MANAGER_STATE_WAIT_STOP,
};
use actingcommand_runtime_host::{CatalogTransitionRequest, RuntimeHostConfig};
use std::path::Path;
use std::time::Duration;

/// Everything the manifest reports: the assembled host, whose effective values are read
/// back, and what the configuration file named, which decides each parameter's source.
pub(super) struct ManifestInputs<'a> {
    pub(super) host: &'a RuntimeHostConfig,
    pub(super) bind_port_explicit: bool,
    pub(super) device_diagnostic_mode_explicit: bool,
    /// The file's `frame_retention_enabled` as declared; the subsystem outcome names it.
    pub(super) frame_retention_enabled: Option<bool>,
    pub(super) failed_run_successes_explicit: bool,
    pub(super) failed_run_days_explicit: bool,
    /// Whether the file named `frame_retention_dedup_error` / `frame_retention_dedup_lab`.
    pub(super) dedup_error_explicit: bool,
    pub(super) dedup_lab_explicit: bool,
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
    /// The configured `policy.catalog_transition` (Workflow #361 A), reported only when set.
    pub(super) catalog_transition: Option<&'a CatalogTransitionRequest>,
    /// `policy.monitor` (Workflow #397), reported with an assembled `policy` section.
    pub(super) policy_monitor: Option<PolicyMonitorParameters>,
    pub(super) vision_provider_configured: bool,
    pub(super) instances_count: usize,
    pub(super) instances_deferred_count: usize,
    pub(super) instances_startup_package_count: usize,
    /// Workflow #387 D1b: the rows the configuration mechanism resolved (the per-instance
    /// section, every configured instance in declaration order).
    pub(super) mechanism_parameters: Vec<ConfigParameter>,
}

/// The effective `policy.monitor` settings (Workflow #397) and which of them the file named.
/// They are actingd's own, not the host's, so they are reported as `assemble` resolved them.
#[derive(Clone, Copy)]
pub(super) struct PolicyMonitorParameters {
    pub(super) settings: PolicyMonitorSettings,
    pub(super) io_timeout_explicit: bool,
    pub(super) retry_limit_explicit: bool,
    pub(super) retry_backoff_explicit: bool,
    pub(super) retry_backoff_max_explicit: bool,
}

pub(super) fn build(inputs: &ManifestInputs<'_>) -> Result<RuntimeConfigManifest, Code> {
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
        .ok_or(ActingdCode::ConfigManifestIncomplete.code())?;
    let discovery_bound = inputs.instances_deferred_count > 0;
    // Workflow #318 cfg4: the effective allow-list read back from the host; `any` when the
    // host accepts every well-formed card.
    let governance_allowed_clients = host
        .governance_policy()
        .allowed_clients
        .as_ref()
        .map(|allowed| allowed.iter().map(String::as_str).collect::<Vec<_>>());
    // Ledger revision 3: each subsystem says why it is on or off by a registered code; the
    // values the former prose reason carried are kept where the key table has a key for them
    // (`count`), and the rest are the manifest's parameters.
    let governance = match &governance_allowed_clients {
        Some(allowed) => Outcome::new(ActingdCode::ConfigSubsystemAlwaysOn).count(allowed.len()),
        None => Outcome::new(ActingdCode::ConfigSubsystemAlwaysOn),
    };
    let discovery = || {
        if discovery_bound {
            Outcome::new(ActingdCode::ConfigSubsystemDiscoveryBound)
                .count(inputs.instances_deferred_count)
        } else {
            Outcome::new(ActingdCode::ConfigSubsystemDiscoveryUnbound)
        }
    };
    let subsystems = vec![
        subsystem(
            "frame_retention",
            frame_retention_enabled,
            Outcome::new(flag_state(inputs.frame_retention_enabled)),
        ),
        subsystem(
            "agent_dispatcher",
            inputs.agent_dispatcher.is_some(),
            Outcome::new(present(inputs.agent_dispatcher.is_some())),
        ),
        subsystem("governance", true, governance),
        subsystem(
            "policy_driver",
            inputs.policy_configured,
            Outcome::new(present(inputs.policy_configured)),
        ),
        subsystem(
            "vision_provider",
            inputs.vision_provider_configured,
            Outcome::new(present(inputs.vision_provider_configured)),
        ),
        subsystem(
            "device_diagnostic",
            true,
            Outcome::new(ActingdCode::ConfigSubsystemAlwaysOn),
        ),
        subsystem(
            "performance_monitor",
            true,
            Outcome::new(ActingdCode::ConfigSubsystemAlwaysOn),
        ),
        subsystem("mumu_discovery", discovery_bound, discovery()),
        subsystem("emulator_control", discovery_bound, discovery()),
        subsystem(
            "runtime_fact_snapshot",
            true,
            Outcome::new(ActingdCode::ConfigSubsystemAlwaysOn),
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
            settable: false,
        },
        ConfigParameter {
            key: "device_diagnostic_mode".to_owned(),
            value: FactScalar::String(
                device_diagnostic_mode_name(device_diagnostic_mode).to_owned(),
            ),
            source: explicit_or_default(inputs.device_diagnostic_mode_explicit),
            settable: false,
        },
        ConfigParameter {
            key: "frame_retention_enabled".to_owned(),
            value: FactScalar::Boolean(frame_retention_enabled),
            source: explicit_or_default(inputs.frame_retention_enabled.is_some()),
            settable: false,
        },
        ConfigParameter {
            key: "frame_retention_failed_run_successes".to_owned(),
            value: FactScalar::Integer(i64::from(failed_run_retention.successor_successes)),
            source: explicit_or_default(inputs.failed_run_successes_explicit),
            settable: false,
        },
        ConfigParameter {
            key: "frame_retention_failed_run_days".to_owned(),
            value: FactScalar::Integer(i64::from(failed_run_retention.retention_days)),
            source: explicit_or_default(inputs.failed_run_days_explicit),
            settable: false,
        },
        ConfigParameter {
            key: "frame_retention_dedup_error".to_owned(),
            value: FactScalar::Boolean(host.frame_retention_dedup_error()),
            source: explicit_or_default(inputs.dedup_error_explicit),
            settable: false,
        },
        ConfigParameter {
            key: "frame_retention_dedup_lab".to_owned(),
            value: FactScalar::Boolean(host.frame_retention_dedup_lab()),
            source: explicit_or_default(inputs.dedup_lab_explicit),
            settable: false,
        },
        explicit(
            "secret_fingerprint_salt_bytes",
            integer(inputs.secret_fingerprint_salt_bytes)?,
        ),
        ConfigParameter {
            key: "allow_env_overrides".to_owned(),
            value: FactScalar::Boolean(inputs.allow_env_overrides.unwrap_or(false)),
            source: explicit_or_default(inputs.allow_env_overrides.is_some()),
            settable: false,
        },
        ConfigParameter {
            key: "governance.allowed_clients".to_owned(),
            value: FactScalar::String(
                governance_allowed_clients
                    .map_or_else(|| "any".to_owned(), |allowed| allowed.join(",")),
            ),
            source: explicit_or_default(inputs.governance_allowed_clients_explicit),
            settable: false,
        },
    ];
    if let Some(transition) = inputs.catalog_transition {
        parameters.extend([
            explicit(
                "policy.catalog_transition.kind",
                FactScalar::String("replace".to_owned()),
            ),
            explicit(
                "policy.catalog_transition.expected_active_catalog_hash",
                FactScalar::String(transition.expected_active_catalog_hash().to_owned()),
            ),
        ]);
    }
    if let Some(monitor) = inputs.policy_monitor {
        let settings = monitor.settings;
        parameters.extend([
            ConfigParameter {
                key: "policy.monitor.io_timeout_ms".to_owned(),
                value: duration_ms(settings.io_timeout)?,
                source: explicit_or_default(monitor.io_timeout_explicit),
                settable: false,
            },
            ConfigParameter {
                key: "policy.monitor.retry_limit".to_owned(),
                value: FactScalar::Integer(i64::from(settings.retry_limit)),
                source: explicit_or_default(monitor.retry_limit_explicit),
                settable: false,
            },
            ConfigParameter {
                key: "policy.monitor.retry_backoff_ms".to_owned(),
                value: duration_ms(settings.retry_backoff)?,
                source: explicit_or_default(monitor.retry_backoff_explicit),
                settable: false,
            },
            ConfigParameter {
                key: "policy.monitor.retry_backoff_max_ms".to_owned(),
                value: duration_ms(settings.retry_backoff_max)?,
                source: explicit_or_default(monitor.retry_backoff_max_explicit),
                settable: false,
            },
        ]);
    }
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
    // registry id (an alias may exceed the 128-byte key bound); since Workflow #387 D1b the
    // per-instance section the configuration mechanism resolved.
    parameters.extend(inputs.mechanism_parameters.iter().cloned());
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
            settable: false,
        },
        ConfigParameter {
            key: "performance_monitor.pressure_end_samples".to_owned(),
            value: FactScalar::Integer(i64::from(performance_monitor.pressure_end_samples())),
            source: explicit_or_default(inputs.pressure_end_samples_explicit),
            settable: false,
        },
        ConfigParameter {
            key: "capacity_thresholds.hard_bytes".to_owned(),
            value: integer(capacity_thresholds.hard_bytes)?,
            source: explicit_or_default(inputs.capacity_thresholds_explicit),
            settable: false,
        },
        ConfigParameter {
            key: "capacity_thresholds.soft_bytes".to_owned(),
            value: integer(capacity_thresholds.soft_bytes)?,
            source: explicit_or_default(inputs.capacity_thresholds_explicit),
            settable: false,
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
    // Workflow #387 D1b: every key is in the key table, and says whether it is settable.
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .map_err(|failure| failure.failure_code().code())?;
    let manifest = RuntimeConfigManifest {
        subsystems,
        parameters,
    };
    manifest
        .validate()
        .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?;
    Ok(manifest)
}

fn subsystem(name: &str, enabled: bool, outcome: Outcome) -> ConfigSubsystem {
    ConfigSubsystem {
        name: name.to_owned(),
        enabled,
        outcome,
    }
}

/// `config_subsystem_configured` for a present section, else `config_subsystem_section_absent`.
const fn present(configured: bool) -> ActingdCode {
    if configured {
        ActingdCode::ConfigSubsystemConfigured
    } else {
        ActingdCode::ConfigSubsystemSectionAbsent
    }
}

/// The state of a subsystem's flag as the file declares it.
const fn flag_state(declared: Option<bool>) -> ActingdCode {
    match declared {
        Some(true) => ActingdCode::ConfigSubsystemConfigured,
        Some(false) => ActingdCode::ConfigSubsystemConfiguredOff,
        None => ActingdCode::ConfigSubsystemFlagAbsent,
    }
}

/// `env_overrides` (Workflow #318 cfg3): enabled only when the file allows the
/// `ACTINGCOMMAND_*` fallbacks. The outcome names the flag's state and, when it is off,
/// carries every set variable as a `note` link of `env_override_ignored`: the startup record
/// of the warnings `check-config` prints. The `config.subsystems` row names those variables
/// under `ignored_variables`, so an ignored override is never silent.
pub(crate) fn env_overrides_subsystem(declared: Option<bool>, ignored: &[&str]) -> ConfigSubsystem {
    let outcome = super::env_override_warnings(ignored)
        .into_iter()
        .fold(Outcome::new(flag_state(declared)), |outcome, warning| {
            outcome.link(CauseRelation::Note, warning)
        });
    subsystem("env_overrides", declared == Some(true), outcome)
}

/// The registry id's canonical text (`instance_<32 hex>`), its only public spelling.
pub(super) fn instance_id_text(instance_id: InstanceId) -> Result<String, Code> {
    match serde_json::to_value(instance_id) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(ActingdCode::ConfigManifestInvalid.code()),
    }
}

fn explicit(key: &str, value: FactScalar) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source: ConfigParameterSource::Explicit,
        settable: false,
    }
}

fn default(key: &str, value: FactScalar) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source: ConfigParameterSource::Default,
        settable: false,
    }
}

fn integer(value: impl TryInto<i64>) -> Result<FactScalar, Code> {
    value
        .try_into()
        .map(FactScalar::Integer)
        .map_err(|_| ActingdCode::ConfigManifestValueOutOfRange.code())
}

fn duration_ms(value: Duration) -> Result<FactScalar, Code> {
    u64::try_from(value.as_millis())
        .map(FactScalar::DurationMs)
        .map_err(|_| ActingdCode::ConfigManifestValueOutOfRange.code())
}

/// The wire spelling of the mode (`serde` snake case); a closed match so a new mode
/// cannot be reported under a stale name.
const fn device_diagnostic_mode_name(mode: DeviceDiagnosticMode) -> &'static str {
    match mode {
        DeviceDiagnosticMode::Shadow => "shadow",
    }
}
