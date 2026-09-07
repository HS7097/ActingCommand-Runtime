// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_artifact_store::{ArtifactReader, open_projected_stream};
use actingcommand_contract::{
    MAX_TASK_DIAGNOSTIC_PAGE_RECORDS, MAX_TASK_DIAGNOSTIC_RECORD_BYTES, TASK_DIAGNOSTIC_SCHEMA,
    TaskDiagnosticCursor, TaskDiagnosticHeader, TaskDiagnosticRecord,
};
use std::io::{BufRead, BufReader, Read};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecordsRequest {
    pub cursor: Option<TaskDiagnosticCursor>,
    pub limit: usize,
    pub include_private: bool,
}

impl Default for TaskRecordsRequest {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: 16,
            include_private: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskDiagnosticPage {
    pub source_sequence: u64,
    pub artifact: ProjectedArtifactReference,
    pub state: &'static str,
    pub schema_version: Option<String>,
    pub header: Option<TaskDiagnosticHeader>,
    pub records: Vec<TaskDiagnosticRecord>,
    pub total_records: Option<u64>,
    pub next_cursor: Option<TaskDiagnosticCursor>,
    pub legacy: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskDiagnosticGap {
    pub run_id: actingcommand_contract::RunId,
    pub state: &'static str,
    pub record_count: Option<u64>,
}

fn invalid(code: &'static str) -> ForensicError {
    ForensicError::new(
        code,
        "read_task_diagnostic",
        "task diagnostic data withheld; see its native reference",
    )
}

fn line(reader: &mut BufReader<ArtifactReader>) -> ForensicResult<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_TASK_DIAGNOSTIC_RECORD_BYTES + 3) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| invalid("task_diagnostic_read_failed"))?;
    if bytes.len() > MAX_TASK_DIAGNOSTIC_RECORD_BYTES + 2 {
        return Err(invalid("task_diagnostic_record_limit"));
    }
    Ok(bytes)
}

/// Identifies this schema before applying an unrelated legacy document's size/privacy rules.
pub(super) fn is_task_stream(
    root: &Path,
    artifact: &ProjectedArtifactReference,
) -> ForensicResult<bool> {
    let mut reader = BufReader::new(
        open_projected_stream(root, artifact)
            .map_err(|_| invalid("task_diagnostic_open_failed"))?,
    );
    let mut first = Vec::new();
    (&mut reader)
        .take(2049)
        .read_until(b'\n', &mut first)
        .map_err(|_| invalid("task_diagnostic_read_failed"))?;
    if first.len() > 2048 {
        return Ok(false);
    }
    let Some(header) = first.strip_suffix(b",\"records\":[\n") else {
        return Ok(false);
    };
    let mut bytes = header.to_vec();
    bytes.push(b'}');
    let header: TaskDiagnosticHeader =
        serde_json::from_slice(&bytes).map_err(|_| invalid("task_diagnostic_header_invalid"))?;
    if header.schema_version != TASK_DIAGNOSTIC_SCHEMA {
        return Ok(false);
    }
    reader
        .into_inner()
        .finish()
        .map_err(|_| invalid("task_diagnostic_integrity_failed"))?;
    Ok(true)
}

fn page(
    root: &Path,
    event: &PersistedEvent,
    artifact: &ProjectedArtifactReference,
    options: &TaskRecordsRequest,
    limit: usize,
) -> ForensicResult<TaskDiagnosticPage> {
    let mut output = TaskDiagnosticPage {
        source_sequence: event.sequence(),
        artifact: artifact.clone(),
        state: "privacy_withheld",
        schema_version: None,
        header: None,
        records: Vec::new(),
        total_records: None,
        next_cursor: None,
        legacy: None,
    };
    if artifact.redaction_state == ArtifactRedactionState::Pending && !options.include_private {
        return Ok(output);
    }
    let mut reader = BufReader::new(
        open_projected_stream(root, artifact)
            .map_err(|_| invalid("task_diagnostic_open_failed"))?,
    );
    let first = line(&mut reader)?;
    if let Some(header_bytes) = first.strip_suffix(b",\"records\":[\n") {
        let mut header_bytes = header_bytes.to_vec();
        header_bytes.push(b'}');
        let value: serde_json::Value = serde_json::from_slice(&header_bytes)
            .map_err(|_| invalid("task_diagnostic_header_invalid"))?;
        let schema = value
            .get("schema_version")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        output.schema_version = schema.clone();
        if schema.as_deref() != Some(TASK_DIAGNOSTIC_SCHEMA) {
            reader
                .into_inner()
                .finish()
                .map_err(|_| invalid("task_diagnostic_integrity_failed"))?;
            output.state = "unknown_schema";
            return Ok(output);
        }
        let header: TaskDiagnosticHeader =
            serde_json::from_value(value).map_err(|_| invalid("task_diagnostic_header_invalid"))?;
        if event.links().request_id() != Some(&header.request_id)
            || event.links().correlation_id() != Some(&header.correlation_id)
            || event.links().task_id() != Some(&header.task_id)
            || event.links().run_id() != Some(&header.run_id)
            || event.links().instance_id() != Some(&header.instance_id)
            || event.links().lease_id() != Some(&header.lease_id)
        {
            return Err(invalid("task_diagnostic_identity_mismatch"));
        }
        let after = options
            .cursor
            .as_ref()
            .map_or(0, |cursor| cursor.after_index);
        let mut total = 0_u64;
        loop {
            let bytes = line(&mut reader)?;
            if bytes == b"]}\n" {
                break;
            }
            if bytes.is_empty() {
                return Err(invalid("task_diagnostic_unsealed"));
            }
            let bytes = if total == 0 {
                bytes.as_slice()
            } else {
                bytes
                    .strip_prefix(b",")
                    .ok_or_else(|| invalid("task_diagnostic_framing_invalid"))?
            };
            let record: TaskDiagnosticRecord = serde_json::from_slice(bytes)
                .map_err(|_| invalid("task_diagnostic_record_invalid"))?;
            total = total
                .checked_add(1)
                .ok_or_else(|| invalid("task_diagnostic_record_limit"))?;
            if record.index != total
                || record
                    .parent_index
                    .is_some_and(|parent| parent == 0 || parent >= total)
            {
                return Err(invalid("task_diagnostic_record_order_invalid"));
            }
            if record.index > after && output.records.len() < limit {
                output.records.push(record);
            }
        }
        if !line(&mut reader)?.is_empty() {
            return Err(invalid("task_diagnostic_trailing_data"));
        }
        reader
            .into_inner()
            .finish()
            .map_err(|_| invalid("task_diagnostic_integrity_failed"))?;
        if after > total {
            return Err(invalid("task_diagnostic_cursor_invalid"));
        }
        let last = output.records.last().map_or(after, |record| record.index);
        output.next_cursor = (last < total).then(|| TaskDiagnosticCursor {
            artifact_id: artifact.artifact_id,
            sha256: artifact.sha256.clone(),
            after_index: last,
        });
        output.total_records = Some(total);
        output.header = Some(header);
        output.state = "verified";
    } else {
        // Existing diagnostic documents retain their native schema and source reference.
        let mut bytes = first;
        while bytes.len() <= MAX_TASK_DIAGNOSTIC_RECORD_BYTES {
            let next = line(&mut reader)?;
            if next.is_empty() {
                break;
            }
            bytes.extend_from_slice(&next);
        }
        if bytes.len() > MAX_TASK_DIAGNOSTIC_RECORD_BYTES {
            return Err(invalid("legacy_diagnostic_read_limit"));
        }
        reader
            .into_inner()
            .finish()
            .map_err(|_| invalid("task_diagnostic_integrity_failed"))?;
        let document: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| invalid("legacy_diagnostic_invalid"))?;
        let schema = document
            .get("schema_version")
            .and_then(|value| value.as_str());
        output.schema_version = schema.map(str::to_owned);
        let known = schema.is_some_and(|schema| {
            matches!(
                schema,
                "actingcommand.runtime.effective-task-configuration.v1"
                    | "actingcommand.runtime.post-admission-ocr-observation.v1"
                    | "actingcommand.runtime.post-admission-ocr-comparison-envelope.v1"
                    | "actingcommand.runtime.post-admission-ocr-failure.v1"
                    | "actingcommand.runtime.contained-task-stability-comparison.v1"
                    | "actingcommand.runtime.contained-page-observation.v1"
                    | "actingcommand.runtime.lab-operation-prepared.v1"
                    | "actingcommand.runtime.lab-operation-terminal.v1"
            )
        });
        output.state = if known {
            "verified_legacy"
        } else {
            "unknown_schema"
        };
        if known {
            let after = options
                .cursor
                .as_ref()
                .map_or(0, |cursor| cursor.after_index);
            if after > 1 {
                return Err(invalid("task_diagnostic_cursor_invalid"));
            }
            output.total_records = Some(1);
            if after == 0 && limit > 0 {
                output.legacy = Some(document);
            }
            if after == 0 && limit == 0 {
                output.state = "outside_record_page";
                output.next_cursor = Some(TaskDiagnosticCursor {
                    artifact_id: artifact.artifact_id,
                    sha256: artifact.sha256.clone(),
                    after_index: 0,
                });
            }
        }
    }
    Ok(output)
}

pub(super) fn expand(
    root: &Path,
    options: &TaskRecordsRequest,
    report: &mut TaskEvidenceReport,
) -> ForensicResult<()> {
    if options.limit == 0 || options.limit > MAX_TASK_DIAGNOSTIC_PAGE_RECORDS {
        return Err(invalid("task_diagnostic_page_limit"));
    }
    let mut remaining = options.limit;
    let mut cursor_found = options.cursor.is_none();
    let mut covered = std::collections::BTreeSet::new();
    for event in &report.page.events {
        if event.event_type() != EventType::ArtifactVerified {
            continue;
        }
        for reference in event.artifacts() {
            if reference.kind() != ArtifactKind::DiagnosticJson {
                continue;
            }
            let artifact = reference.project(true);
            if let Some(cursor) = &options.cursor {
                if cursor.artifact_id != artifact.artifact_id {
                    continue;
                }
                if cursor.sha256 != artifact.sha256 {
                    return Err(invalid("task_diagnostic_cursor_mismatch"));
                }
                cursor_found = true;
            }
            match page(root, event, &artifact, options, remaining) {
                Ok(page) => {
                    remaining = remaining
                        .saturating_sub(page.records.len() + usize::from(page.legacy.is_some()));
                    if page.schema_version.as_deref() == Some(TASK_DIAGNOSTIC_SCHEMA)
                        && let Some(run) = event.links().run_id()
                    {
                        covered.insert(*run);
                    }
                    if page.state == "unknown_schema" {
                        report.gaps.push("unknown_diagnostic_schema");
                    }
                    report.diagnostics.push(page);
                }
                Err(error) => {
                    report.gaps.push("task_diagnostic_unverifiable");
                    report.failures.push(StabilityFailure {
                        source_sequence: Some(event.sequence()),
                        artifact,
                        code: error.code(),
                        operation: error.operation(),
                    });
                }
            }
        }
    }
    if !cursor_found {
        return Err(invalid("task_diagnostic_cursor_outside_window"));
    }
    let runs = report
        .page
        .events
        .iter()
        .filter_map(|event| event.links().run_id().copied())
        .collect::<std::collections::BTreeSet<_>>();
    report.diagnostic_gaps = runs
        .difference(&covered)
        .map(|run| TaskDiagnosticGap {
            run_id: *run,
            state: "not_recorded_or_unpublished_or_withheld_or_outside_window",
            record_count: None,
        })
        .collect();
    report.gaps.sort_unstable();
    report.gaps.dedup();
    report.window_complete &= report.gaps.is_empty();
    Ok(())
}
