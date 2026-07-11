// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, current_unix_ms, monitor_policy_monitor_args,
    parse_optional_duration_ms, runtime_slice_cli, runtime_state_root,
};
use actingcommand_contract::{
    EventActor, EventSource, RUNTIME_INFO_FILE, RuntimeControlPlaneStatus, RuntimeInfo,
    RuntimeMonitorInstanceStatus, RuntimeMonitorPolicy, RuntimeMonitorRegistryStatus,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn run_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let _ = global;
    reject_legacy_flags(&flags)?;
    flags.expect_positionals("session status", 0)?;
    runtime_status(&runtime_state_root()?, flags.bool("--diagnostics"))
}

pub(super) fn run_monitor_policy(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session monitor-policy requires status|set|clear"))?;
    let flags = FlagArgs::parse(&args[1..])?;
    let _ = global;
    reject_legacy_flags(&flags)?;
    let state_root = runtime_state_root()?;
    let requested = flags
        .optional("--instance")
        .filter(|value| value != "true")
        .or_else(|| global.instance.clone());
    let instance = resolve_instance_alias(&state_root, requested.as_deref())?;
    match action {
        "status" => {
            flags.expect_positionals("session monitor-policy status", 0)?;
            runtime_monitor_status(&state_root, &instance)
        }
        "set" => {
            flags.expect_positionals("session monitor-policy set", 0)?;
            let interval = parse_optional_duration_ms(&flags, "--interval-ms", 30_000)?;
            let interval_ms = u64::try_from(interval.as_millis())
                .map_err(|_| CliError::usage("--interval-ms is too large"))?;
            let _ = monitor_policy_monitor_args(&args[1..], &flags)?;
            let expected_page = flags
                .optional("--expect")
                .or_else(|| flags.optional("--to"))
                .filter(|value| value != "true")
                .unwrap_or_else(|| "home".to_string());
            let policy =
                RuntimeMonitorPolicy::new(interval_ms, expected_page, flags.bool("--recover"))
                    .map_err(|error| CliError::usage(error.to_string()))?;
            configure_monitor(&state_root, &instance, policy)
        }
        "clear" => {
            flags.expect_positionals("session monitor-policy clear", 0)?;
            clear_monitor(&state_root, &instance)
        }
        other => Err(CliError::usage(format!(
            "unknown session monitor-policy action: {other}"
        ))),
    }
}

pub(super) fn retired_authority(subcommand: &str, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_flags(&flags)?;
    Err(CliError::not_implemented(
        "legacy_session_authority_retired",
        format!(
            "session {subcommand} belonged to the retired Session file-state authority; use Runtime-backed status, monitor-policy, stream, or actingctl"
        ),
    ))
}

fn reject_legacy_flags(flags: &FlagArgs) -> CliOutcome<()> {
    if flags.bool("--via-daemon")
        || flags.bool("--local")
        || flags.optional("--state-dir").is_some()
    {
        return Err(CliError::not_implemented(
            "legacy_session_authority_retired",
            "legacy Session daemon and file-state routing were retired; use the resident Runtime",
        ));
    }
    Ok(())
}

fn runtime_status(state_root: &Path, diagnostics: bool) -> CliOutcome<Value> {
    let client = connect(state_root)?;
    let health = client
        .health()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let control = client
        .status()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let monitor = client
        .monitor_status()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    if health != client.runtime_info().owner_epoch()
        || health != control.owner_epoch()
        || health != monitor.owner_epoch()
    {
        return Err(CliError::device(
            "Runtime owner epoch changed during session status projection",
        ));
    }
    project_status(
        client.runtime_info(),
        &control,
        &monitor,
        state_root,
        diagnostics,
    )
}

fn runtime_monitor_status(state_root: &Path, instance_alias: &str) -> CliOutcome<Value> {
    let client = connect(state_root)?;
    let registry = client
        .monitor_status()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let status = find_monitor_status(&registry, instance_alias)?;
    project_monitor_status(state_root, status)
}

pub(super) fn resolve_instance_alias(
    state_root: &Path,
    requested: Option<&str>,
) -> CliOutcome<String> {
    if let Some(requested) = requested.filter(|value| !value.trim().is_empty()) {
        return Ok(requested.to_string());
    }
    let client = connect(state_root)?;
    let status = client
        .status()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    match status.instances() {
        [instance] => Ok(instance.instance_alias().to_string()),
        [] => Err(CliError::device("Runtime has no registered instances")),
        _ => Err(CliError::usage(
            "multiple Runtime instances are registered; pass --instance <id>",
        )),
    }
}

pub(super) fn configure_monitor(
    state_root: &Path,
    instance_alias: &str,
    policy: RuntimeMonitorPolicy,
) -> CliOutcome<Value> {
    let client = connect(state_root)?;
    let status = client
        .configure_monitor(instance_alias, policy)
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let projected = project_monitor_instance(&status)?;
    Ok(json!({
        "status": "configured",
        "mode": "session_monitor_policy",
        "daemon_owned": true,
        "state_dir": state_root.display().to_string(),
        "policy_path": state_root.join("monitor.journal").display().to_string(),
        "state_path": state_root.join("monitor.journal").display().to_string(),
        "policy": projected["policy"],
        "state": projected["state"]
    }))
}

pub(super) fn clear_monitor(state_root: &Path, instance_alias: &str) -> CliOutcome<Value> {
    let client = connect(state_root)?;
    let before = client
        .monitor_status()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let existed = find_monitor_status(&before, instance_alias)?
        .policy()
        .is_some();
    let cleared = client
        .clear_monitor(instance_alias)
        .map_err(runtime_slice_cli::map_runtime_error)?;
    if cleared.policy().is_some() || cleared.state().is_some() {
        return Err(CliError::device(
            "Runtime monitor clear returned configured state",
        ));
    }
    Ok(json!({
        "status": "cleared",
        "mode": "session_monitor_policy",
        "daemon_owned": true,
        "state_dir": state_root.display().to_string(),
        "policy_path": state_root.join("monitor.journal").display().to_string(),
        "policy_existed": existed,
        "state_preserved": false
    }))
}

fn connect(state_root: &Path) -> CliOutcome<RuntimeClient> {
    RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(runtime_slice_cli::map_runtime_error)
}

fn project_status(
    info: &RuntimeInfo,
    control: &RuntimeControlPlaneStatus,
    monitor: &RuntimeMonitorRegistryStatus,
    state_root: &Path,
    diagnostics: bool,
) -> CliOutcome<Value> {
    let now = current_unix_ms();
    let owner_epoch = to_value(info.owner_epoch(), "Runtime owner epoch")?;
    let endpoint = info
        .socket_addr()
        .map_err(|_| CliError::device("Runtime info contains an invalid endpoint"))?
        .to_string();
    let session_info = json!({
        "pid": info.pid(),
        "daemon_id": owner_epoch,
        "daemon_liveness_endpoint": endpoint,
        "process_creation_key": Value::Null,
        "started_at_unix_ms": info.started_at_unix_ms(),
        "state_dir": state_root.display().to_string(),
        "runtime_version": "actingcommand-runtime"
    });
    let heartbeat = json!({
        "pid": info.pid(),
        "daemon_id": owner_epoch,
        "daemon_liveness_endpoint": endpoint,
        "process_creation_key": Value::Null,
        "updated_at_unix_ms": now,
        "state": "running"
    });
    let mut output = json!({
        "state_dir": state_root.display().to_string(),
        "running": true,
        "info": session_info,
        "heartbeat": heartbeat
    });
    if diagnostics {
        output["diagnostics"] = project_diagnostics(control, monitor, state_root, now)?;
    }
    Ok(output)
}

fn project_diagnostics(
    control: &RuntimeControlPlaneStatus,
    monitor: &RuntimeMonitorRegistryStatus,
    state_root: &Path,
    now: u64,
) -> CliOutcome<Value> {
    let pending = control
        .instances()
        .iter()
        .map(|instance| u64::from(instance.queued_request_count()))
        .sum::<u64>();
    let active_leases = control
        .instances()
        .iter()
        .filter(|instance| instance.lease_active())
        .count();
    Ok(json!({
        "liveness": {
            "status": "alive",
            "info_present": true,
            "heartbeat_present": true,
            "process_alive": true,
            "heartbeat_state": "running",
            "heartbeat_updated_at_unix_ms": now,
            "heartbeat_age_ms": 0,
            "can_accept_requests": true,
            "authority": "runtime"
        },
        "recommended_actions": [],
        "paths": {
            "info": state_root.join(RUNTIME_INFO_FILE).display().to_string(),
            "heartbeat": Value::Null,
            "requests": Value::Null,
            "running": Value::Null,
            "responses": Value::Null,
            "journal": state_root.join("ledger").display().to_string()
        },
        "queues": {
            "pending_requests": pending,
            "running_requests": active_leases,
            "pending_responses": 0,
            "health": {"status": "runtime_owned"},
            "pending_request_preview": [],
            "running_request_preview": [],
            "pending_response_preview": []
        },
        "instances": to_value(control, "Runtime control-plane status")?,
        "leases": {
            "source": "runtime_scheduler",
            "active_count": active_leases
        },
        "monitor_policy": project_monitor_registry(monitor)?,
        "capture_freshness": {
            "status": "not_projected",
            "source": "runtime_artifact_ledger",
            "message": "capture freshness is available through Runtime events"
        },
        "self_heal": {"status": "runtime_coordinated"},
        "interaction_flow": {"status": "runtime_owned"},
        "trusted_channel": {"status": "reserved"},
        "phase_c": {"status": "runtime_owned"},
        "validation": {"status": "runtime_connected"},
        "journal": {
            "source": "runtime_global_ledger",
            "exists": state_root.join("ledger").exists(),
            "path": state_root.join("ledger").display().to_string(),
            "ledger": state_root.join("ledger").display().to_string(),
            "skipped_corrupt_lines": 0,
            "projected_from_legacy": 0
        }
    }))
}

fn project_monitor_status(
    state_root: &Path,
    status: &RuntimeMonitorInstanceStatus,
) -> CliOutcome<Value> {
    let projected = project_monitor_instance(status)?;
    Ok(json!({
        "schema_version": "session.monitor_policy_status.v0.1",
        "state_dir": state_root.display().to_string(),
        "policy_path": state_root.join("monitor.journal").display().to_string(),
        "state_path": state_root.join("monitor.journal").display().to_string(),
        "configured": status.policy().is_some(),
        "policy": projected["policy"],
        "state": projected["state"],
        "execution_model": {
            "daemon_owned": true,
            "read_only": status.policy().is_none_or(|policy| !policy.recovery_enabled()),
            "runs_monitor_once": true,
            "recover_enabled": status.policy().is_some_and(RuntimeMonitorPolicy::recovery_enabled),
            "recovery_requires_matching_lease": false,
            "recovery_without_matching_lease_status": "scheduler_coordinated",
            "executes_input": false,
            "executes_app_restart": false
        }
    }))
}

fn project_monitor_registry(registry: &RuntimeMonitorRegistryStatus) -> CliOutcome<Value> {
    let instances = registry
        .instances()
        .iter()
        .map(project_monitor_instance)
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok(json!({
        "schema_version": "session.monitor_policy_status.v0.1",
        "owner_epoch": to_value(registry.owner_epoch(), "monitor owner epoch")?,
        "authority": "runtime",
        "instances": instances
    }))
}

fn project_monitor_instance(status: &RuntimeMonitorInstanceStatus) -> CliOutcome<Value> {
    let policy = status.policy().map(|policy| {
        json!({
            "schema_version": "session.monitor_policy.v0.1",
            "enabled": true,
            "interval_ms": policy.interval_ms(),
            "global": {
                "instance": status.instance_alias(),
                "game": Value::Null,
                "server": Value::Null,
                "resource_root": Value::Null,
                "capture_backend": "runtime_owned",
                "touch_backend": Value::Null,
                "dry_run": false
            },
            "args": ["--expect", policy.expected_page()],
            "read_only": !policy.recovery_enabled(),
            "recover_enabled": policy.recovery_enabled(),
            "lease": Value::Null,
            "created_at_unix_ms": Value::Null,
            "updated_at_unix_ms": Value::Null,
            "runtime_policy": policy
        })
    });
    let state = status.state().map(|state| {
        let last_result = state
            .last_decision()
            .map(|decision| to_value(decision, "monitor decision"))
            .transpose()?;
        let last_error = state
            .last_error()
            .map(|error| to_value(error, "monitor error"))
            .transpose()?;
        Ok::<Value, CliError>(json!({
            "schema_version": "session.monitor_state.v0.1",
            "policy_updated_at_unix_ms": Value::Null,
            "next_due_unix_ms": state.next_due_unix_ms(),
            "run_count": state.run_count(),
            "last_started_at_unix_ms": state.last_started_at_unix_ms(),
            "last_completed_at_unix_ms": state.last_completed_at_unix_ms(),
            "last_ok": if state.run_count() == 0 { Value::Null } else { Value::Bool(last_error.is_none()) },
            "last_status": if last_error.is_some() { "failed" } else if last_result.is_some() { "completed" } else { "pending" },
            "last_result": last_result,
            "last_error": last_error,
            "last_recovery": Value::Null,
            "last_recovery_error": Value::Null,
            "runtime_state": state
        }))
    }).transpose()?;
    Ok(json!({
        "instance_alias": status.instance_alias(),
        "policy": policy,
        "state": state
    }))
}

fn find_monitor_status<'a>(
    registry: &'a RuntimeMonitorRegistryStatus,
    instance_alias: &str,
) -> CliOutcome<&'a RuntimeMonitorInstanceStatus> {
    registry
        .instances()
        .iter()
        .find(|status| status.instance_alias() == instance_alias)
        .ok_or_else(|| {
            CliError::device(format!(
                "Runtime instance is not registered: {instance_alias}"
            ))
        })
}

fn to_value(value: impl serde::Serialize, label: &str) -> CliOutcome<Value> {
    serde_json::to_value(value)
        .map_err(|error| CliError::device(format!("failed to serialize {label}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{IdentifierIssuer, RuntimeInstanceStatus, RuntimeMonitorState};
    use std::path::PathBuf;

    #[test]
    fn status_projection_preserves_public_fields_and_uses_runtime_authority() {
        let ids = IdentifierIssuer::new().expect("ids");
        let owner_epoch = *ids.mint_owner_epoch().expect("owner").transport();
        let instance_id = *ids.mint_instance_id().expect("instance").transport();
        let info = RuntimeInfo::new(42, "127.0.0.1", 12345, owner_epoch, 100).expect("info");
        let control = RuntimeControlPlaneStatus::new(
            owner_epoch,
            vec![
                RuntimeInstanceStatus::new("ak.cn", instance_id, false, 2, false, false, false)
                    .expect("status"),
            ],
        )
        .expect("control");
        let policy = RuntimeMonitorPolicy::new(1_000, "home", false).expect("policy");
        let monitor = RuntimeMonitorRegistryStatus::new(
            owner_epoch,
            vec![
                RuntimeMonitorInstanceStatus::configured(
                    "ak.cn",
                    policy,
                    RuntimeMonitorState::scheduled(10).expect("state"),
                )
                .expect("monitor"),
            ],
        )
        .expect("registry");
        let root = PathBuf::from("runtime-state");

        let projected = project_status(&info, &control, &monitor, &root, true).expect("projection");

        assert_eq!(projected["running"], true);
        assert_eq!(projected["info"]["pid"], 42);
        assert_eq!(projected["heartbeat"]["state"], "running");
        assert_eq!(projected["diagnostics"]["liveness"]["authority"], "runtime");
        assert_eq!(projected["diagnostics"]["queues"]["pending_requests"], 2);
        assert_eq!(
            projected["diagnostics"]["monitor_policy"]["instances"][0]["policy"]["runtime_policy"]
                ["expected_page"],
            "home"
        );
    }
}
