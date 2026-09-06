// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CliError, CliOutcome, FlagArgs};
use actingcommand_artifact_store::{read_projected_verified, verify_projected_read_only};
use actingcommand_contract::{
    ArtifactKind, ContainedLabOperationResult, EventQuery, EventType,
    LAB_OPERATION_PREPARED_SCHEMA, LAB_OPERATION_TERMINAL_SCHEMA, LabEvidenceReference,
    LabOperationEvidence, LabOperationPrepared, LabOperationRecord,
    ProjectionPayload, ProjectionProfile, RequestId, RuntimeDebugOperation, TerminalEvent,
    verify_lab_operation_evidence,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerReadOnly, GlobalLedgerReadOnlyConfig, project_subscription_event,
};
use actingcommand_pack_containment::{Containment, InstanceId, Sha256Hash};
use actingcommand_resource_tooling::{
    ResourceRestoreRecord, ResourceRestoreRequest, materialize_authoring_draft,
    open_published_package, restore_authoring_draft,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

pub(super) fn run_resource_restore(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_values(args)?;
    flags.expect_positionals("resource restore", 0)?;
    for (name, values) in &flags.flags {
        if !matches!(
            name.as_str(),
            "--repo"
                | "--state-root"
                | "--request-id"
                | "--through-sequence"
                | "--zip"
                | "--expected-sha256"
                | "--task-id"
                | "--entry-page"
                | "--target-page"
                | "--goal"
        ) || (values.len() != 1 && !matches!(name.as_str(), "--request-id" | "--target-page"))
        {
            return Err(CliError::usage(
                "resource restore has an unknown or repeated flag",
            ));
        }
    }
    let repo = flags.required_path("--repo")?;
    match fs::symlink_metadata(&repo) {
        Ok(_) => {
            return Err(CliError::usage(
                "resource restore --repo must be a new directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::package_invalid(format!(
                "inspect draft root: {error}"
            )));
        }
    }
    let root = flags.required_path("--state-root")?;
    let through = flags
        .required("--through-sequence")?
        .parse::<u64>()
        .map_err(|_| CliError::usage("invalid --through-sequence"))?;
    if through == 0 {
        return Err(CliError::usage("--through-sequence must be positive"));
    }
    let raw_requests = flags.values("--request-id");
    if !(1..=32).contains(&raw_requests.len()) {
        return Err(CliError::usage(
            "resource restore requires 1–32 unique request IDs",
        ));
    }
    let mut requests = BTreeSet::new();
    for value in raw_requests {
        let id: RequestId = serde_json::from_value(Value::String(value))
            .map_err(|_| CliError::usage("invalid --request-id"))?;
        if !requests.insert(id) {
            return Err(CliError::usage("duplicate --request-id"));
        }
    }
    let expected = Sha256Hash::parse_hex(&flags.required("--expected-sha256")?)
        .map_err(|_| CliError::usage("invalid --expected-sha256"))?;
    let package = open_published_package(&flags.required_path("--zip")?)?;
    let bytes = package.read_all()?;
    let mut containment = Containment::for_metadata_validation();
    let bundle = containment
        .load_observation(
            &InstanceId::new("resource-restore")
                .map_err(|_| restore_error("invalid containment identity"))?,
            &bytes,
            &expected,
        )
        .map_err(|error| {
            CliError::package_invalid(format!("resource restore package admission: {error}"))
        })?;

    let mut artifact_failure = None;
    let snapshot = GlobalLedger::open_read_only(
        GlobalLedgerReadOnlyConfig::new(root.join("ledger")),
        |reference| match verify_projected_read_only(&root, reference) {
            Ok(verified) => Some(verified),
            Err(error) => {
                artifact_failure.get_or_insert(error.code());
                None
            }
        },
    )
    .map_err(|error| CliError::package_invalid(format!("resource restore ledger: {error}")))?;
    if let Some(code) = artifact_failure {
        return Err(CliError::package_invalid(format!(
            "resource restore artifact verification: {code}"
        )));
    }
    if snapshot.corrupt_tail().is_some() || through > snapshot.latest_sequence() {
        return Err(restore_error(
            "requested ledger snapshot is incomplete or corrupt",
        ));
    }
    let mut records = Vec::new();
    let mut gaps = Vec::new();
    let mut instance = None;
    for request_id in requests {
        let events = restore_event_page(
            &snapshot,
            &EventQuery {
                request_id: Some(request_id),
                ..EventQuery::default()
            },
            through,
        )?;
        if events.is_empty() {
            return Err(restore_error("requested native request is absent"));
        }
        let mut artifacts = BTreeMap::new();
        let mut stored_prepared = None;
        let mut prepared_reference = None;
        let mut terminal_record = None;
        for event in events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
        {
            for reference in &event.artifacts {
                if reference.kind == ArtifactKind::DiagnosticJson
                    && reference.byte_count > 256 * 1024
                {
                    return Err(restore_error(
                        "Lab diagnostic artifact exceeds its original budget",
                    ));
                }
                let bytes = read_projected_verified(&root, reference).map_err(|error| {
                    CliError::package_invalid(format!("read Lab artifact: {}", error.code()))
                })?;
                if reference.kind == ArtifactKind::DiagnosticJson {
                    let value: Value = serde_json::from_slice(&bytes)
                        .map_err(|_| restore_error("Lab diagnostic JSON cannot be decoded"))?;
                    if value["schema_version"] == LAB_OPERATION_PREPARED_SCHEMA {
                        let prepared: LabOperationPrepared = serde_json::from_value(value)
                            .map_err(|_| restore_error("Lab prepared record cannot be decoded"))?;
                        if reference.frame_id.is_some()
                            || event.links.correlation_id() != Some(&prepared.correlation_id)
                            || event.links.instance_id() != Some(&prepared.instance_id)
                            || event.links.lease_id() != prepared.lease_id.as_ref()
                            || reference.correlation_id != Some(prepared.correlation_id)
                        {
                            return Err(restore_error("Lab prepared artifact identity mismatch"));
                        }
                        if stored_prepared.replace(prepared).is_some() {
                            return Err(restore_error("multiple Lab prepared records"));
                        }
                        prepared_reference = Some(json!({"sequence":event.sequence,
                            "event_id":event.event_id,"artifact_id":reference.artifact_id,
                            "sha256":reference.sha256}));
                    } else if value["schema_version"] == LAB_OPERATION_TERMINAL_SCHEMA {
                        let record: LabOperationRecord = serde_json::from_value(value)
                            .map_err(|_| restore_error("Lab operation record cannot be decoded"))?;
                        let operation = ContainedLabOperationResult {
                            record,
                            terminal_artifact: LabEvidenceReference {
                                artifact: reference.clone(),
                                verified: TerminalEvent {
                                    sequence: event.sequence,
                                    event_id: event.event_id,
                                },
                            },
                        };
                        if terminal_record.replace(operation).is_some() {
                            return Err(restore_error("multiple Lab operation terminal records"));
                        }
                    }
                }
                if artifacts.insert(reference.artifact_id, bytes).is_some() {
                    return Err(restore_error("duplicate native ArtifactVerified identity"));
                }
            }
        }
        let prepared = stored_prepared
            .as_ref()
            .ok_or_else(|| restore_error("native Lab package identity is unavailable"))?;
        if prepared.request_id != request_id
            || prepared.expected_package_sha256 != expected.to_string()
            || prepared.actual_package_sha256 != expected.to_string()
            || instance.is_some_and(|value| value != prepared.instance_id)
            || events.iter().any(|event| {
                event.links.correlation_id() != Some(&prepared.correlation_id)
            })
        {
            return Err(restore_error(
                "request instance or external package hash mismatch",
            ));
        }
        instance = Some(prepared.instance_id);
        let native_inputs = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::InputIntent | EventType::InputCommitted | EventType::InputFailed
                )
            })
            .map(|event| {
                let effect = match &event.payload {
                    ProjectionPayload::Full(payload) => payload.effect_disposition(),
                    _ => None,
                };
                json!({"sequence":event.sequence,"event_id":event.event_id,
                    "action_id":event.links.action_id(),"event_type":event.event_type,
                    "effect":effect})
            })
            .collect::<Vec<_>>();
        let Some(operation) = terminal_record else {
            gaps.push(json!({"request_id":request_id, "code":"lab_terminal_record_missing",
                "correlation_id":prepared.correlation_id,"instance_id":prepared.instance_id,
                "lease_id":prepared.lease_id,"package_sha256":prepared.actual_package_sha256,
                "prepared":prepared_reference,"native_inputs":native_inputs}));
            continue;
        };
        if operation.record.prepared != *prepared {
            return Err(restore_error("stored Lab prepared and terminal content differ"));
        }
        let terminals = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::CommandValidated | EventType::CommandRejected
                ) && matches!(&event.payload, ProjectionPayload::Full(payload)
                    if payload.action() == RuntimeDebugOperation::Do.event_action())
            })
            .collect::<Vec<_>>();
        if terminals.is_empty() {
            gaps.push(
                json!({"request_id":request_id,"code":"native_command_terminal_missing",
                "record_sequence":operation.terminal_artifact.verified.sequence,
                "record_event_id":operation.terminal_artifact.verified.event_id,
                "record_sha256":operation.terminal_artifact.artifact.sha256,
                "prepared":prepared_reference,"native_inputs":native_inputs}),
            );
            continue;
        }
        if terminals.len() != 1 {
            return Err(restore_error("native command terminal is ambiguous"));
        }
        let terminal = TerminalEvent {
            sequence: terminals[0].sequence,
            event_id: terminals[0].event_id,
        };
        let lease_events = match prepared.lease_id {
            Some(lease) => restore_event_page(
                &snapshot,
                &EventQuery {
                    lease_id: Some(lease),
                    ..EventQuery::default()
                },
                through,
            )?,
            None => Vec::new(),
        };
        let action_events = match operation.record.input_action_id {
            Some(action) => restore_event_page(
                &snapshot,
                &EventQuery {
                    action_id: Some(action),
                    ..EventQuery::default()
                },
                through,
            )?,
            None => Vec::new(),
        };
        let evidence = LabOperationEvidence {
            operation,
            terminal,
            events,
            lease_events,
            action_events,
            artifacts,
        };
        verify_lab_operation_evidence(&evidence).map_err(|error| {
            CliError::package_invalid(format!("Lab evidence consistency: {}", error.code()))
        })?;
        let input_event_type = evidence.operation.record.input_event.and_then(|terminal| {
            evidence
                .events
                .iter()
                .find(|event| {
                    event.sequence == terminal.sequence && event.event_id == terminal.event_id
                })
                .map(|event| event.event_type)
        });
        records.push(ResourceRestoreRecord {
            operation: evidence.operation,
            terminal,
            input_event_type,
        });
    }
    records.sort_by_key(|input| {
        input
            .operation
            .record
            .input_intent
            .map(|event| event.sequence)
            .unwrap_or(input.operation.record.prepared_artifact.verified.sequence)
    });
    let request = ResourceRestoreRequest {
        task_id: flags.required("--task-id")?,
        through_sequence: through,
        entry_page: flags.optional("--entry-page"),
        target_pages: flags.values("--target-page"),
        goal: flags.optional("--goal"),
    };
    let restored = restore_authoring_draft(bundle, &records, &gaps, &request)?;
    let parent = repo
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| CliError::package_invalid(format!("create draft parent: {error}")))?;
    fs::create_dir(&repo).map_err(|error| {
        CliError::package_invalid(format!("claim new draft directory: {error}"))
    })?;
    materialize_authoring_draft(&repo, &restored.draft)?;
    let mut report = restored.report;
    report["repo"] = json!(repo);
    Ok(report)
}

fn restore_event_page(
    snapshot: &GlobalLedgerReadOnly,
    query: &EventQuery,
    through: u64,
) -> CliOutcome<Vec<actingcommand_contract::ProjectedEvent>> {
    let mut events = Vec::new();
    let mut after = 0;
    loop {
        let page = snapshot
            .query_page(query, after, through, 256)
            .map_err(|error| CliError::package_invalid(format!("read Lab event page: {error}")))?;
        if page.is_empty() {
            return Ok(events);
        }
        if events.len() + page.len() > 4096 {
            return Err(restore_error(
                "Lab evidence exceeds 4096 events per identity query",
            ));
        }
        for event in &page {
            let projected = project_subscription_event(event, query, ProjectionProfile::Forensic)
                .ok_or_else(|| {
                restore_error("native event projection omitted a requested event")
            })?;
            after = projected.sequence;
            events.push(projected);
        }
        if page.len() < 256 {
            return Ok(events);
        }
    }
}

fn restore_error(message: &str) -> CliError {
    CliError::package_invalid(format!("resource restore: {message}"))
}
