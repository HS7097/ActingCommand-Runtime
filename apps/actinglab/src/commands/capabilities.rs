use crate::{
    CliError, CliOutcome, GlobalOptions, REQUIRE_SESSION_DAEMON_ENV, effective_resource_root,
    exit_code_table, find_files, lab2_cli, package_cli, read_user_config, runtime_debug,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(crate) fn command_capabilities() -> Vec<Value> {
    let mut commands = vec![
        command_cap("version", ["offline"], "available"),
        command_cap("doctor", ["offline"], "available"),
        command_cap("paths", ["offline"], "available"),
        // Workflow #434 LAB-2 (open point 7): actinglab's own user configuration file.
        command_cap("user-config get", ["offline"], "available"),
        command_cap("user-config set", ["offline"], "available"),
        command_cap("schema", ["offline"], "available"),
        command_cap("list", ["offline"], "available"),
        command_cap("capabilities", ["offline"], "available"),
        command_cap("resource validate", ["offline"], "available"),
        command_cap("resource catalog", ["offline", "read_only"], "available"),
        command_cap("resource restore", ["offline"], "available"),
        command_cap("scheduling compile", ["offline", "read_only"], "available"),
        command_cap("scheduling timeline", ["offline", "read_only"], "available"),
        command_cap("resource compile-maa", ["offline"], "available"),
        // Workflow #411 RV-1: `report_schema` tells a repository wrapper this build can verify.
        resource_verify_capability(),
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
        command_cap("package preflight", ["offline", "read_only"], "available"),
        // Workflow #411 A5: the Runtime's structural admission of one package; reads only.
        command_cap("package check", ["offline", "read_only"], "available"),
        command_cap("package inspect", ["offline"], "available"),
        package_cli::offline_capability(),
        command_cap("replay", ["offline"], "available"),
        command_cap(
            "recognition-regression",
            ["offline", "read_only"],
            "available",
        ),
        command_cap("package bundle", ["offline"], "available"),
        // Workflow #407 LOAD-1: copies a game's loading pages into its packs, or checks them.
        command_cap("package loading", ["offline"], "available"),
        // Workflow #373 NAV-T: the bundle's cross-check of an installed game root; reads only.
        command_cap("standard-pack check", ["offline", "read_only"], "available"),
        // ROUT-L RL-1a: composes the schedule rules of bound games into a new directory.
        command_cap("standard-pack compose", ["offline"], "available"),
        command_cap("package fmt", ["offline"], "available"),
        command_cap("package edit", ["offline"], "available"),
        command_cap("operation validate", ["offline"], "available"),
        command_cap("operation inspect", ["offline"], "available"),
        command_cap("operation explain", ["offline"], "available"),
        // Workflow #434 LAB-2b: `status` is the CLI module's core row now, not actinglab's own.
        command_cap("run summary", ["running_runtime", "read_only"], "available"),
        command_cap("touch-probe", ["device"], "available"),
        command_cap("tap", ["device"], "available"),
        command_cap("swipe", ["device"], "available"),
        command_cap("long-tap", ["device"], "available"),
        command_cap("key", ["device"], "available"),
        command_cap("text", ["device"], "available"),
        command_cap("session status", ["running_runtime"], "available"),
        command_cap("session throat-policy", ["offline"], "available"),
        command_cap("session capture-policy", ["offline"], "available"),
        command_cap("session record-policy", ["offline"], "available"),
        command_cap("session self-heal-policy", ["offline"], "available"),
        command_cap("session contract", ["offline"], "available"),
        command_cap("session api", ["offline"], "available"),
        command_cap("session transport", ["offline"], "available"),
        command_cap("session transport check", ["offline"], "available"),
        command_cap("session stream", ["running_runtime", "device"], "available"),
        command_cap("session stream check", ["offline"], "available"),
        command_cap("session monitor-policy", ["running_runtime"], "available"),
        command_cap("session instance", ["offline", "device"], "available"),
        command_cap("session instance list", ["offline"], "available"),
        command_cap("session instance registry", ["offline"], "available"),
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
        // #402 HS-L: switches the instance's touch and capture backends through the Runtime.
        command_cap(
            "session backend",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap("session capture", ["device"], "available"),
        command_cap("session capture diagnose", ["device"], "available"),
        command_cap("session recover", ["device"], "available"),
        command_cap("session recover --stale-capture", ["device"], "available"),
        command_cap("session record", ["offline"], "available"),
        command_cap("session record start", ["offline"], "available"),
        command_cap("session record status", ["offline"], "available"),
        command_cap("session record stop", ["offline"], "available"),
        command_cap("session record mark", ["offline"], "available"),
        command_cap("session record step", ["offline", "device"], "available"),
        command_cap("session record candidates", ["offline"], "available"),
        command_cap("session record amend", ["offline"], "available"),
        command_cap("record", ["offline"], "available"),
        command_cap("record start", ["offline"], "available"),
        command_cap("record status", ["offline"], "available"),
        command_cap("record stop", ["offline"], "available"),
        command_cap("record mark", ["offline"], "available"),
        command_cap("record step", ["offline", "device"], "available"),
        command_cap("record candidates", ["offline"], "available"),
        command_cap("record amend", ["offline"], "available"),
        // Workflow #434 LAB-5G4c: the binding draft of a `record stop` answer; it changes nothing.
        command_cap("record draft", ["offline", "read_only"], "available"),
        command_cap("capture --record", ["device"], "available"),
        command_cap("session capture --record", ["device"], "available"),
        command_cap("session app --record", ["device"], "available"),
        command_cap("session instance app --record", ["device"], "available"),
        command_cap(
            "observe --capture --record",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "do --capture --record",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap("current-page", ["device"], "available"),
        command_cap("is-visible", ["device"], "available"),
        command_cap("locate", ["device"], "available"),
        command_cap("tap-target", ["device"], "available"),
        command_cap("navigate", ["device"], "available"),
        command_cap("stream", ["device"], "available"),
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
    // Workflow #434 LAB-2: the retired and reserved command words are gone from the table above;
    // `lab arbitrator` is the one retired entry left, `lab2_cli`'s.
    let (status, reason_code) = match status {
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
    let mut capability = json!({
        "command": command,
        "needs": needs,
        "status": status,
        "available": status == "available",
        "reason_code": reason_code,
        "availability_scope": "offline_declaration"
    });
    crate::dry_run_gate::annotate_capability(&mut capability);
    capability
}

/// Workflow #411 RV-1: `resource verify`'s entry, with the schema of the report it writes.
fn resource_verify_capability() -> Value {
    let mut capability = command_cap("resource verify", ["offline"], "available");
    capability["report_schema"] = json!(crate::resource_verify::REPORT_SCHEMA);
    capability
}

/// Workflow #434 LAB-5B5 (register §1.287, "Slice LAB-5B5"): the flags and words `capabilities`
/// reads: none (it reads actinglab's global options); its command line is parsed by it.
pub(crate) const CAPABILITIES_READS: crate::flag_args::reads::FlagReads =
    crate::flag_args::reads::FlagReads {
        row: "capabilities",
        parts: &[],
        positionals: &[],
    };

pub(crate) fn run_capabilities(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    crate::FlagArgs::parse_reading(args, &CAPABILITIES_READS)?;
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
    // Workflow #434 LAB-5F: the authoring module's commands only where the option lists them.
    let commands = crate::cli_module::authoring::listed(command_capabilities())?;
    Ok(json!({
        "commands": commands,
        "session_layer": session_layer_capability_contract(),
        // Workflow #434 LAB-2b fix 1: the exit codes of this command line, `--via` or not.
        "exit_codes": exit_code_table(global.via.is_some()),
        "recognition_match_policy": recognition_match_policy,
        "schema_domains": schema_capabilities(),
        "capture_backends": capture_backend_capabilities(),
        "touch_backends": touch_backend_capabilities(),
        "lab2_cli": lab2_cli::capability_summary(&config, global.via.is_some()),
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
                "status": "retired",
                "available": false,
                "reason_code": "trusted_remote_transport_retired"
            }
        ],
        "request_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": ["status", "queue", "journal", "capabilities", "devices", "session bootstrap", "session throat-policy", "session capture-policy", "session record-policy", "session self-heal-policy", "session self-heal-plan", "session phase-c-plan", "session transport check", "session connect-plan", "session stream-plan", "session submit-plan", "session validation-plan", "session instance registry", "capture", "stream", "session recover --stale-capture", "session record step --capture", "session record step --current-frame", "session monitor-policy status"],
                "device_affecting_examples": ["capture", "stream", "session record step --capture", "session record step --current-frame"]
            },
            "daemon_state": {
                "requires_lease": false,
                "recovery_policy_requires_matching_lease": true,
                "recovery_policy_defers_without_matching_lease": true,
                "examples": ["session monitor-policy set", "session monitor-policy clear", "session record start", "session record step --frame <png>", "session record amend", "record mark --frame <png>", "record mark --request <file>", "record mark --step <k> --optional [--settle-ms <ms>]", "record mark --step <k> --not-optional", "capture --record", "observe --capture --record"]
            },
            "control": {
                "requires_lease": true,
                "examples": ["tap", "swipe", "long-tap", "key", "text", "stream --input-relay", "stream --input-event <action,args>", "stream --relay-event <action,args>", "session app launch", "session app stop", "session app force-stop", "session app restart", "session instance app launch", "session instance app stop", "session instance app force-stop", "session instance app restart", "tap-target", "navigate", "recover except --stale-capture", "do --capture --record", "session app <launch|restart|stop|force-stop> --record", "session instance app <launch|restart|stop|force-stop> --record"]
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
            "supported": actingcommand_contract::TaskSchema::ALL
                .map(actingcommand_contract::TaskSchema::as_str),
            "validation": "version_specific_fields_required"
        },
        "recognition_pack": {"supported": ["0.1", "0.3", "0.4", "0.5", "0.6", "0.7"]},
        "control": {"supported": ["Lab-1y.control.v1", actingcommand_contract::PHASED_CONTROL_SCHEMA]},
        // Workflow #336 PKG-S4 (option 丙): a package is found by its path, which chooses how
        // it is read; no reference or digest names it.
        "package_locator": {
            "by": "path",
            "readings": ["directory", "zip", "json"]
        }
    })
}

/// The capture choices an actingd instance may name (`capture_backend`); `scrcpy` only on a
/// handset (`contracts/device-binding.md`). Workflow #411 Lab L4: the automatic choices stay
/// listed as refused, since actingd refuses an instance that names one
/// (`capture_backend_must_be_explicit`).
fn capture_backend_capabilities() -> Value {
    let mut backends = json!([
        {"id": "adb", "backend": "adb_screencap", "external_tool": true, "required_assets": ["adb"]},
        {"id": "droidcast_raw", "backend": "droidcast_raw", "external_tool_env": "ACTINGCOMMAND_DROIDCAST_RAW_APK", "required_assets": ["adb", "droidcast_raw_apk"]},
        {"id": "nemu_ipc", "backend": "nemu_ipc", "external_tool_env": "ACTINGCOMMAND_NEMU_FOLDER or ACTINGCOMMAND_NEMU_IPC_DLL", "required_assets": ["nemu_ipc_library"]},
        {"id": "scrcpy", "backend": "scrcpy", "required_assets": ["adb"], "device_kinds": ["handset"]},
        {"id": "auto", "fallback_allowed": true, "diagnostics_required": true},
        {"id": "auto-fastest", "probe_all_backends": true, "diagnostics_required": true}
    ]);
    for backend in backends.as_array_mut().expect("backend declaration array") {
        let explicit = !matches!(backend["id"].as_str(), Some("auto" | "auto-fastest"));
        if explicit {
            declare_runtime_backend(backend);
        } else {
            backend["declared_support"] = json!(false);
            backend["status"] = json!("unavailable");
            backend["available"] = json!(false);
            backend["reason_code"] = json!("capture_backend_must_be_explicit");
            backend["execution_authority"] = json!("runtime");
            backend["availability_checked"] = json!(false);
        }
    }
    backends
}

/// Workflow #411 Lab L4: the input backends an actingd instance may name (`touch_backend`), with
/// the Runtime input actions each performs. `nemu_ipc` performs them only on a frame the Runtime
/// captured (`nemu_input_frame_required`); `adb_shell_input` performs a long tap as a swipe that
/// stays at its point. Workflow #406 T4: every backend that performs segmented plans performs the
/// path drag (`single_touch_path_drag_v1`, Workflow #406 T3), and `adb_shell_input` performs the
/// segmented plans (both brake drags and the path drag) through `input motionevent` from the
/// device's Android SDK level 23 (plan Q18 (a)). HS-4 (#402): `nemu_ipc` performs the long tap
/// and the swipe too (still no `key` or `text`).
fn touch_backend_capabilities() -> Value {
    let mut backends = json!([
        {"id": "nemu_ipc", "gestures": ["tap", "long_tap", "swipe", "single_touch_drag_with_vertical_brake_v1", "single_touch_drag_with_pause_and_brake_v1", "single_touch_path_drag_v1"]},
        {"id": "maatouch", "gestures": ["tap", "long_tap", "swipe", "single_touch_drag_with_vertical_brake_v1", "single_touch_drag_with_pause_and_brake_v1", "single_touch_path_drag_v1", "key", "text"]},
        {"id": "minitouch", "gestures": ["tap", "long_tap", "swipe", "single_touch_drag_with_vertical_brake_v1", "single_touch_drag_with_pause_and_brake_v1", "single_touch_path_drag_v1"]},
        {"id": "adb_shell_input", "gestures": ["tap", "long_tap", "swipe", "single_touch_drag_with_vertical_brake_v1", "single_touch_drag_with_pause_and_brake_v1", "single_touch_path_drag_v1"]}
    ]);
    for backend in backends.as_array_mut().expect("backend declaration array") {
        declare_runtime_backend(backend);
    }
    backends
}

/// A backend the Runtime supports and this offline query does not check.
fn declare_runtime_backend(backend: &mut Value) {
    backend["declared_support"] = json!(true);
    backend["status"] = json!("unverified");
    backend["available"] = json!(false);
    backend["reason_code"] = json!("runtime_backend_configuration_and_assets_unverified");
    backend["execution_authority"] = json!("runtime");
    backend["availability_checked"] = json!(false);
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
