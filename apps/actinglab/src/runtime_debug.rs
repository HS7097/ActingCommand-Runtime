// SPDX-License-Identifier: AGPL-3.0-only

use super::commands::capabilities::command_cap;
use super::contained_resources::{PackageInput, finish_package_use};
use super::{CliError, CliOutcome, FlagArgs, runtime_state_root};
use actingcommand_artifact_store::verify_evidence_archive;
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, PackageDebugRequest, ProjectionProfile,
    RuntimeEvidenceExportRequest, RuntimeResult, RuntimeSubscriptionRequest, SubscriptionCursor,
    TaskOutcome,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::fs;

const WATCH_QUERY_FLAGS: [(&str, &str); 18] = [
    ("--from-sequence", "from_sequence"),
    ("--to-sequence", "to_sequence"),
    ("--event-type", "event_type"),
    ("--minimum-severity", "minimum_severity"),
    ("--source", "source"),
    ("--origin-module", "origin_module"),
    ("--diagnostic-code", "diagnostic_code"),
    ("--instance-id", "instance_id"),
    ("--request-id", "request_id"),
    ("--correlation-id", "correlation_id"),
    ("--req", "correlation_id"),
    ("--causation-id", "causation_id"),
    ("--task-id", "task_id"),
    ("--run-id", "run_id"),
    ("--lease-id", "lease_id"),
    ("--frame-id", "frame_id"),
    ("--action-id", "action_id"),
    ("--recognition-id", "recognition_id"),
];

pub(super) fn watch_options() -> Value {
    json!({
        "query_flags": WATCH_QUERY_FLAGS.iter().map(|(flag, _)| *flag).collect::<Vec<_>>(),
        "cursor_flags": ["--after", "--wait-ms", "--max-events"],
        "req_alias": "correlation_id",
        "sequence_bounds": "inclusive; --after remains exclusive",
        "minimum_severity": "inclusive lower bound"
    })
}

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

pub(super) fn capabilities() -> [Value; 9] {
    [
        command_cap("lab status", ["running_runtime"], "available"),
        command_cap("lab receipt", ["running_runtime"], "available"),
        command_cap("lab debug-package", ["running_runtime"], "available"),
        {
            let mut command = command_cap("lab watch", ["running_runtime"], "available");
            command["options"] = watch_options();
            command
        },
        command_cap("lab export-evidence", ["running_runtime"], "available"),
        command_cap("lab replay-evidence", ["offline"], "available"),
        crate::signature_cli::capability("register"),
        crate::signature_cli::capability("match"),
        crate::signature_cli::capability("retire"),
    ]
}

pub(super) fn run_package_debug(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("lab debug-package", 0)?;
    let PreparedPackageDebug { request, reader } =
        package_debug_request(&flags, "lab debug-package")?;
    let client = runtime_lab_client()?;
    let session = client
        .begin_debug_session()
        .map_err(|error| CliError::device(error.to_string()))?;
    let receipt = session
        .debug_package(request)
        .map_err(|error| CliError::device(error.to_string()));
    let receipt = finish_package_use(receipt, reader.close())?;
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

struct PreparedPackageDebug {
    request: PackageDebugRequest,
    reader: PackageInput,
}

fn package_debug_request(flags: &FlagArgs, _command: &str) -> CliOutcome<PreparedPackageDebug> {
    let reader = PackageInput::open(flags)?;
    let request = PackageDebugRequest::new(
        reader.path().to_string_lossy().into_owned(),
        reader.reference.clone(),
    )
    .map_err(|error| CliError::usage(error.to_string()))?;
    Ok(PreparedPackageDebug { request, reader })
}

pub(super) fn runtime_lab_client() -> CliOutcome<RuntimeClient> {
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
    let PreparedPackageDebug {
        request: package,
        reader,
    } = package_debug_request(&flags, "lab export-evidence")?;
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
        .map_err(|error| CliError::device(error.to_string()));
    let package_receipt = finish_package_use(package_receipt, reader.close())?;
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
    let flags = FlagArgs::parse_values(args)?;
    flags.expect_positionals("lab watch", 0)?;
    let query = watch_query(&flags)?;
    let after_sequence = parse_u64_flag(&flags, "--after", 0)?;
    let wait_ms = parse_u64_flag(&flags, "--wait-ms", 1_000)?;
    let max_events = parse_u16_flag(&flags, "--max-events", 64)?;
    let request = RuntimeSubscriptionRequest::new(
        query.clone(),
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
        "filter": query,
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

fn watch_query(flags: &FlagArgs) -> CliOutcome<EventQuery> {
    let mut query = serde_json::Map::new();
    for (flag, values) in &flags.flags {
        if values.len() != 1 {
            return Err(CliError::usage(format!(
                "duplicate lab watch option {flag}"
            )));
        }
        if matches!(flag.as_str(), "--after" | "--wait-ms" | "--max-events") {
            continue;
        }
        let (_, field) = WATCH_QUERY_FLAGS
            .iter()
            .find(|(name, _)| *name == flag.as_str())
            .ok_or_else(|| CliError::usage(format!("unknown lab watch option {flag}")))?;
        let value = if matches!(*field, "from_sequence" | "to_sequence") {
            json!(parse_u64_flag(flags, flag, 0)?)
        } else {
            json!(values[0])
        };
        if query.insert((*field).to_owned(), value).is_some() {
            return Err(CliError::usage(
                "--req and --correlation-id select the same correlation condition; use one",
            ));
        }
    }
    serde_json::from_value(Value::Object(query))
        .map_err(|error| CliError::usage(format!("invalid lab watch query: {error}")))
}

pub(super) fn parse_u64_flag(flags: &FlagArgs, name: &str, default: u64) -> CliOutcome<u64> {
    match flags.optional(name) {
        None => Ok(default),
        Some(value) if value != "true" => value
            .parse::<u64>()
            .map_err(|error| CliError::usage(format!("failed to parse {name} '{value}': {error}"))),
        Some(_) => Err(CliError::usage(format!("missing {name} <value>"))),
    }
}

pub(super) fn parse_u16_flag(flags: &FlagArgs, name: &str, default: u16) -> CliOutcome<u16> {
    let value = parse_u64_flag(flags, name, u64::from(default))?;
    u16::try_from(value)
        .map_err(|error| CliError::usage(format!("failed to parse {name} '{value}': {error}")))
}
