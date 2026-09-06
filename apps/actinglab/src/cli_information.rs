use crate::runtime_endpoint::runtime_endpoint_check;
use crate::user_config_keys::{config_get, config_set};
use crate::user_config_store::{config_path, read_user_config, write_user_config};
use crate::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, RUNTIME_VERSION, SCHEMA_VERSION,
    command_capabilities, effective_resource_root, effective_run_root, effective_runtime_endpoint,
    lab2_cli, list_resource_kind, path_string, reject_legacy_session_routing, require_runtime,
    resolved_adb_json, resolved_adb_json_from,
};
use actingcommand_device::resolve_adb_path;
use serde_json::{Value, json};

pub(super) fn help_data() -> Value {
    json!({
        "usage": "actinglab [global-options] <command> [args]",
        "global_options": [
            "--json",
            "--run-root <path>",
            "--instance <id>",
            "--instances <id,id,...>",
            "--profile <name>",
            "--resource-root <path>",
            "--game <game>",
            "--server <server>",
            "--runtime-endpoint <url>",
            "--capture-backend <auto|auto-fastest|adb|droidcast_raw|nemu_ipc>",
            "--backend <auto|auto-fastest|adb|droidcast_raw|nemu_ipc> (alias of --capture-backend)",
            "--touch-backend <auto|auto-fastest|maatouch|minitouch|adb_shell_input>",
            "--require-session",
            "--dry-run",
            "--verbose",
            "--quiet",
            "--version"
        ],
        "command_options": {
            "resource restore": [
                "--repo <new directory>", "--state-root <Runtime state>",
                "--request-id <ID> (repeatable, 1–32 unique)", "--through-sequence <upper bound>",
                "--zip <package>", "--expected-sha256 <external hash>", "--task-id <draft ID>",
                "--entry-page <page>", "--target-page <page> (repeatable)", "--goal <author text>"
            ],
            "resource convert": [
                "--operations <dir>",
                "--out <dir>",
                "--maa-tasks <dir>"
            ],
            "resource compile-maa": [
                "--maa-tasks <dir>", "--task <id> (repeatable with --facts)", "--facts"
            ],
            "session record build-task": [
                "--locale <locale>"
            ]
        },
        "compatibility_notes": {
            "recognize --target": "target output includes width, height, matched_rect, and the shared evaluation object"
        },
        "commands": command_capabilities()
    })
}

pub(super) fn version_data() -> Value {
    json!({
        "name": "actinglab",
        "cli_version": env!("CARGO_PKG_VERSION"),
        "runtime_version": RUNTIME_VERSION,
        "schema_version": SCHEMA_VERSION
    })
}

pub(super) fn run_paths(global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let adb = resolved_adb_json(&config);
    Ok(json!({
        "config_path": config_path()?.display().to_string(),
        "run_root": global.run_root.as_ref().map(|path| path_string(path)).or(config.run_root),
        "resource_root": global.resource_root.as_ref().map(|path| path_string(path)).or(config.resource_root),
        "runtime_endpoint": global.runtime_endpoint.clone().or(config.runtime_endpoint),
        "adb": adb
    }))
}

pub(super) fn run_config(sub: &str, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "get" => {
            let config = read_user_config()?;
            if args.is_empty() {
                serde_json::to_value(config)
                    .map_err(|err| CliError::usage(format!("failed to serialize config: {err}")))
            } else {
                let key = &args[0];
                Ok(json!({
                    "key": key,
                    "value": config_get(&config, key)?
                }))
            }
        }
        "set" => {
            if args.len() < 2 {
                return Err(CliError::usage("config set requires <key> <value>"));
            }
            let mut config = read_user_config()?;
            config_set(&mut config, &args[0], &args[1])?;
            write_user_config(&config)?;
            Ok(json!({
                "config_path": config_path()?.display().to_string(),
                "key": args[0],
                "value": args[1]
            }))
        }
        _ => Err(CliError::usage(format!("unknown config command: {sub}"))),
    }
}

pub(super) fn run_doctor(global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let adb_resolution = resolve_adb_path(config.adb_path.as_deref());
    let runtime_endpoint = effective_runtime_endpoint(global, &config);
    let resource_root = effective_resource_root(global, &config);
    let run_root = effective_run_root(global, &config);
    let mut checks = Vec::new();

    checks.push(json!({
        "name": "config",
        "ok": config_path()?.exists(),
        "path": config_path()?.display().to_string()
    }));
    let mut adb_check = resolved_adb_json_from(adb_resolution);
    adb_check["name"] = json!("adb");
    checks.push(adb_check);
    let runtime_endpoint_check = runtime_endpoint
        .as_ref()
        .map(|endpoint| runtime_endpoint_check(endpoint));
    checks.push(json!({
        "name": "runtime_endpoint",
        "ok": runtime_endpoint_check.as_ref().and_then(|check| check.get("ok")).and_then(Value::as_bool).unwrap_or(false),
        "endpoint": runtime_endpoint,
        "policy": runtime_endpoint_check
    }));
    checks.push(json!({
        "name": "resource_root",
        "ok": resource_root.as_ref().map(|path| path.is_dir()).unwrap_or(false),
        "path": resource_root.as_ref().map(|path| path_string(path))
    }));
    checks.push(json!({
        "name": "run_root",
        "ok": run_root.as_ref().and_then(|path| path.parent()).map(|path| path.exists()).unwrap_or(false),
        "path": run_root.as_ref().map(|path| path_string(path))
    }));
    Ok(json!({
        "checks": checks,
        "note": "doctor is diagnostic; runtime/device unavailability is reported without blocking offline commands"
    }))
}

pub(super) fn run_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    require_runtime(global).map(|data| {
        json!({
            "state": "running",
            "runtime": data,
        })
    })
}

pub(super) fn run_devices(_global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    flags.expect_positionals("devices", 0)?;
    Err(CliError::not_implemented(
        "actinglab_device_authority_retired",
        "direct ADB device discovery was retired from ActingLab; query the resident Runtime",
    ))
}

pub(super) fn run_schema(args: &[String]) -> CliOutcome<Value> {
    let kind = if args.is_empty() {
        "all".to_string()
    } else {
        args.join(" ")
    };
    let data = match kind.as_str() {
        "task" => json!({
            "schema_version": "0.1",
            "required": ["schema_version", "id", "steps"],
            "step_action_types": ["complete", "click"]
        }),
        "control" => json!({
            "schema_version": "Lab-1y.control.v1",
            "execution_modes": ["navigable_route", "recognize_only", "in_page_guard"],
            "capture_backend": ["auto", "auto-fastest", "adb", "droidcast_raw", "nemu_ipc"],
            "touch_backend": ["auto", "auto-fastest", "maatouch", "minitouch", "adb_shell_input"],
            "frame_store": {
                "similarity_threshold": "default 0.95; CLI --similarity-threshold overrides control",
                "tier1_ratio": "warning watermark; CLI --tier1-ratio",
                "tier2_ratio": "temp-disk spill watermark; CLI --tier2-ratio",
                "tier3_ratio": "alarm watermark; CLI --tier3-ratio",
                "hysteresis_ratio": "release margin for active watermarks; CLI --hysteresis-ratio",
                "max_mem_bytes": "optional lab frame-store cap; CLI --max-mem-bytes",
                "os_reserve_bytes": "physical-memory reserve left for the OS; CLI --os-reserve-bytes",
                "flush_workspace_reserve_bytes": "required byte gap between tier2 and tier3; CLI --flush-workspace-reserve-bytes",
                "tier3_mode": "synchronous graceful partial-output failure; no runtime pause/resume wait is performed in this CLI"
            },
            "rules": [
                "CLI capture backend overrides control capture_backend",
                "CLI frame-store flags override control frame_store values",
                "trusted_execution is provenance and does not block semantic actions",
                "unresolved or placeholder coordinates are not executable"
            ]
        }),
        "pack" => json!({
            "schema_version": ["0.1", "0.3", "0.4", "0.5"],
            "default_match_metric": "ccorr_normed",
            "supported_match_metric": ["ccorr_normed", "ccoeff_normed"]
        }),
        "package" => json!({
            "schema_version": "0.2",
            "required_paths": ["<module>/manifest.json", "<module>/operations/<task_id>/task.json"],
            "security": ["no zip-slip", "no executable scripts", "hashes verified when declared"]
        }),
        "ledger" => json!({
            "schema_version": "actingcommand.ledger.query.v0.1",
            "commands": ["show", "events", "receipts", "diagnose", "evidence"],
            "filters": ["--run-id", "--req-id", "--instance-id"],
            "read_only": true,
            "device_io": false
        }),
        "all" => json!({
            "schemas": ["task", "control", "pack", "package", "ledger", "observe", "do", "ensure", "wait", "lab receipt"]
        }),
        other => lab2_cli::command_schema(other)
            .ok_or_else(|| CliError::usage(format!("unknown schema kind: {other}")))?,
    };
    Ok(data)
}

pub(super) fn run_list(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let kind = args.first().map(String::as_str).unwrap_or("commands");
    match kind {
        "commands" => Ok(json!({ "commands": command_capabilities() })),
        "targets" | "pages" | "tasks" | "bundles" | "controls" => {
            let config = read_user_config()?;
            let root = effective_resource_root(global, &config).ok_or_else(|| {
                CliError::usage("list requires --resource-root or config resource_root")
            })?;
            list_resource_kind(&root, kind)
        }
        other => Err(CliError::usage(format!("unknown list kind: {other}"))),
    }
}
