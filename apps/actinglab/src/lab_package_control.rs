use crate::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, IdIssuer, IdKind, PackageValidationResponse,
    create_package_blocked_result_zip, file_sha256, lab_run, lab2_cli, package_build, package_cli,
    reject_legacy_session_routing, require_runtime, run_session_status, runtime_debug,
    runtime_session_adapter, validate_json_file, validate_operation_dir,
};
use actingcommand_device::vendor_stdio_session_diagnostic;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn run_lab(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "run" => {
            let flags = FlagArgs::parse(args)?;
            reject_legacy_session_routing(&flags)?;
            lab_run::run_lab_run(global, args)
        }
        "validate" => lab_run::run_lab_validate(args),
        "debug-package" | "watch" => runtime_debug::run_runtime_debug(sub, args),
        "export-evidence" | "replay-evidence" => runtime_debug::run_runtime_debug(sub, args),
        "start" => {
            require_runtime(global)?;
            let flags = FlagArgs::parse(args)?;
            let mode = flags
                .optional("--mode")
                .unwrap_or("passive_mirror".to_string());
            if !["passive_mirror", "scheduler_noop", "exclusive_drain"].contains(&mode.as_str()) {
                return Err(CliError::usage(format!("unsupported lab mode: {mode}")));
            }
            Err(CliError::not_implemented(
                "not_implemented",
                "lab start is reserved until Runtime lab sessions are connected",
            ))
        }
        "status" => run_session_status(global, args),
        "lease" | "preempt" | "release" => runtime_session_adapter::retired_authority(sub, args),
        "receipt" => lab2_cli::run_receipt(global, args),
        "evidence" => lab2_cli::run_evidence(global, args),
        "arbitrator" => lab2_cli::run_arbitrator(global, args),
        "vendor-stdio-selftest" => run_lab_vendor_stdio_selftest(args),
        _ => Err(CliError::usage(format!("unknown lab command: {sub}"))),
    }
}

fn run_lab_vendor_stdio_selftest(args: &[String]) -> CliOutcome<Value> {
    FlagArgs::parse(args)?.expect_positionals("lab vendor-stdio-selftest", 0)?;
    let capture =
        vendor_stdio_session_diagnostic().map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "status": "ok",
        "stdout_captured": !capture.stdout.is_empty(),
        "stderr_captured": !capture.stderr.is_empty(),
        "captured": capture
    }))
}

pub(super) fn run_package(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "validate" => package_cli::run_validate(global, &flags),
        "dry-run" => package_cli::run_offline(global, &flags),
        "inspect" => {
            let zip = flags.required_path("--zip")?;
            let validation = package_cli::validate_package(&zip, true)?;
            let mut payload = package_cli::serialize_response(&validation)?;
            attach_package_event(
                global,
                "package.inspect.ok",
                "package-inspect",
                &zip,
                &validation,
                &mut payload,
            )?;
            Ok(payload)
        }
        "run" => {
            reject_legacy_session_routing(&flags)?;
            let zip = flags.required_path("--zip")?;
            let out = flags.optional_path("--out");
            let validation = package_cli::validate_package(&zip, false)?;
            if global.instance.is_none() && global.game.is_none() {
                return Err(CliError::instance(
                    "package run requires --instance or --game/--server selector",
                ));
            }
            let result_zip = out
                .map(|out| create_package_blocked_result_zip(&out, &validation))
                .transpose()?;
            let mut details = package_cli::serialize_response(&validation)?;
            details["status"] = json!("blocked");
            details["blocked_by"] = json!(["lab_lease", "exclusive_drain"]);
            details["result_zip"] =
                json!(result_zip.as_ref().map(|path| path.display().to_string()));
            attach_package_event(
                global,
                "package.run.blocked",
                global.instance.as_deref().unwrap_or("package-run"),
                &zip,
                &validation,
                &mut details,
            )?;
            Err(CliError::safety_blocked(
                "lab_lease_required",
                format!(
                    "package run requires an exclusive_drain LabLease before executing navigation-only operations{}",
                    result_zip
                        .as_ref()
                        .map(|path| format!("; blocked result zip written to {}", path.display()))
                        .unwrap_or_default()
                ),
                &["lab_lease", "exclusive_drain"],
            )
            .with_details(details))
        }
        "build-task" => package_build::run_build_task(global, &flags),
        "build-pack" => package_build::run_build_pack(global, &flags),
        _ => Err(CliError::usage(format!("unknown package command: {sub}"))),
    }
}

pub(super) fn attach_package_event(
    global: &GlobalOptions,
    event_type: &str,
    instance: &str,
    zip: &Path,
    validation: &PackageValidationResponse,
    payload: &mut Value,
) -> CliOutcome<()> {
    let req_id = IdIssuer::new().issue(IdKind::Req).value;
    let event = write_package_light_event(global, event_type, instance, &req_id, zip, validation)?;
    payload["req_id"] = json!(req_id);
    payload["ledger_event"] = event;
    Ok(())
}

fn write_package_light_event(
    _global: &GlobalOptions,
    event_type: &str,
    instance: &str,
    _req_id: &str,
    _zip: &Path,
    validation: &PackageValidationResponse,
) -> CliOutcome<Value> {
    Ok(json!({
        "written": false,
        "reason": "offline_resource_tooling_projection",
        "event_type": event_type,
        "instance": instance,
        "module": validation.module,
        "task_count": validation.task_count,
        "entry_count": validation.entry_count
    }))
}

pub(super) fn run_operation(
    sub: &str,
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "validate" | "inspect" | "explain" => {
            let dir = flags.required_path("--operation-dir")?;
            let report = validate_operation_dir(&dir)?;
            Ok(json!({
                "operation_dir": dir.display().to_string(),
                "status": "valid",
                "report": report,
                "mode": sub
            }))
        }
        "dry-run" => {
            require_runtime(global)?;
            Err(CliError::not_implemented(
                "not_implemented",
                "operation dry-run is reserved until Runtime operation adapter is connected",
            ))
        }
        "run" => {
            reject_legacy_session_routing(&flags)?;
            Err(CliError::safety_blocked(
                "lab_lease_required",
                "operation run requires Runtime scheduler admission",
                &["runtime_scheduler"],
            ))
        }
        _ => Err(CliError::usage(format!("unknown operation command: {sub}"))),
    }
}

pub(super) fn run_control(
    sub: &str,
    _global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "inspect" => Ok(json!({
            "control": flags.optional("--control"),
            "status": "reserved"
        })),
        "verify" => {
            let candidate = flags.required_path("--candidate")?;
            let candidate_id = flags.required("--candidate-id")?;
            validate_json_file(&candidate)?;
            Ok(json!({
                "candidate": candidate.display().to_string(),
                "candidate_id": candidate_id,
                "status": "validated",
                "click_executed": false
            }))
        }
        "probe-click" => {
            let effect = flags.optional("--effect").unwrap_or_default();
            if effect != "navigation_only" {
                return Err(CliError::safety_blocked(
                    "effect_not_navigation_only",
                    "control probe-click only allows effect navigation_only",
                    &["navigation_only"],
                ));
            }
            if flags.optional("--expect-before").is_none()
                || flags.optional("--expect-after").is_none()
            {
                return Err(CliError::safety_blocked(
                    "unresolved_coords",
                    "control probe-click requires expect-before and expect-after page guards",
                    &["expect_after", "page_guard"],
                ));
            }
            Err(CliError::safety_blocked(
                "lab_lease_required",
                "control probe-click requires an exclusive_drain LabLease",
                &["lab_lease", "exclusive_drain"],
            ))
        }
        "export" => Err(CliError::not_implemented(
            "not_implemented",
            "control export is reserved for stable-control promotion",
        )),
        "diff" => {
            let candidate = flags.required_path("--candidate")?;
            let stable = flags.required_path("--stable")?;
            let candidate_hash = file_sha256(&candidate)?;
            let stable_hash = file_sha256(&stable)?;
            Ok(json!({
                "candidate": candidate.display().to_string(),
                "stable": stable.display().to_string(),
                "same_hash": candidate_hash == stable_hash,
                "candidate_sha256": candidate_hash,
                "stable_sha256": stable_hash
            }))
        }
        _ => Err(CliError::usage(format!("unknown control command: {sub}"))),
    }
}

pub(super) fn run_scheduler(sub: &str, _global: &GlobalOptions) -> CliOutcome<Value> {
    match sub {
        "status" | "pause" | "resume" | "start" | "stop" => Err(CliError::not_implemented(
            "scheduler_not_available",
            "Scheduler interface is reserved but not implemented yet.",
        )),
        _ => Err(CliError::usage(format!("unknown scheduler command: {sub}"))),
    }
}
