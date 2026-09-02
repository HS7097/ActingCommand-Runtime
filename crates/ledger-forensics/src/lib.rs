// SPDX-License-Identifier: AGPL-3.0-only

#![forbid(unsafe_code)]

use actingcommand_artifact_store::{
    ArtifactStoreError, EvidenceManifest, verify_evidence_archive, verify_projected_read_only,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerCorruptTail, GlobalLedgerError, GlobalLedgerReadOnly,
    GlobalLedgerReadOnlyConfig, GlobalLedgerRepairRecord, GlobalLedgerWriterMetadataObservation,
    PersistedEvent,
};
use serde::Serialize;
use serde_json::json;
use std::error::Error;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

pub const MAX_FORENSIC_EVENTS: usize = 1_024;
pub const MAX_FORENSIC_REPAIRS: usize = 1_024;
pub const EVIDENCE_ARCHIVE_VERIFIER: &str = "actingcommand_artifact_store::verify_evidence_archive";
const MAX_REQUEST_ID_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForensicCommand {
    Open,
    Events,
    Chain { request_id: String },
    Tail,
    Repairs,
    Export,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicReplayRequest {
    zip_path: PathBuf,
    expected_sha256: String,
}

impl ForensicReplayRequest {
    pub fn new(zip_path: impl AsRef<Path>, expected_sha256: impl Into<String>) -> Self {
        Self {
            zip_path: zip_path.as_ref().to_path_buf(),
            expected_sha256: expected_sha256.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ForensicEventFilter {
    pub origin_module: Option<String>,
    pub diagnostic_code: Option<String>,
    pub severity: Option<String>,
    pub correlation_id: Option<String>,
}

impl ForensicEventFilter {
    pub fn new(
        origin_module: Option<String>,
        diagnostic_code: Option<String>,
        severity: Option<String>,
        correlation_id: Option<String>,
    ) -> ForensicResult<Self> {
        if origin_module
            .as_deref()
            .is_some_and(|value| !valid_origin_module(value))
            || diagnostic_code
                .as_deref()
                .is_some_and(|value| !valid_diagnostic_code(value))
            || severity
                .as_deref()
                .is_some_and(|value| !valid_severity(value))
            || correlation_id
                .as_deref()
                .is_some_and(|value| !valid_correlation_id(value))
        {
            return Err(ForensicError::new(
                "invalid_event_filter",
                "validate_event_filter",
                "event filter contains an unknown enum or invalid token",
            ));
        }
        Ok(Self {
            origin_module,
            diagnostic_code,
            severity,
            correlation_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicEventsRequest {
    filter: ForensicEventFilter,
    after_sequence: u64,
    through_sequence: Option<u64>,
    limit: usize,
}

impl ForensicEventsRequest {
    pub fn new(
        filter: ForensicEventFilter,
        after_sequence: u64,
        through_sequence: Option<u64>,
        limit: usize,
    ) -> ForensicResult<Self> {
        if limit == 0
            || limit > MAX_FORENSIC_EVENTS
            || through_sequence.is_some_and(|through| after_sequence > through)
        {
            return Err(ForensicError::new(
                "invalid_event_page",
                "validate_event_page",
                "event page limit or sequence boundary is invalid",
            ));
        }
        Ok(Self {
            filter,
            after_sequence,
            through_sequence,
            limit,
        })
    }
}

impl Default for ForensicEventsRequest {
    fn default() -> Self {
        Self {
            filter: ForensicEventFilter::default(),
            after_sequence: 0,
            through_sequence: None,
            limit: MAX_FORENSIC_EVENTS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicRequest {
    pub state_root: PathBuf,
    pub command: ForensicCommand,
    events: ForensicEventsRequest,
}

impl ForensicRequest {
    pub fn new(state_root: impl AsRef<Path>, command: ForensicCommand) -> Self {
        Self {
            state_root: state_root.as_ref().to_path_buf(),
            command,
            events: ForensicEventsRequest::default(),
        }
    }

    pub fn events(state_root: impl AsRef<Path>, events: ForensicEventsRequest) -> Self {
        Self {
            state_root: state_root.as_ref().to_path_buf(),
            command: ForensicCommand::Events,
            events,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "command", content = "data", rename_all = "snake_case")]
pub enum ForensicReport {
    Open(OpenReport),
    Events(EventsReport),
    Chain(ChainReport),
    Tail(TailReport),
    Repairs(RepairsReport),
    Replay(ReplayReport),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WriterObservationReport {
    Absent,
    Readable {
        schema_version: String,
        owner_id: String,
        pid: u32,
        active: bool,
        started_at_unix_ms: u64,
        closed_at_unix_ms: Option<u64>,
    },
    Locked {
        byte_count: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorruptTailReport {
    pub code: String,
    pub segment_index: u64,
    pub byte_offset: u64,
    pub dangling_byte_count: u64,
    pub tail_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairReport {
    pub schema_version: String,
    pub repair_id: String,
    pub completed: bool,
    pub segment_index: u64,
    pub original_len: u64,
    pub repaired_len: u64,
    pub tail_sha256: String,
    pub quarantine_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenReport {
    pub latest_sequence: u64,
    pub event_count: usize,
    pub listed_through_segment: Option<u64>,
    pub writer: WriterObservationReport,
    pub repair_count: usize,
    pub corrupt_tail: Option<CorruptTailReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventsReport {
    pub filter: ForensicEventFilter,
    pub after_sequence: u64,
    pub through_sequence: u64,
    pub limit: usize,
    pub events: Vec<PersistedEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_after_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainReport {
    pub request_id: String,
    pub through_sequence: u64,
    pub limit: usize,
    pub events: Vec<PersistedEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TailReport {
    pub corrupt_tail: Option<CorruptTailReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairsReport {
    pub limit: usize,
    pub repairs: Vec<RepairReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplayReport {
    pub verifier: &'static str,
    pub zip_byte_count: u64,
    pub zip_sha256: String,
    pub manifest_sha256: String,
    pub manifest: EvidenceManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForensicOutput {
    Machine(ForensicReport),
    Human(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicError {
    code: &'static str,
    operation: &'static str,
    detail: String,
}

impl ForensicError {
    fn new(code: &'static str, operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            operation,
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn operation(&self) -> &'static str {
        self.operation
    }
}

impl fmt::Display for ForensicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} during {}: {}",
            self.code, self.operation, self.detail
        )
    }
}

impl Error for ForensicError {}

pub type ForensicResult<T> = Result<T, ForensicError>;

pub fn run(request: ForensicRequest) -> ForensicResult<ForensicOutput> {
    if request.state_root.as_os_str().is_empty() {
        return Err(ForensicError::new(
            "invalid_state_root",
            "validate_forensic_request",
            "state root is empty",
        ));
    }
    if let ForensicCommand::Chain { request_id } = &request.command
        && (request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_BYTES)
    {
        return Err(ForensicError::new(
            "invalid_request_id",
            "validate_forensic_request",
            "request id is empty or exceeds the bounded length",
        ));
    }

    let artifact_root = request.state_root.clone();
    let snapshot = GlobalLedger::open_read_only(
        GlobalLedgerReadOnlyConfig::new(request.state_root.join("ledger")),
        |reference| verify_projected_read_only(&artifact_root, reference).ok(),
    )
    .map_err(map_ledger_error)?;

    match request.command {
        ForensicCommand::Open => Ok(ForensicOutput::Machine(ForensicReport::Open(open_report(
            &snapshot,
        )))),
        ForensicCommand::Events => Ok(ForensicOutput::Machine(ForensicReport::Events(
            events_report(&snapshot, request.events)?,
        ))),
        ForensicCommand::Chain { request_id } => {
            let query =
                serde_json::from_value(json!({ "request_id": request_id })).map_err(query_error)?;
            let through_sequence = snapshot.latest_sequence();
            let events = snapshot
                .query_page(&query, 0, through_sequence, MAX_FORENSIC_EVENTS)
                .map_err(map_ledger_error)?;
            Ok(ForensicOutput::Machine(ForensicReport::Chain(
                ChainReport {
                    request_id,
                    through_sequence,
                    limit: MAX_FORENSIC_EVENTS,
                    events,
                },
            )))
        }
        ForensicCommand::Tail => Ok(ForensicOutput::Machine(ForensicReport::Tail(TailReport {
            corrupt_tail: snapshot.corrupt_tail().map(corrupt_tail_report),
        }))),
        ForensicCommand::Repairs => Ok(ForensicOutput::Machine(ForensicReport::Repairs(
            RepairsReport {
                limit: MAX_FORENSIC_REPAIRS,
                repairs: repair_reports(&snapshot),
            },
        ))),
        ForensicCommand::Export => Ok(ForensicOutput::Human(render_export(&snapshot)?)),
    }
}

pub fn replay(request: ForensicReplayRequest) -> ForensicResult<ForensicOutput> {
    if request.zip_path.as_os_str().is_empty() {
        return Err(ForensicError::new(
            "invalid_evidence_archive_path",
            "validate_forensic_replay",
            "evidence archive path is empty",
        ));
    }
    let verification = verify_evidence_archive(&request.zip_path, &request.expected_sha256)
        .map_err(map_artifact_store_error)?;
    Ok(ForensicOutput::Machine(ForensicReport::Replay(
        ReplayReport {
            verifier: EVIDENCE_ARCHIVE_VERIFIER,
            zip_byte_count: verification.zip_byte_count,
            zip_sha256: verification.zip_sha256,
            manifest_sha256: verification.manifest_sha256,
            manifest: verification.manifest,
        },
    )))
}

fn events_report(
    snapshot: &GlobalLedgerReadOnly,
    request: ForensicEventsRequest,
) -> ForensicResult<EventsReport> {
    let through_sequence = request
        .through_sequence
        .unwrap_or_else(|| snapshot.latest_sequence());
    if request.after_sequence > through_sequence {
        return Err(ForensicError::new(
            "invalid_event_page",
            "validate_event_page",
            "after sequence exceeds the frozen through sequence",
        ));
    }
    let mut events = Vec::with_capacity(request.limit);
    let mut has_more = false;
    for event in snapshot.events() {
        if event.sequence() <= request.after_sequence || event.sequence() > through_sequence {
            continue;
        }
        if !event_matches_filter(event, &request.filter)? {
            continue;
        }
        if events.len() == request.limit {
            has_more = true;
            break;
        }
        events.push(event.clone());
    }
    let next_after_sequence = has_more.then(|| {
        events
            .last()
            .expect("a full positive-limit page has a final event")
            .sequence()
    });
    Ok(EventsReport {
        filter: request.filter,
        after_sequence: request.after_sequence,
        through_sequence,
        limit: request.limit,
        events,
        next_after_sequence,
    })
}

fn event_matches_filter(
    event: &PersistedEvent,
    filter: &ForensicEventFilter,
) -> ForensicResult<bool> {
    if filter
        .origin_module
        .as_deref()
        .is_some_and(|value| event.origin().module().as_str() != value)
        || filter.diagnostic_code.as_deref().is_some_and(|value| {
            event.payload().diagnostic_code().map(|code| code.as_str()) != Some(value)
        })
        || filter
            .severity
            .as_deref()
            .is_some_and(|value| event.severity().as_str() != value)
    {
        return Ok(false);
    }
    let Some(expected) = filter.correlation_id.as_deref() else {
        return Ok(true);
    };
    let actual = event
        .links()
        .correlation_id()
        .map(serde_json::to_value)
        .transpose()
        .map_err(serialization_error)?;
    Ok(actual.as_ref().and_then(serde_json::Value::as_str) == Some(expected))
}

fn valid_origin_module(value: &str) -> bool {
    matches!(
        value,
        "actingctl"
            | "actinglab"
            | "runtime"
            | "scheduler"
            | "policy"
            | "device-proxy"
            | "capture"
            | "capture-pipeline"
            | "recognition"
            | "resource-tooling"
            | "artifact-store"
            | "evidence-exporter"
            | "global-ledger"
            | "performance-monitor"
            | "fact-store"
            | "governance"
            | "agent-dispatcher"
            | "process-test"
    )
}

fn valid_diagnostic_code(value: &str) -> bool {
    matches!(
        value,
        "runtime.diagnostic"
            | "runtime.owner_conflict"
            | "runtime.protocol_invalid"
            | "lease.busy"
            | "lease.cooldown"
            | "lease.expired"
            | "lease.fencing_denied"
            | "lease.queue_cancelled"
            | "lease.queue_expired"
            | "lease.queue_disconnected"
            | "backend.open_failed"
            | "backend.operation_failed"
            | "capture.failed"
            | "artifact.write_failed"
            | "artifact.verify_failed"
            | "artifact.export_failed"
            | "artifact.pinned_frame_missing"
            | "recognition.failed"
            | "input.failed"
            | "application.failed"
            | "command.rejected"
            | "policy.rejected"
            | "catalog.transition_failed"
            | "release.transition_failed"
    )
}

fn valid_severity(value: &str) -> bool {
    matches!(value, "debug" | "info" | "warning" | "error" | "fatal")
}

fn valid_correlation_id(value: &str) -> bool {
    value.strip_prefix("correlation_").is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn open_report(snapshot: &GlobalLedgerReadOnly) -> OpenReport {
    OpenReport {
        latest_sequence: snapshot.latest_sequence(),
        event_count: snapshot.events().len(),
        listed_through_segment: snapshot.listed_through_segment(),
        writer: writer_report(snapshot.writer_metadata()),
        repair_count: snapshot.repairs().len(),
        corrupt_tail: snapshot.corrupt_tail().map(corrupt_tail_report),
    }
}

fn writer_report(observation: &GlobalLedgerWriterMetadataObservation) -> WriterObservationReport {
    match observation {
        GlobalLedgerWriterMetadataObservation::Absent => WriterObservationReport::Absent,
        GlobalLedgerWriterMetadataObservation::Readable(metadata) => {
            WriterObservationReport::Readable {
                schema_version: metadata.schema_version().to_owned(),
                owner_id: metadata.owner_id().to_owned(),
                pid: metadata.pid(),
                active: metadata.active(),
                started_at_unix_ms: metadata.started_at_unix_ms(),
                closed_at_unix_ms: metadata.closed_at_unix_ms(),
            }
        }
        GlobalLedgerWriterMetadataObservation::Locked { byte_count } => {
            WriterObservationReport::Locked {
                byte_count: *byte_count,
            }
        }
    }
}

fn corrupt_tail_report(tail: &GlobalLedgerCorruptTail) -> CorruptTailReport {
    CorruptTailReport {
        code: tail.code.to_owned(),
        segment_index: tail.segment_index,
        byte_offset: tail.byte_offset,
        dangling_byte_count: tail.dangling_byte_count,
        tail_sha256: tail.tail_sha256.clone(),
    }
}

fn repair_reports(snapshot: &GlobalLedgerReadOnly) -> Vec<RepairReport> {
    snapshot
        .repairs()
        .iter()
        .take(MAX_FORENSIC_REPAIRS)
        .map(repair_report)
        .collect()
}

fn repair_report(repair: &GlobalLedgerRepairRecord) -> RepairReport {
    RepairReport {
        schema_version: repair.schema_version().to_owned(),
        repair_id: repair.repair_id().to_owned(),
        completed: repair.completed(),
        segment_index: repair.segment_index(),
        original_len: repair.original_len(),
        repaired_len: repair.repaired_len(),
        tail_sha256: repair.tail_sha256().to_owned(),
        quarantine_key: repair.quarantine_key().to_owned(),
    }
}

fn render_export(snapshot: &GlobalLedgerReadOnly) -> ForensicResult<String> {
    let open = open_report(snapshot);
    let repairs = repair_reports(snapshot);
    let query = serde_json::from_value(json!({})).map_err(query_error)?;
    let events = snapshot
        .query_page(&query, 0, snapshot.latest_sequence(), MAX_FORENSIC_EVENTS)
        .map_err(map_ledger_error)?;
    let mut report = String::new();
    writeln!(report, "ActingCommand ledger forensic export").expect("write String");
    writeln!(report, "latest_sequence: {}", open.latest_sequence).expect("write String");
    writeln!(report, "event_count: {}", open.event_count).expect("write String");
    writeln!(
        report,
        "listed_through_segment: {}",
        open.listed_through_segment
            .map_or_else(|| "none".to_owned(), |value| value.to_string())
    )
    .expect("write String");
    match &open.writer {
        WriterObservationReport::Absent => writeln!(report, "writer: absent"),
        WriterObservationReport::Readable {
            schema_version,
            owner_id,
            pid,
            active,
            started_at_unix_ms,
            closed_at_unix_ms,
        } => writeln!(
            report,
            "writer: readable schema={schema_version} owner={owner_id} pid={pid} active={active} started={started_at_unix_ms} closed={}",
            closed_at_unix_ms.map_or_else(|| "none".to_owned(), |value| value.to_string())
        ),
        WriterObservationReport::Locked { byte_count } => {
            writeln!(report, "writer: locked bytes={byte_count}")
        }
    }
    .expect("write String");
    if let Some(tail) = &open.corrupt_tail {
        writeln!(
            report,
            "corrupt_tail: {} segment={} offset={} bytes={} sha256={}",
            tail.code,
            tail.segment_index,
            tail.byte_offset,
            tail.dangling_byte_count,
            tail.tail_sha256
        )
        .expect("write String");
    } else {
        writeln!(report, "corrupt_tail: none").expect("write String");
    }
    writeln!(report, "repairs:").expect("write String");
    for repair in repairs {
        writeln!(
            report,
            "- id={} completed={} segment={} original={} repaired={} tail_sha256={} quarantine={}",
            repair.repair_id,
            repair.completed,
            repair.segment_index,
            repair.original_len,
            repair.repaired_len,
            repair.tail_sha256,
            repair.quarantine_key
        )
        .expect("write String");
    }
    writeln!(report, "events:").expect("write String");
    for event in events {
        let line = serde_json::to_string(&event).map_err(serialization_error)?;
        writeln!(report, "- {line}").expect("write String");
    }
    Ok(report)
}

fn map_ledger_error(error: GlobalLedgerError) -> ForensicError {
    ForensicError::new(error.code(), error.operation(), error.to_string())
}

fn map_artifact_store_error(error: ArtifactStoreError) -> ForensicError {
    ForensicError::new(error.code(), error.operation(), error.to_string())
}

fn query_error(error: serde_json::Error) -> ForensicError {
    ForensicError::new("invalid_request_id", "parse_event_query", error.to_string())
}

fn serialization_error(error: serde_json::Error) -> ForensicError {
    ForensicError::new(
        "serialization_failed",
        "serialize_forensic_export",
        error.to_string(),
    )
}
