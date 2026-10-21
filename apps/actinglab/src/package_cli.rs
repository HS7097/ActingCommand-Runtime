// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions, attach_package_event};
use actingcommand_contract::outcome::Detail;
use actingcommand_lab::{PackageValidateRequest, PackageValidationResponse};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

#[path = "package_offline.rs"]
pub(super) mod offline;

/// Workflow #411 A5: how long one `package check` may read its package, the budget of the
/// Runtime's own pack admission (`runtime-host` `pack_admission.rs`).
const CHECK_BUDGET: Duration = Duration::from_secs(60);
/// The instance label the check admits a package under: admission depends only on the package
/// and the kernel, not on an instance.
const CHECK_LABEL: &str = "package-check";

/// Workflow #411 A5 (M-PKG §3.2, S5-T4): the answer of `package check`, a package the Runtime's
/// structural admission admitted, named by what its control declares.
#[derive(Debug, Serialize)]
struct PackageCheckResponse {
    status: &'static str,
    package_id: String,
    entry_task_id: String,
}

/// Workflow #411 A5 (M-PKG §3.2, S5-T4: "`package check` 只做结构准入"): `package check
/// --package <path>` admits the package exactly as the Runtime admits a pack in check-config and
/// in the daemon, through `PreparedContainedTask::describe_path` with the metadata-only vision
/// capability, so the verdict never depends on a local OCR install. A refusal is
/// `package_invalid` with `details.code`, the admission's own code. It only reads.
pub(super) fn run_check(flags: &FlagArgs) -> CliOutcome<Value> {
    if let Some(flag) = flags.flags.keys().find(|flag| flag.as_str() != "--package") {
        return Err(CliError::usage(format!(
            "package check does not accept {flag}"
        )));
    }
    flags.expect_positionals("package check", 0)?;
    let package = flags.required_path("--package")?;
    // A relative path names the package below the working directory, as every `--package` does.
    let package = if package.is_absolute() {
        package
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::package_invalid(error.to_string()))?
            .join(package)
    };
    let deadline = Instant::now()
        .checked_add(CHECK_BUDGET)
        .ok_or_else(|| CliError::package_invalid("package admission deadline overflow"))?;
    let descriptor =
        actingcommand_lab::PreparedContainedTask::describe_path(CHECK_LABEL, &package, deadline)
            .map_err(|error| {
                CliError::package_invalid(format!(
                    "{}: {}",
                    error.code(),
                    error.detail().unwrap_or("package admission failed")
                ))
                .with_details(json!({
                    "code": error.code().as_str(),
                    "declaration": error.declaration_issue(),
                    // Workflow #411 A39: the admission's outcome with the values it carries.
                    "outcome": error.outcome().envelope(Detail::Full),
                }))
            })?;
    serialize_response(PackageCheckResponse {
        status: "admitted",
        package_id: descriptor.package_id().to_owned(),
        entry_task_id: descriptor.entry_task_id().to_owned(),
    })
}

// Workflow #411 A5 (apps key 71): `package check` gives the Runtime's verdict.
#[cfg(test)]
#[path = "tests/gate_0121_pkgcheck.rs"]
mod gate_0121_pkgcheck;

// Workflow #411 slice E1 (train 0.12.9), T1 (A39): `package check` and `package preflight` name
// the rule that refused a package and where.
#[cfg(test)]
#[path = "tests/gate_411_e1.rs"]
mod gate_411_e1;

pub(super) fn is_offline_command(args: &[String]) -> bool {
    matches!(args, [group, command, ..] if group == "package" && matches!(command.as_str(), "dry-run" | "preflight"))
}

/// Workflow #336 PKG-S4 (option 丙): the package by its path only (`--package` or `--zip`);
/// the answer names it by its `package_id`, never a digest.
pub(super) fn run_preflight(flags: &FlagArgs) -> CliOutcome<Value> {
    for name in flags.flags.keys() {
        if !matches!(name.as_str(), "--package" | "--zip") {
            return Err(CliError::usage(format!(
                "package preflight does not accept {name}"
            )));
        }
    }
    let prepared = preflight_admission(flags)?;
    Ok(json!({
        "schema_version":"actingcommand.package-preflight.v1", "status":"prepared", "executed":false,
        "task_id":prepared.task_label(), "package_label":prepared.package_label(),
        "coverage":{"declaration_parse":"passed","package_load":"passed","task_prepare":"passed","recognition_execution":"not_run","execution":"not_run"},
        "effective_timing":prepared.effective_timing(), "production_global_ledger_written":false
    }))
}

/// The admission `package preflight` answers for: the package `--package` or `--zip` names,
/// loaded with every file and asset, then its entry task prepared. Workflow #411 RV-1:
/// `resource verify` admits every pack with it.
pub(super) fn preflight_admission(
    flags: &FlagArgs,
) -> CliOutcome<actingcommand_lab::PreparedContainedTask> {
    let resources = super::contained_resources::load(flags, "package-preflight").map_err(|error| {
        error.with_details(json!({"coverage":{"declaration_parse":"not_completed","package_load":"failed","task_prepare":"not_run","execution":"not_run"}}))
    })?;
    let bundle = std::sync::Arc::try_unwrap(resources)
        .map_err(|_| CliError::package_invalid("preflight package ownership is shared"))?;
    actingcommand_lab::PreparedContainedTask::from_bundle(bundle).map_err(|error| {
        CliError::package_invalid(format!("{}: {}", error.code(), error.detail().unwrap_or("task preparation failed")))
            .with_details(json!({"code":error.code().as_str(),"declaration":error.declaration_issue(),"outcome":error.outcome().envelope(Detail::Full),"coverage":{"declaration_parse":"passed","package_load":"passed","task_prepare":"failed","execution":"not_run"}}))
    })
}

pub(super) fn run_offline(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    offline::run_dry_run(global, flags)
}

pub(super) fn offline_capability() -> Value {
    offline::capability()
}

/// Workflow #336 PKG-S4 (option 丙): the package by its path; no hash is taken or reported.
pub(super) fn run_validate(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let zip = flags.required_path("--zip")?;
    let validation = validate_package(&zip, false)?;
    let mut payload = serialize_response(&validation)?;
    payload["coverage"] =
        json!({"package_format":"passed","task_prepare":"not_run","execution":"not_run"});
    attach_package_event(
        global,
        "package.validate.ok",
        "package-validate",
        &zip,
        &validation,
        &mut payload,
    )?;
    Ok(payload)
}

pub(super) fn validate_package(
    zip_path: &Path,
    include_entries: bool,
) -> CliOutcome<PackageValidationResponse> {
    let mut lab = super::env_detection::build_readonly_lab()?;
    lab.package_validate(PackageValidateRequest {
        zip_path: zip_path.to_path_buf(),
        include_entries,
    })
}

pub(super) fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
