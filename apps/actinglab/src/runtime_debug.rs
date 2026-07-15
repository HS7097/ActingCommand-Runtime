// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, command_cap, runtime_state_root};
use actingcommand_artifact_store::verify_evidence_archive;
use actingcommand_contract::{
    CorrelationId, EventActor, EventQuery, EventSource, PackageDebugRequest, ProjectionProfile,
    RuntimeEvidenceExportRequest, RuntimeResult, RuntimeSubscriptionRequest, SubscriptionCursor,
    TaskOutcome,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_resource_tooling::resolve_published_package_path;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::fs;

pub(super) fn run_runtime_debug(subcommand: &str, args: &[String]) -> CliOutcome<Value> {
    match subcommand {
        "debug-package" => run_package_debug(args),
        "watch" => run_watch(args),
        "export-evidence" => run_export_evidence(args),
        "replay-evidence" => run_replay_evidence(args),
        _ => Err(CliError::usage(format!(
            "unknown Runtime-backed lab command: {subcommand}"
        ))),
    }
}

pub(super) fn capabilities() -> [Value; 6] {
    [
        command_cap("lab status", ["running_runtime"], "available"),
        command_cap("lab receipt", ["offline"], "available"),
        command_cap("lab debug-package", ["running_runtime"], "available"),
        command_cap("lab watch", ["running_runtime"], "available"),
        command_cap("lab export-evidence", ["running_runtime"], "available"),
        command_cap("lab replay-evidence", ["offline"], "available"),
    ]
}

pub(super) fn run_package_debug(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("lab debug-package", 0)?;
    let request = package_debug_request(&flags, "lab debug-package")?;
    let client = runtime_lab_client()?;
    let session = client
        .begin_debug_session()
        .map_err(|error| CliError::device(error.to_string()))?;
    let receipt = session
        .debug_package(request)
        .map_err(|error| CliError::device(error.to_string()))?;
    let summary = match receipt.result() {
        Some(RuntimeResult::PackageDebugCompleted { summary }) => summary,
        _ => {
            return Err(CliError::device(
                "Runtime returned an invalid package debug receipt",
            ));
        }
    };
    let events = session
        .query_events(ProjectionProfile::Lab)
        .map_err(|error| CliError::device(error.to_string()))?;
    Ok(json!({
        "schema_version": "actingcommand.lab.package-debug.v1",
        "authority": "runtime",
        "correlation_id": session.correlation_id(),
        "summary": summary,
        "terminal_receipt": receipt,
        "events": events
    }))
}

fn package_debug_request(flags: &FlagArgs, command: &str) -> CliOutcome<PackageDebugRequest> {
    let package = flags.required_path("--zip")?;
    let package = resolve_published_package_path(&package)?;
    let package = fs::canonicalize(&package).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to resolve debug package {}: {error}",
            package.display()
        ))
    })?;
    let expected = flags
        .optional("--expected-sha256")
        .filter(|value| value != "true")
        .ok_or_else(|| {
            CliError::usage(format!(
                "{command} requires --expected-sha256 <sha256> from an external trust source"
            ))
        })?;
    let expected = Sha256Hash::parse_hex(&expected)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    PackageDebugRequest::new(package.to_string_lossy().into_owned(), expected.to_string())
        .map_err(|error| CliError::usage(error.to_string()))
}

fn runtime_lab_client() -> CliOutcome<RuntimeClient> {
    RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(|error| CliError::device(error.to_string()))
}

fn run_export_evidence(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("lab export-evidence", 0)?;
    let package = package_debug_request(&flags, "lab export-evidence")?;
    let output = flags.required_path("--out")?;
    let task_outcome = parse_task_outcome(flags.optional("--outcome").as_deref())?;
    let export =
        RuntimeEvidenceExportRequest::new(output.to_string_lossy().into_owned(), task_outcome)
            .map_err(|error| CliError::usage(error.to_string()))?;
    let client = runtime_lab_client()?;
    let session = client
        .begin_debug_session()
        .map_err(|error| CliError::device(error.to_string()))?;
    let package_receipt = session
        .debug_package(package)
        .map_err(|error| CliError::device(error.to_string()))?;
    if !matches!(
        package_receipt.result(),
        Some(RuntimeResult::PackageDebugCompleted { .. })
    ) {
        return Err(CliError::device(
            "Runtime returned an invalid package debug receipt",
        ));
    }
    let receipt = session
        .export_evidence(export)
        .map_err(|error| CliError::device(error.to_string()))?;
    let summary = match receipt.result() {
        Some(RuntimeResult::EvidenceExportCompleted { summary }) => summary,
        _ => {
            return Err(CliError::device(
                "Runtime returned an invalid evidence export receipt",
            ));
        }
    };
    let events = session
        .query_events(ProjectionProfile::Lab)
        .map_err(|error| CliError::device(error.to_string()))?;
    Ok(json!({
        "schema_version": "actingcommand.lab.evidence-export.v1",
        "authority": "runtime",
        "correlation_id": session.correlation_id(),
        "summary": summary,
        "terminal_receipt": receipt,
        "events": events
    }))
}

fn run_replay_evidence(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("lab replay-evidence", 0)?;
    let archive = flags.required_path("--zip")?;
    let expected = flags
        .optional("--expected-sha256")
        .filter(|value| value != "true")
        .ok_or_else(|| {
            CliError::usage(
                "lab replay-evidence requires --expected-sha256 <sha256> from an external receipt",
            )
        })?;
    let verification = verify_evidence_archive(&archive, &expected)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    Ok(json!({
        "schema_version": "actingcommand.lab.evidence-replay.v1",
        "authority": "sealed_offline_verifier",
        "zip_byte_count": verification.zip_byte_count,
        "zip_sha256": verification.zip_sha256,
        "manifest_sha256": verification.manifest_sha256,
        "manifest": verification.manifest,
    }))
}

fn parse_task_outcome(value: Option<&str>) -> CliOutcome<TaskOutcome> {
    match value.unwrap_or("success") {
        "success" => Ok(TaskOutcome::Success),
        "failure" => Ok(TaskOutcome::Failure),
        "cancelled" => Ok(TaskOutcome::Cancelled),
        value => Err(CliError::usage(format!(
            "unsupported evidence task outcome: {value}"
        ))),
    }
}

pub(super) fn run_watch(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("lab watch", 0)?;
    let after_sequence = parse_u64_flag(&flags, "--after", 0)?;
    let wait_ms = parse_u64_flag(&flags, "--wait-ms", 1_000)?;
    let max_events = parse_u16_flag(&flags, "--max-events", 64)?;
    let correlation_id = flags
        .optional("--req")
        .filter(|value| value != "true")
        .map(|value| {
            serde_json::from_value::<CorrelationId>(json!(value)).map_err(|_| {
                CliError::usage("lab watch --req must be a Runtime correlation identifier")
            })
        })
        .transpose()?;
    let request = RuntimeSubscriptionRequest::new(
        EventQuery {
            correlation_id,
            ..EventQuery::default()
        },
        ProjectionProfile::Lab,
        SubscriptionCursor { after_sequence },
        wait_ms,
        max_events,
    )
    .map_err(|error| CliError::usage(error.to_string()))?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(|error| CliError::device(error.to_string()))?;
    let batch = client
        .subscribe_events(request)
        .map_err(|error| CliError::device(error.to_string()))?;
    let latest = batch.events().last().map(|event| {
        json!({
            "sequence": event.sequence,
            "event_type": event.event_type,
            "severity": event.severity,
            "correlation_id": event.links.correlation_id(),
        })
    });
    Ok(json!({
        "schema_version": "actingcommand.lab.watch.v1",
        "authority": "runtime_global_ledger",
        "filter": { "correlation_id": correlation_id },
        "progress": {
            "state": if batch.timed_out() { "idle" } else { "advanced" },
            "after_sequence": after_sequence,
            "next_sequence": batch.next_cursor().after_sequence,
            "event_count": batch.events().len(),
            "latest": latest,
        },
        "events": batch.events(),
    }))
}

fn parse_u64_flag(flags: &FlagArgs, name: &str, default: u64) -> CliOutcome<u64> {
    match flags.optional(name) {
        None => Ok(default),
        Some(value) if value != "true" => value
            .parse::<u64>()
            .map_err(|error| CliError::usage(format!("failed to parse {name} '{value}': {error}"))),
        Some(_) => Err(CliError::usage(format!("missing {name} <value>"))),
    }
}

fn parse_u16_flag(flags: &FlagArgs, name: &str, default: u16) -> CliOutcome<u16> {
    let value = parse_u64_flag(flags, name, u64::from(default))?;
    u16::try_from(value)
        .map_err(|error| CliError::usage(format!("failed to parse {name} '{value}': {error}")))
}
