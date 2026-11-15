use crate::flag_args::reads::FlagReads;
use crate::runtime_endpoint::runtime_endpoint_check;
use crate::user_config_keys::{config_get, config_set};
use crate::user_config_store::{config_path, read_user_config, write_user_config};
use crate::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, RUNTIME_VERSION, SCHEMA_VERSION,
    command_capabilities, effective_resource_root, effective_run_root, effective_runtime_endpoint,
    lab2_cli, list_resource_kind, path_string, process_env_overrides, resolved_adb_json,
    resolved_adb_json_from,
};
use actingcommand_device::resolve_adb_path;
use serde_json::{Value, json};

/// Workflow #434 LAB-5B5 (register §1.287, "Slice LAB-5B5"): the flags and words `help` reads:
/// no flag; its command line is parsed by it (`FlagArgs::parse_reading`), which refuses one.
/// Workflow #434 CLI-HELP (register §1.313): its words are the command words it describes.
pub(crate) const HELP_READS: FlagReads = FlagReads {
    row: "help",
    parts: &[],
    positionals: &["<command>..."],
};

/// Workflow #434 LAB-5B5: `version`'s: none. A line `--version` makes a version line keeps its
/// words, which are the line's own and not the command's (`cli_parse`), unread as before.
pub(crate) const VERSION_READS: FlagReads = FlagReads {
    row: "version",
    parts: &[],
    positionals: &[],
};

/// Workflow #434 LAB-5B5: `paths`' flags and words: none (it reads actinglab's global options).
pub(crate) const PATHS_READS: FlagReads = FlagReads {
    row: "paths",
    parts: &[],
    positionals: &[],
};

/// Workflow #434 LAB-5B5: `doctor`'s flags and words: none (it reads the global options).
pub(crate) const DOCTOR_READS: FlagReads = FlagReads {
    row: "doctor",
    parts: &[],
    positionals: &[],
};

/// Workflow #434 LAB-5B5: `list`'s: no flag, and the kind it lists as its one positional.
pub(crate) const LIST_READS: FlagReads = FlagReads {
    row: "list",
    parts: &[],
    positionals: &["<kind>"],
};

/// Workflow #434 LAB-5B5: `user-config get`'s: no flag, and the key as its one positional.
pub(crate) const USER_CONFIG_GET_READS: FlagReads = FlagReads {
    row: "user-config get",
    parts: &[],
    positionals: &["<key>"],
};

/// Workflow #434 LAB-5B5: `user-config set`'s: no flag, and the key and its value.
pub(crate) const USER_CONFIG_SET_READS: FlagReads = FlagReads {
    row: "user-config set",
    parts: &[],
    positionals: &["<key>", "<value>"],
};

/// Workflow #434 LAB-5B5: `help`, its command line parsed by [`HELP_READS`] first. Workflow #434
/// CLI-HELP (register §1.313): without words, every member it answered before and `list`, every
/// command this build offers with its sentence (`cli_help::list`); with the words of a command or
/// a group, that command's detail or the group's commands (`cli_help::topic`).
pub(super) fn run_help(args: &[String]) -> CliOutcome<Value> {
    let words = FlagArgs::parse_reading(args, &HELP_READS)?.positionals;
    let mut help = help_data();
    if !words.is_empty() {
        return crate::cli_help::topic(&words, &help["command_options"]);
    }
    let list = crate::cli_help::list(&help["command_options"])?;
    help["list"] = list;
    crate::cli_module::authoring::listed_help(help)
}

/// Workflow #434 LAB-5B5: `version`, its command line parsed by [`VERSION_READS`] first, but on a
/// line the global `--version` made a version line.
pub(super) fn run_version(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    if !global.version {
        FlagArgs::parse_reading(args, &VERSION_READS)?;
    }
    Ok(version_data())
}

pub(super) fn help_data() -> Value {
    let mut help = json!({
        "usage": "actinglab [global-options] <command> [args]",
        "global_options": [
            "--json",
            "--run-root <path>",
            "--instance <id> (names the instance on every command that acts on one; on an operation row it stands for the row's <instance>, never beside it)",
            "--instances <id,id,...>",
            "--profile <name>",
            "--resource-root <path>",
            "--game <game>",
            "--server <server>",
            "--runtime-endpoint <127.0.0.1:port>",
            "--capture-backend <auto|auto-fastest|adb|droidcast_raw|nemu_ipc>",
            "--backend <auto|auto-fastest|adb|droidcast_raw|nemu_ipc> (alias of --capture-backend)",
            "--touch-backend <auto|auto-fastest|maatouch|minitouch|adb_shell_input>",
            "--via <mcp|ui> (set by the counter that forwards the work; every Runtime connection binds the Lab channel)",
            "--require-session",
            "--dry-run (global; read at any argv position; commands[].dry_run_mode states each command's behaviour)",
            "--verbose",
            "--quiet",
            "--version"
        ],
        "command_options": {
            "resource validate": crate::resource_runtime_support::resource_declarations::options(),
            "resource verify": crate::resource_verify::options(),
            "resource catalog": ["--repo <repository root>", "--catalog <static catalog JSON>", "--catalog-server <server>", "--field <identity feature name>"],
            "package preflight": ["--package <directory | .zip | .json container>", "--zip <published .zip package>"],
            "package check": ["--package <directory | .zip | .json container>"],
            "package dry-run": [
                "--package <directory | .zip | .json container> | --zip <published .zip package>",
                "--fixture <recorded.png> (repeatable, one per frame)",
                "--out <the result file>",
                "--seed <unsigned 64-bit integer: the run's draws, 0 without it>",
                "--standard-pack-root <the root of the game directories: the shared tables a select step reads>"
            ],
            "operation validate": ["--repo <repository root>", "--operation-dir <repository-relative operation directory>"],
            "lab watch": crate::runtime_debug::watch_options(),
            "lab unpin": ["--artifact-id <artifact-id>", "--pin-sequence <sequence>", "--pin-event-id <event-id>"],
            "lab signatures register": crate::signature_cli::options("register"),
            "lab signatures match": crate::signature_cli::options("match"),
            "lab signatures retire": crate::signature_cli::options("retire"),
            "resource restore": [
                "--repo <new directory>", "--state-root <Runtime state>",
                "--request-id <ID> (repeatable, 1–32 unique)", "--through-sequence <upper bound>",
                "--package <directory | .zip | .json container> | --zip <published .zip package>", "--task-id <draft ID>",
                "--entry-page <page>", "--target-page <page> (repeatable)", "--goal <author text>"
            ],
            "resource compile-maa": [
                "--maa-tasks <dir>", "--task <id> (repeatable with --facts)", "--facts"
            ],
            "recognition-regression": [
                "--state-root <the Runtime state root whose ledger kept the run's frames>",
                "--evidence <the saved answer of actingctl run evidence or ac_run_evidence; repeatable, one per page of the same run>",
                "--package <directory | .zip | .json container> | --zip <published .zip package>: the new package or version"
            ],
            "replay": [
                "--package <directory | .zip | .json container>",
                "--frames <directory of PNG frames, replayed in file-name order>",
                "--facts <the instance answer of actingctl facts --instance or ac_facts, read without a key prefix>",
                "--select <operation id of a select step: replays its scan over the frames, with --scan>",
                "--scan <the step's scan declaration, a JSON file, with --select>",
                "--parameter <name>=<value> (with --select; repeatable, once per parameter the package declares; replaces that parameter's declared default)",
                "--standard-pack-root <the root of the game directories: the shared tables a select step reads>"
            ],
            "package bundle": [
                "--applications <applications.json>", "--packs-root <directory of pack directories>",
                "--out <new directory>", "--source-repository <owner/name>", "--source-commit <commit>",
                "--maintenance <JSON array of package_id, server, uses; written into actingcommand.bundle.v4>",
                "--pages <pages.json>", "--interrupts <interrupts.json>",
                "--navigation <navigation.json> (root files: cross-checked, laid beside bundle.json)",
                "--schedule <schedule.json> (the schedule rules: composed as the Runtime composes them, laid beside bundle.json)",
                "--display-layout <display-layout.json> (the display declaration: checked as the Runtime reads it, laid beside bundle.json)",
                "--loading <loading.json> (the game's loading pages: every pack's loading copy checked as package loading --check checks it, laid beside bundle.json with its loading/ templates; without it a pack that carries a copy is refused)",
                "--notices <directory> (the package author's notice texts: every file held to the notices rules before anything is written, laid byte for byte into notices/ beside bundle.json; not listed in bundle.json)",
                "--goals <goals.json> (the goal entries the game offers: read after the composition and held to the bundle's pools, rules and packages, laid beside bundle.json; a refused file refuses the bundle)"
            ],
            "package loading": [
                "--root-file <loading.json> (the game's loading pages; their templates in the loading/ directory beside it)",
                "--packs-root <directory of pack directories>",
                "--write (writes each pack's resources/loading.json and resources/loading/ that is not the copy the root file makes; removes nothing; with --dry-run a preview that writes nothing)",
                "--check (writes nothing; fails with standard_pack_root_invalid, loading_pack_stale, naming every pack whose copy is not the one the root file makes)"
            ],
            // #402 HS-L: one ConfigureSettings of the instance's backend keys.
            "session backend": [
                "--instance <alias> (global, required: the Runtime instance whose backends change)",
                "--touch <name> (adb_shell_input, maatouch, minitouch, nemu_ipc; the Runtime judges it)",
                "--capture <name> (adb, droidcast_raw, nemu_ipc, scrcpy; the Runtime judges it)",
                "--clear (both keys back to the configuration file's values, itself a switch; with neither --touch nor --capture)"
            ],
            "standard-pack check": crate::standard_pack_cli::options(),
            "standard-pack compose": crate::standard_pack_cli::compose_options(),
            "standard-pack adopt": crate::standard_pack_cli::adopt_options(),
            "package fmt": [
                "--package <content directory>",
                "--check (writes nothing; fails with package_fmt_check_failed naming every JSON file that is not canonical)"
            ],
            "package edit": [
                "--package <content directory>",
                "--patch <JSON array of {file, op: add|replace, path: JSON pointer, value}>",
                "--file <path in the package> with --add <JSON pointer> or --replace <JSON pointer> and --value <JSON>: one edit instead of --patch"
            ]
        },
        "compatibility_notes": {
            "recognize --target": "target output includes width, height, matched_rect, and the shared evaluation object"
        },
        "commands": command_capabilities()
    });
    // #411 Lab B-2: the read-only verbs take an offline frame set.
    let scene = "--scene <png> (repeatable: 1 to 5 frames of one screen, oldest first, the last the current frame)";
    let package = "--package <directory | .zip | .json container> | --zip <published .zip package>";
    help["command_options"]["recognize"] = json!(["--target <id>", scene, package]);
    help["command_options"]["detect-page"] = json!([
        scene,
        package,
        "--check-pages (checks the page set and evaluates nothing)"
    ]);
    help["command_options"]["current-page"] = json!([scene, package]);
    help["command_options"]["is-visible"] = json!(["<target> | --target <id>", scene, package]);
    help
}

pub(super) fn version_data() -> Value {
    json!({
        "name": "actinglab",
        "cli_version": env!("CARGO_PKG_VERSION"),
        "runtime_version": RUNTIME_VERSION,
        "schema_version": SCHEMA_VERSION
    })
}

pub(super) fn run_paths(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    FlagArgs::parse_reading(args, &PATHS_READS)?;
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

/// Workflow #434 LAB-2 (open point 7): `user-config get|set`, actinglab's own user configuration
/// file (`config get|set` before LAB-2). Workflow #434 LAB-5B5: each parses its command line by
/// its read list first ([`USER_CONFIG_GET_READS`], [`USER_CONFIG_SET_READS`]), so that a flag
/// or a word beyond its key (and value) is refused; its words are then the list's positionals.
pub(super) fn run_config(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "get" => {
            let args = FlagArgs::parse_reading(args, &USER_CONFIG_GET_READS)?.positionals;
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
            let args = FlagArgs::parse_reading(args, &USER_CONFIG_SET_READS)?.positionals;
            if args.len() < 2 {
                return Err(CliError::usage("user-config set requires <key> <value>"));
            }
            let mut config = read_user_config()?;
            config_set(&mut config, &args[0], &args[1])?;
            if global.dry_run {
                return Ok(json!({
                    "config_path": config_path()?.display().to_string(),
                    "key": args[0],
                    "value": args[1],
                    "status": "validated",
                    "dry_run": true,
                    "persisted": false,
                    "next": "re-run without --dry-run to write config.json"
                }));
            }
            write_user_config(&config)?;
            Ok(json!({
                "config_path": config_path()?.display().to_string(),
                "key": args[0],
                "value": args[1]
            }))
        }
        _ => Err(CliError::usage(format!(
            "unknown user-config command: {sub}"
        ))),
    }
}

pub(super) fn run_doctor(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    FlagArgs::parse_reading(args, &DOCTOR_READS)?;
    let config = read_user_config()?;
    let adb_resolution = resolve_adb_path(config.adb_path.as_deref(), &process_env_overrides());
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

pub(super) fn run_schema(args: &[String]) -> CliOutcome<Value> {
    let kind = if args.is_empty() {
        "all".to_string()
    } else {
        args.join(" ")
    };
    let data = match kind.as_str() {
        "task" => json!({
            "schema_version": "0.1",
            "domain": "legacy_task",
            "task_operation": crate::commands::capabilities::schema_capabilities()["task_operation"],
            "required": ["schema_version", "id", "steps"],
            "step_action_types": ["complete", "click"]
        }),
        "control" => json!({
            "schema_version": "Lab-1y.control.v1",
            "domain": "control",
            "supported_schema_versions": crate::commands::capabilities::schema_capabilities()["control"]["supported"],
            "execution_modes": ["navigable_route", "recognize_only", "in_page_guard", "linear_steps"],
            // Workflow #411 Lab L4: the choices an actingd instance may name; actingd refuses the
            // automatic ones (`*_backend_must_be_explicit`).
            "capture_backend": ["adb", "droidcast_raw", "nemu_ipc", "scrcpy"],
            "touch_backend": ["nemu_ipc", "maatouch", "minitouch", "adb_shell_input"],
            "frame_store": {
                "similarity_threshold": "default 0.95; CLI --similarity-threshold overrides control",
                "tier1_ratio": "warning watermark; CLI --tier1-ratio",
                "tier2_ratio": "temp-disk spill watermark; CLI --tier2-ratio",
                "tier3_ratio": "alarm watermark; CLI --tier3-ratio",
                "hysteresis_ratio": "release margin for active watermarks; CLI --hysteresis-ratio",
                "max_mem_bytes": "optional lab frame-store cap; CLI --max-mem-bytes",
                "os_reserve_bytes": "physical-memory reserve left for the OS; CLI --os-reserve-bytes",
                "flush_workspace_reserve_bytes": "required byte gap between tier2 and tier3; CLI --flush-workspace-reserve-bytes",
                "floor_mem_bytes": "frame-store budget when the memory sample leaves less after the OS reserve; default 256 MiB",
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
            "domain": "recognition_pack",
            "schema_version": crate::commands::capabilities::schema_capabilities()["recognition_pack"]["supported"],
            "default_match_metric": "ccorr_normed",
            "supported_match_metric": ["ccorr_normed", "ccoeff_normed"]
        }),
        "package" => json!({
            "schema_version": "0.2",
            "domain": "module_package",
            "package_locator": crate::commands::capabilities::schema_capabilities()["package_locator"],
            "required_paths": ["<module>/manifest.json", "<module>/operations/<task_id>/task.json"],
            "security": ["no zip-slip", "no executable scripts", "structural admission, no digest compared"]
        }),
        "ledger" => json!({
            "schema_version": "actingcommand.ledger.query.v0.1",
            "status": "retired",
            "available": false,
            "reason_code": "local_ledger_retired",
            "commands": ["show", "events", "receipts", "diagnose", "evidence"],
            "filters": ["--run-id", "--req-id", "--instance-id"],
            "read_only": true,
            "device_io": false
        }),
        "all" => json!({
            "schemas": ["task", "control", "pack", "package", "ledger", "observe", "do", "ensure", "wait", "lab receipt", "lab unpin"],
            "schema_domains": crate::commands::capabilities::schema_capabilities()
        }),
        other => lab2_cli::command_schema(other)
            .ok_or_else(|| CliError::usage(format!("unknown schema kind: {other}")))?,
    };
    Ok(data)
}

pub(super) fn run_list(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    // Workflow #434 LAB-5B5: a flag, or a word after the kind, is refused, not ignored.
    let flags = FlagArgs::parse_reading(args, &LIST_READS)?;
    let kind = flags
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or("commands");
    match kind {
        // Workflow #434 LAB-5F: the authoring module's commands only where the option lists them.
        "commands" => Ok(json!({
            "commands": crate::cli_module::authoring::listed(command_capabilities())?
        })),
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
