use crate::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, IdIssuer, IdKind, PackageValidationResponse,
    create_package_blocked_result_zip, file_sha256, lab_run, lab2_cli, package_build, package_cli,
    reject_legacy_session_routing, run_session_status, runtime_debug, validate_json_file,
    validate_operation_dir,
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
        "signatures" => {
            let form = args.first().map_or_else(
                || "lab signatures".to_string(),
                |operation| format!("lab signatures {operation}"),
            );
            crate::dry_run_gate::refuse(
                global,
                &form,
                Some(
                    "actingledger --state-root <historical-root> signatures --through <sequence> --catalog-state-root <registered-ledger-root> --catalog-through <sequence>",
                ),
                Value::Null,
            )?;
            crate::signature_cli::run_signatures(args)
        }
        "debug-package" | "watch" | "unpin" => {
            if sub != "watch" {
                let alternative = if sub == "unpin" {
                    "lab watch"
                } else {
                    "lab validate"
                };
                crate::dry_run_gate::refuse(
                    global,
                    &format!("lab {sub}"),
                    Some(alternative),
                    Value::Null,
                )?;
            }
            runtime_debug::run_runtime_debug(sub, args)
        }
        "export-evidence" | "replay-evidence" => {
            if sub == "export-evidence" {
                crate::dry_run_gate::refuse(global, "lab export-evidence", None, Value::Null)?;
            }
            runtime_debug::run_runtime_debug(sub, args)
        }
        // Workflow #434 LAB-2: the reserved `lab start` and the retired `lab lease`, `preempt` and
        // `release` are gone; they are unknown lab commands now.
        "status" => run_session_status(global, args),
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

/// Workflow #434 LAB-5B3: the flags `package inspect` reads. Workflow #434 LAB-5B4: its command
/// line is parsed by it (`FlagArgs::parse_reading`); no positional.
pub(crate) const INSPECT_READS: crate::flag_args::reads::FlagReads =
    crate::flag_args::reads::FlagReads {
        row: "package inspect",
        parts: &[&crate::flag_args::reads::ReadPart {
            options: &["--zip"],
            switches: &[],
            refused: &[],
        }],
        positionals: &[],
    };

/// Workflow #434 LAB-5B4: the flags of `package <sub>`: `validate`'s, `inspect`'s and `bundle`'s
/// parsed by their read lists (`FlagArgs::parse_reading`), which refuse a word the list does not
/// hold; `fmt`'s and `loading`'s with their switches taking no value from the next word but `true`
/// or `false` (they check their flags against their own lists); every other subcommand's as
/// before. A function of its own, not a match in `run_package`: the command inventory guard
/// (`command_inventory_matches_checked_in_snapshot`) takes the first match on the subcommand in a
/// group's dispatcher for its dispatch.
fn package_flags(sub: &str, args: &[String]) -> CliOutcome<FlagArgs> {
    Ok(match sub {
        "validate" => FlagArgs::parse_reading(args, &package_cli::VALIDATE_READS)?,
        "inspect" => FlagArgs::parse_reading(args, &INSPECT_READS)?,
        "bundle" => FlagArgs::parse_reading(args, &package_build::BUNDLE_READS)?,
        "fmt" => FlagArgs::parse_switches(args, &crate::package_tool::FMT_SWITCHES),
        "loading" => FlagArgs::parse_switches(args, &package_build::LOADING_SWITCHES),
        _ => FlagArgs::parse(args)?,
    })
}

/// Workflow #434 LAB-5B3 (tests only): holds the reads of `package fmt`'s and `package loading`'s
/// flags to the forms the register gives them (the read lists hold the others,
/// [`package_flags`]); a function of its own for the reason `package_flags` gives.
#[cfg(test)]
fn hold_package_reads(sub: &str, flags: &FlagArgs) {
    match sub {
        "fmt" => flags.reading_forms("package fmt"),
        "loading" => flags.reading_forms("package loading"),
        _ => {}
    }
}

pub(super) fn run_package(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = package_flags(sub, args)?;
    // Workflow #434 LAB-5B3: in test builds, the reads of `fmt` and `loading` are held to the
    // forms the register gives their flags.
    #[cfg(test)]
    hold_package_reads(sub, &flags);
    match sub {
        "validate" => package_cli::run_validate(global, &flags),
        "preflight" => package_cli::run_preflight(&flags),
        // Workflow #411 A5: the Runtime's structural admission of one package.
        "check" => package_cli::run_check(&flags),
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
            crate::dry_run_gate::refuse(
                global,
                "package run",
                Some("package dry-run"),
                Value::Null,
            )?;
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
        // Workflow #336 PKG-S5: `package digest` is retired and is an unknown package command.
        "bundle" => {
            crate::dry_run_gate::refuse(
                global,
                "package bundle",
                Some("package preflight"),
                Value::Null,
            )?;
            package_build::run_bundle(&flags)
        }
        // Workflow #407 LOAD-1: a game's loading pages copied into, or checked in, its packs.
        "loading" => package_build::run_loading(global, &flags),
        // Workflow #390 TOOL-1: the content-directory authoring tools.
        "fmt" => crate::package_tool::run_fmt(global, &flags),
        "edit" => crate::package_tool::run_edit(global, &flags),
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

/// Workflow #434 LAB-5B3: the flags `operation validate` reads. Workflow #434 LAB-5B4: each of the
/// three lists of `operation` parses its command line (`FlagArgs::parse_reading`); no positional.
pub(crate) const OPERATION_VALIDATE_READS: crate::flag_args::reads::FlagReads =
    crate::flag_args::reads::FlagReads {
        row: "operation validate",
        parts: &[&crate::flag_args::reads::ReadPart {
            options: &["--repo", "--operation-dir"],
            switches: &[],
            refused: &[],
        }],
        positionals: &[],
    };

/// Workflow #434 LAB-5B3: the flags `operation inspect` reads.
pub(crate) const OPERATION_INSPECT_READS: crate::flag_args::reads::FlagReads =
    crate::flag_args::reads::FlagReads {
        row: "operation inspect",
        parts: &[&OPERATION_DIR_READS],
        positionals: &[],
    };

/// Workflow #434 LAB-5B3: the flags `operation explain` reads, as `inspect`'s.
pub(crate) const OPERATION_EXPLAIN_READS: crate::flag_args::reads::FlagReads =
    crate::flag_args::reads::FlagReads {
        row: "operation explain",
        parts: &[&OPERATION_DIR_READS],
        positionals: &[],
    };

/// The operation directory `operation inspect` and `operation explain` read.
const OPERATION_DIR_READS: crate::flag_args::reads::ReadPart = crate::flag_args::reads::ReadPart {
    options: &["--operation-dir"],
    switches: &[],
    refused: &[],
};

/// Workflow #434 LAB-5B4: the flags of `operation <sub>`: `validate`'s, `inspect`'s and
/// `explain`'s parsed by their read lists, which refuse a word the list does not hold; `run`'s as
/// before. A function of its own for the reason `package_flags` gives.
fn operation_flags(sub: &str, args: &[String]) -> CliOutcome<FlagArgs> {
    match sub {
        "validate" => FlagArgs::parse_reading(args, &OPERATION_VALIDATE_READS),
        "inspect" => FlagArgs::parse_reading(args, &OPERATION_INSPECT_READS),
        "explain" => FlagArgs::parse_reading(args, &OPERATION_EXPLAIN_READS),
        _ => FlagArgs::parse(args),
    }
}

pub(super) fn run_operation(
    sub: &str,
    _global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    let flags = operation_flags(sub, args)?;
    match sub {
        "validate" => {
            let repo = flags.required_path("--repo")?;
            let dir = flags.required_path("--operation-dir")?;
            let report =
                crate::resource_runtime_support::resource_declarations::operation(&repo, &dir)?;
            Ok(json!({
                "operation_dir": dir.display().to_string(),
                "status": "valid",
                "report": report,
                "mode": sub
            }))
        }
        "inspect" | "explain" => {
            let dir = flags.required_path("--operation-dir")?;
            let report = validate_operation_dir(&dir)?;
            Ok(json!({
                "operation_dir": dir.display().to_string(),
                "status": "valid",
                "report": report,
                "mode": sub
            }))
        }
        // Workflow #434 LAB-2: the reserved `operation dry-run` is gone.
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
