use crate::{
    CliError, CliOutcome, GlobalOptions, REQUIRE_SESSION_DAEMON_ENV,
    TRUSTED_REMOTE_CLIENT_CERT_ENV, TRUSTED_REMOTE_TOKEN_ENV, effective_resource_root,
    exit_code_table, find_files, lab2_cli, package_cli, read_user_config, runtime_debug,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(crate) fn command_capabilities() -> Vec<Value> {
    let mut commands = vec![
        command_cap("version", ["offline"], "available"),
        command_cap("doctor", ["offline"], "available"),
        command_cap("paths", ["offline"], "available"),
        command_cap("config get", ["offline"], "available"),
        command_cap("config set", ["offline"], "available"),
        command_cap("schema", ["offline"], "available"),
        command_cap("list", ["offline"], "available"),
        command_cap("capabilities", ["offline"], "available"),
        command_cap("resource validate", ["offline"], "available"),
        command_cap("resource convert", ["offline"], "available"),
        command_cap("resource restore", ["offline"], "available"),
        command_cap("scheduling compile", ["offline", "read_only"], "available"),
        command_cap("scheduling timeline", ["offline", "read_only"], "available"),
        command_cap("resource compile-maa", ["offline"], "available"),
        command_cap("resource import-alas", ["offline"], "reserved"),
        command_cap("resource drift-alas", ["offline"], "reserved"),
        command_cap("resource check-release", ["offline"], "available"),
        command_cap("observe", ["offline", "device_optional"], "available"),
        command_cap(
            "do",
            ["offline", "device_optional", "lab_lease"],
            "available",
        ),
        command_cap(
            "ensure",
            ["offline", "device_optional", "lab_lease"],
            "available",
        ),
        command_cap("wait", ["offline", "device_optional"], "available"),
        command_cap("package validate", ["offline"], "available"),
        command_cap("package inspect", ["offline"], "available"),
        package_cli::offline_capability(),
        command_cap("package build-task", ["offline"], "available"),
        command_cap("package build-pack", ["offline"], "available"),
        command_cap("ledger show", ["offline", "read_only"], "retired"),
        command_cap("ledger events", ["offline", "read_only"], "retired"),
        command_cap("ledger receipts", ["offline", "read_only"], "retired"),
        command_cap("ledger diagnose", ["offline", "read_only"], "retired"),
        command_cap("ledger evidence", ["offline", "read_only"], "retired"),
        command_cap("operation validate", ["offline"], "available"),
        command_cap("operation inspect", ["offline"], "available"),
        command_cap("operation explain", ["offline"], "available"),
        command_cap("status", ["running_runtime"], "available"),
        command_cap("run summary", ["running_runtime", "read_only"], "available"),
        command_cap("devices", ["device"], "retired"),
        command_cap("touch-probe", ["device"], "available"),
        command_cap("tap", ["device"], "available"),
        command_cap("swipe", ["device"], "available"),
        command_cap("long-tap", ["device"], "available"),
        command_cap("key", ["device"], "available"),
        command_cap("text", ["device"], "available"),
        command_cap("session status", ["running_runtime"], "available"),
        command_cap("session bootstrap", ["offline"], "retired"),
        command_cap("session throat-policy", ["offline"], "available"),
        command_cap("session capture-policy", ["offline"], "available"),
        command_cap("session record-policy", ["offline"], "available"),
        command_cap("session self-heal-policy", ["offline"], "available"),
        command_cap("session self-heal-plan", ["offline"], "retired"),
        command_cap("session phase-c-plan", ["offline"], "retired"),
        command_cap("session readiness", ["offline"], "retired"),
        command_cap("session connect-plan", ["offline"], "retired"),
        command_cap("session stream-plan", ["offline"], "retired"),
        command_cap("session queue", ["offline"], "retired"),
        command_cap("session command-check", ["offline"], "retired"),
        command_cap("session submit-plan", ["offline"], "retired"),
        command_cap("session validation-plan", ["offline"], "retired"),
        command_cap("session start", ["offline"], "retired"),
        command_cap("session stop", ["offline"], "retired"),
        command_cap("session cleanup", ["offline"], "retired"),
        command_cap("session journal", ["offline"], "retired"),
        command_cap("session events", ["offline"], "retired"),
        command_cap("session events wait", ["offline"], "retired"),
        command_cap("session response", ["offline"], "retired"),
        command_cap("session response get", ["offline"], "retired"),
        command_cap("session response wait", ["offline"], "retired"),
        command_cap("session request-state", ["offline"], "retired"),
        command_cap("session request-state get", ["offline"], "retired"),
        command_cap("session request-state wait", ["offline"], "retired"),
        command_cap("session request-state list", ["offline"], "retired"),
        command_cap("session contract", ["offline"], "available"),
        command_cap("session api", ["offline"], "available"),
        command_cap("session transport", ["offline"], "available"),
        command_cap("session transport plan", ["offline"], "available"),
        command_cap("session transport check", ["offline"], "available"),
        command_cap("session stream", ["running_runtime", "device"], "available"),
        command_cap("session stream check", ["offline"], "available"),
        command_cap("session monitor-policy", ["running_runtime"], "available"),
        command_cap("session request cancel", ["offline"], "retired"),
        command_cap("session request status", ["running_runtime"], "retired"),
        command_cap("session request bootstrap", ["running_runtime"], "retired"),
        command_cap(
            "session request throat-policy",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request capture-policy",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request record-policy",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request self-heal-policy",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request self-heal-plan",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request phase-c-plan",
            ["running_runtime"],
            "retired",
        ),
        command_cap("session request readiness", ["running_runtime"], "retired"),
        command_cap(
            "session request connect-plan",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request stream-plan",
            ["running_runtime"],
            "retired",
        ),
        command_cap("session request queue", ["running_runtime"], "retired"),
        command_cap(
            "session request command-check",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request submit-plan",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request validation-plan",
            ["running_runtime"],
            "retired",
        ),
        command_cap("session request --no-wait", ["running_runtime"], "retired"),
        command_cap("session request journal", ["running_runtime"], "retired"),
        command_cap("session request events", ["running_runtime"], "retired"),
        command_cap(
            "session request events wait",
            ["running_runtime"],
            "retired",
        ),
        command_cap("session request response", ["running_runtime"], "retired"),
        command_cap(
            "session request response get",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request response wait",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request request-state",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request request-state get",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request request-state wait",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request request-state list",
            ["running_runtime"],
            "retired",
        ),
        command_cap("session request contract", ["running_runtime"], "retired"),
        command_cap("session request api", ["running_runtime"], "retired"),
        command_cap("session request transport", ["running_runtime"], "retired"),
        command_cap(
            "session request transport plan",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request transport check",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request capabilities",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request monitor-policy",
            ["running_runtime"],
            "retired",
        ),
        command_cap("session request devices", ["running_runtime"], "retired"),
        command_cap("session request record", ["running_runtime"], "retired"),
        command_cap(
            "session request capture",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request capture-diagnose",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request stream",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request stream check",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request recognize",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request detect-page",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request current-page",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request is-visible",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request locate",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request monitor",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request monitor-once",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap(
            "session request instance list",
            ["running_runtime"],
            "retired",
        ),
        command_cap(
            "session request instance registry",
            ["running_runtime"],
            "retired",
        ),
        command_cap("session request instance health", ["offline"], "retired"),
        command_cap(
            "session request instance keep-alive",
            ["offline"],
            "retired",
        ),
        command_cap("session request instance connect", ["offline"], "retired"),
        command_cap("session request instance reconnect", ["offline"], "retired"),
        command_cap(
            "session request instance app",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request app",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request lab-run",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request package-run",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request operation-run",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request tap",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request swipe",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request long-tap",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request key",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request text",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request tap-target",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request navigate",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request recover",
            ["running_runtime", "device", "lab_lease"],
            "retired",
        ),
        command_cap(
            "session request recover --stale-capture",
            ["running_runtime", "device"],
            "retired",
        ),
        command_cap("session instance", ["offline", "device"], "available"),
        command_cap("session instance list", ["offline"], "available"),
        command_cap("session instance registry", ["offline"], "available"),
        command_cap("session instance health", ["offline"], "retired"),
        command_cap("session instance keep-alive", ["offline"], "retired"),
        command_cap("session instance connect", ["offline"], "retired"),
        command_cap("session instance reconnect", ["offline"], "retired"),
        command_cap("session instance app", ["device"], "available"),
        command_cap("session instance app launch", ["device"], "available"),
        command_cap("session instance app stop", ["device"], "available"),
        command_cap("session instance app force-stop", ["device"], "available"),
        command_cap("session instance app restart", ["device"], "available"),
        command_cap("session app", ["device"], "available"),
        command_cap("session app launch", ["device"], "available"),
        command_cap("session app stop", ["device"], "available"),
        command_cap("session app force-stop", ["device"], "available"),
        command_cap("session app restart", ["device"], "available"),
        command_cap("session capture", ["device"], "available"),
        command_cap("session capture diagnose", ["device"], "available"),
        command_cap("session recover", ["device"], "available"),
        command_cap("session recover --stale-capture", ["device"], "available"),
        command_cap(
            "session lease run",
            ["running_runtime", "lab_lease"],
            "retired",
        ),
        command_cap("session record", ["offline"], "available"),
        command_cap("session record start", ["offline"], "available"),
        command_cap("session record status", ["offline"], "available"),
        command_cap("session record stop", ["offline"], "available"),
        command_cap("session record step", ["offline", "device"], "available"),
        command_cap("session record candidates", ["offline"], "available"),
        command_cap("session record amend", ["offline"], "available"),
        command_cap("session record build-task", ["offline"], "available"),
        command_cap("session record promote", ["offline"], "available"),
        command_cap("record", ["offline"], "available"),
        command_cap("record start", ["offline"], "available"),
        command_cap("record status", ["offline"], "available"),
        command_cap("record stop", ["offline"], "available"),
        command_cap("record step", ["offline", "device"], "available"),
        command_cap("record candidates", ["offline"], "available"),
        command_cap("record amend", ["offline"], "available"),
        command_cap("record build-task", ["offline"], "available"),
        command_cap("record promote", ["offline"], "available"),
        command_cap("current-page", ["device"], "available"),
        command_cap("is-visible", ["device"], "available"),
        command_cap("locate", ["device"], "available"),
        command_cap("tap-target", ["device"], "available"),
        command_cap("navigate", ["device"], "available"),
        command_cap("monitor --once", ["device"], "retired"),
        command_cap("monitor", ["device"], "retired"),
        command_cap("stream", ["device"], "available"),
        command_cap("scheduler status", ["running_runtime"], "reserved"),
        command_cap("scheduler pause", ["running_runtime"], "reserved"),
        command_cap("scheduler resume", ["running_runtime"], "reserved"),
        command_cap("scheduler start", ["running_runtime"], "reserved"),
        command_cap("scheduler stop", ["running_runtime"], "reserved"),
        command_cap("lab validate", ["offline"], "available"),
        command_cap("lab run", ["device"], "available"),
        command_cap("capture", ["device"], "available"),
        command_cap("capture diagnose", ["device"], "available"),
        command_cap("detect", ["offline"], "available"),
        command_cap("env resolve", ["offline"], "available"),
        command_cap("env status", ["offline"], "available"),
        command_cap("detect-page", ["device"], "available"),
        command_cap("recognize", ["device"], "available"),
        command_cap("recognize-artifact", ["running_runtime"], "available"),
        command_cap(
            "operation dry-run",
            ["running_runtime", "device"],
            "reserved",
        ),
        command_cap(
            "operation run",
            ["running_runtime", "device", "lab_lease"],
            "unavailable",
        ),
        command_cap(
            "control probe-click",
            ["running_runtime", "device", "lab_lease"],
            "unavailable",
        ),
        command_cap(
            "package run",
            ["running_runtime", "device", "lab_lease"],
            "unavailable",
        ),
    ];
    commands.extend(runtime_debug::capabilities());
    commands
}

pub(crate) fn command_cap<I>(command: &str, needs: I, status: &str) -> Value
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let needs = needs.into_iter().map(Into::into).collect::<Vec<String>>();
    let (status, reason_code) = match status {
        "retired" if command.starts_with("ledger ") => ("retired", "local_ledger_retired"),
        "retired" if command == "devices" || command.starts_with("session instance ") => {
            ("retired", "actinglab_device_authority_retired")
        }
        "retired" if command == "lab arbitrator" => ("retired", "legacy_lab2_arbitrator_retired"),
        "retired" => ("retired", "legacy_session_authority_retired"),
        "reserved" => ("reserved", "handler_reserved"),
        "unavailable" => ("unavailable", "lab_lease_required"),
        "available"
            if needs.iter().any(|need| {
                matches!(need.as_str(), "running_runtime" | "device" | "lab_lease")
            }) =>
        {
            ("unverified", "runtime_dependency_unverified")
        }
        "available" => ("available", "offline_handler_ready"),
        _ => ("unverified", "availability_evidence_missing"),
    };
    json!({
        "command": command,
        "needs": needs,
        "status": status,
        "available": status == "available",
        "reason_code": reason_code,
        "availability_scope": "offline_declaration"
    })
}

pub(crate) fn run_capabilities(global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let root = effective_resource_root(global, &config);
    let discovered = match root {
        Some(root) if root.is_dir() => discover_recognition_packs(&root)?,
        _ => Vec::new(),
    };
    let recognition_match_policy = discovered
        .iter()
        .map(|pack| {
            json!({
                "game": pack.get("game"),
                "server": pack.get("server"),
                "locale": pack.get("locale"),
                "match_metric": pack.get("match_metric")
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "commands": command_capabilities(),
        "session_layer": session_layer_capability_contract(),
        "exit_codes": exit_code_table(),
        "recognition_match_policy": recognition_match_policy,
        "schema_domains": schema_capabilities(),
        "capture_backends": capture_backend_capabilities(),
        "lab2_cli": lab2_cli::capability_summary(&config),
        "discovered_recognition_packs": discovered
    }))
}

pub(crate) fn session_layer_capability_contract() -> Value {
    json!({
        "schema_version": "session.capabilities.v0.1",
        "execution_authority": "runtime",
        "command_status_source": "commands",
        "resident_daemon": {
            "status": "retired",
            "available": false,
            "reason_code": "legacy_session_authority_retired",
            "request_command": "session request capabilities",
            "bootstrap_command": "session bootstrap",
            "throat_policy_command": "session throat-policy",
            "capture_policy_command": "session capture-policy",
            "self_heal_policy_command": "session self-heal-policy",
            "self_heal_plan_command": "session self-heal-plan [--trigger <kind>] [--to <page>]",
            "phase_c_plan_command": "session phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]",
            "status_command": "session status --diagnostics",
            "readiness_command": "session readiness",
            "validation_plan_command": "session validation-plan",
            "status_instance_registry_field": "diagnostics.instances",
            "monitor_policy_command": "session monitor-policy status",
            "journal_command": "session journal"
        },
        "access_channels": [
            {
                "id": "local_cli",
                "status": "available",
                "available": true,
                "reason_code": "offline_handler_ready",
                "encryption_required": false,
                "reason": "local operator command surface"
            },
            {
                "id": "trusted_remote",
                "status": "reserved",
                "available": false,
                "reason_code": "trusted_remote_transport_reserved",
                "encryption_required": true,
                "authentication_required": true,
                "plan_command": "session transport plan [--endpoint <url>]",
                "preflight_command": "session transport check --endpoint <url>",
                "auth_env": {
                    "token": TRUSTED_REMOTE_TOKEN_ENV,
                    "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
                },
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked",
                "reason": "future UI/API channel must be authenticated and encrypted"
            }
        ],
        "request_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": ["status", "queue", "journal", "capabilities", "devices", "session bootstrap", "session throat-policy", "session capture-policy", "session record-policy", "session self-heal-policy", "session self-heal-plan", "session phase-c-plan", "session transport plan", "session transport check", "session connect-plan", "session stream-plan", "session submit-plan", "session validation-plan", "session instance registry", "capture", "stream", "session recover --stale-capture", "session record step --capture", "session record step --current-frame", "session monitor-policy status"],
                "device_affecting_examples": ["capture", "stream", "session record step --capture", "session record step --current-frame"]
            },
            "daemon_state": {
                "requires_lease": false,
                "recovery_policy_requires_matching_lease": true,
                "recovery_policy_defers_without_matching_lease": true,
                "examples": ["session monitor-policy set", "session monitor-policy clear", "session record start", "session record step --frame <png>", "session record amend", "session record build-task", "session record promote"]
            },
            "control": {
                "requires_lease": true,
                "examples": ["tap", "swipe", "long-tap", "key", "text", "stream --input-relay", "stream --input-event <action,args>", "stream --relay-event <action,args>", "session app launch", "session app stop", "session app force-stop", "session app restart", "session instance app launch", "session instance app stop", "session instance app force-stop", "session instance app restart", "tap-target", "navigate", "recover except --stale-capture"]
            }
        },
        "safety": {
            "session_layer_only_throat": false,
            "runtime_only_device_holder": true,
            "strict_session_throat_status": "retired",
            "strict_session_throat_flag": "--require-session",
            "strict_session_throat_env": REQUIRE_SESSION_DAEMON_ENV,
            "strict_session_throat_failure_code": "validation_failed",
            "ui_must_not_directly_touch_adb_or_device": true,
            "control_requests_require_matching_lease": true,
            "severe_errors_fail_loud": true
        }
    })
}

/// Supported contract domains, independent of deployed Runtime/provider availability.
pub(crate) fn schema_capabilities() -> Value {
    json!({
        "cli_envelope": {"supported": [actingcommand_contract::CLI_SCHEMA_VERSION]},
        "legacy_task": {"supported": ["0.1"]},
        "task_operation": {
            "supported": ["0.3", "0.4", "0.5", "0.6", "0.7", "0.8", "0.9"],
            "validation": "version_specific_fields_required"
        },
        "recognition_pack": {"supported": ["0.1", "0.3", "0.4", "0.5", "0.6"]},
        "control": {"supported": ["Lab-1y.control.v1", actingcommand_contract::PHASED_CONTROL_SCHEMA]},
        "package_reference": {
            "legacy_zip": {"kind": "sha256"},
            "git_source_tree": {"schema_version": actingcommand_contract::GitSourceTreeVersion::V1}
        }
    })
}

fn capture_backend_capabilities() -> Value {
    let mut backends = json!([
        {"id": "adb", "backend": "adb_screencap", "external_tool": true, "required_assets": ["adb"]},
        {"id": "droidcast_raw", "backend": "droidcast_raw", "external_tool_env": "ACTINGCOMMAND_DROIDCAST_RAW_APK", "required_assets": ["adb", "droidcast_raw_apk"]},
        {"id": "nemu_ipc", "backend": "nemu_ipc", "external_tool_env": "ACTINGCOMMAND_NEMU_FOLDER or ACTINGCOMMAND_NEMU_IPC_DLL", "required_assets": ["nemu_ipc_library"]},
        {"id": "auto", "fallback_allowed": true, "diagnostics_required": true},
        {"id": "auto-fastest", "probe_all_backends": true, "diagnostics_required": true}
    ]);
    for backend in backends.as_array_mut().expect("backend declaration array") {
        backend["declared_support"] = json!(true);
        backend["status"] = json!("unverified");
        backend["available"] = json!(false);
        backend["reason_code"] = json!("runtime_backend_configuration_and_assets_unverified");
        backend["execution_authority"] = json!("runtime");
        backend["availability_checked"] = json!(false);
    }
    backends
}

fn discover_recognition_packs(root: &Path) -> CliOutcome<Vec<Value>> {
    let packs = find_files(root, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".pack.json"))
    })?;
    let mut discovered = Vec::new();
    for pack in packs {
        let text = fs::read_to_string(&pack)
            .map_err(|err| CliError::usage(format!("failed to read {}: {err}", pack.display())))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|err| CliError::usage(format!("failed to parse {}: {err}", pack.display())))?;
        discovered.push(json!({
            "path": pack.display().to_string(),
            "status": "unverified",
            "available": false,
            "reason_code": "resource_metadata_not_admitted",
            "game": value.get("game").and_then(Value::as_str),
            "server": value.get("server").and_then(Value::as_str),
            "locale": value.get("locale").and_then(Value::as_str),
            "match_metric": value
                .get("defaults")
                .and_then(|defaults| defaults.get("match_metric"))
                .and_then(Value::as_str)
        }));
    }
    Ok(discovered)
}
