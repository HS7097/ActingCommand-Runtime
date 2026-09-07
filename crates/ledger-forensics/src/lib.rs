// SPDX-License-Identifier: AGPL-3.0-only

#![forbid(unsafe_code)]

use actingcommand_artifact_store::{
    ArtifactStoreError, EvidenceManifest, read_projected_verified, verify_evidence_archive,
    verify_projected_read_only,
};
use actingcommand_contract::ArtifactProducer;
use actingcommand_contract::{
    ActionId, ArtifactKind, ArtifactRedactionState, EFFECTIVE_CONFIGURATION_SCHEMA,
    EffectiveConfigurationFacts, EffectiveConfigurationRecord, EventType, FrameId,
    MAX_EFFECTIVE_CONFIGURATION_BYTES, ProjectedArtifactReference, RunId, TaskId,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerCorruptTail, GlobalLedgerError, GlobalLedgerReadOnly,
    GlobalLedgerReadOnlyConfig, GlobalLedgerRepairRecord, GlobalLedgerStorageSnapshot,
    GlobalLedgerWriterMetadataObservation, PerformanceLedgerSample, PerformanceProcessOwnership,
    PerformanceProcessSummary, PersistedEvent,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::error::Error;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

mod task_records;
pub use task_records::{TaskDiagnosticGap, TaskDiagnosticPage, TaskRecordsRequest};

pub const MAX_FORENSIC_EVENTS: usize = 1_024;
pub const MAX_FORENSIC_REPAIRS: usize = 1_024;
pub const MAX_STABILITY_ARTIFACT_BYTES: u64 = 16 * 1_024;
const STABILITY_SCHEMA: &str = "actingcommand.runtime.contained-task-stability-comparison.v1";
const STABILITY_SCHEMA_PREFIX: &str = "actingcommand.runtime.contained-task-stability-comparison.";
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
    Performance,
    Stability,
    TaskEvidence,
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
    task_records: TaskRecordsRequest,
}

impl ForensicRequest {
    pub fn new(state_root: impl AsRef<Path>, command: ForensicCommand) -> Self {
        Self {
            state_root: state_root.as_ref().to_path_buf(),
            command,
            events: ForensicEventsRequest::default(),
            task_records: TaskRecordsRequest::default(),
        }
    }

    pub fn events(state_root: impl AsRef<Path>, events: ForensicEventsRequest) -> Self {
        Self {
            state_root: state_root.as_ref().to_path_buf(),
            command: ForensicCommand::Events,
            events,
            task_records: TaskRecordsRequest::default(),
        }
    }

    pub fn performance(state_root: impl AsRef<Path>, events: ForensicEventsRequest) -> Self {
        Self {
            state_root: state_root.as_ref().to_path_buf(),
            command: ForensicCommand::Performance,
            events,
            task_records: TaskRecordsRequest::default(),
        }
    }

    pub fn stability(state_root: impl AsRef<Path>, events: ForensicEventsRequest) -> Self {
        Self {
            state_root: state_root.as_ref().to_path_buf(),
            command: ForensicCommand::Stability,
            events,
            task_records: TaskRecordsRequest::default(),
        }
    }

    pub fn task_evidence(state_root: impl AsRef<Path>, events: ForensicEventsRequest) -> Self {
        Self {
            state_root: state_root.as_ref().to_path_buf(),
            command: ForensicCommand::TaskEvidence,
            events,
            task_records: TaskRecordsRequest::default(),
        }
    }

    pub fn with_task_records(mut self, records: TaskRecordsRequest) -> ForensicResult<Self> {
        if self.command != ForensicCommand::TaskEvidence
            || records.limit == 0
            || records.limit > actingcommand_contract::MAX_TASK_DIAGNOSTIC_PAGE_RECORDS
        {
            return Err(ForensicError::new(
                "invalid_task_record_page",
                "task_evidence",
                "invalid task record pagination",
            ));
        }
        self.task_records = records;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "command", content = "data", rename_all = "snake_case")]
pub enum ForensicReport {
    Open(OpenReport),
    Events(EventsReport),
    Performance(Box<PerformanceReport>),
    Stability(Box<StabilityReport>),
    TaskEvidence(Box<TaskEvidenceReport>),
    Chain(ChainReport),
    Tail(TailReport),
    Repairs(RepairsReport),
    Replay(Box<ReplayReport>),
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
    pub storage_snapshot: Box<GlobalLedgerStorageSnapshot>,
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
pub struct TaskEvidenceRelation {
    pub state: &'static str,
    pub source_sequences: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskFrameEvidence {
    pub frame_id: Option<FrameId>,
    pub capture: TaskEvidenceRelation,
    pub png: TaskEvidenceRelation,
    pub artifacts: Vec<ProjectedArtifactReference>,
    pub capture_summary: TaskEvidenceRelation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskInputEvidence {
    pub intent_sequence: u64,
    pub physical_action_id: Option<ActionId>,
    pub intent: actingcommand_contract::InputIntentPayload,
    pub source_step: TaskEvidenceRelation,
    pub outcome: TaskEvidenceRelation,
    pub before_frame: TaskFrameEvidence,
    pub after_capture: TaskEvidenceRelation,
    pub after_frame: TaskFrameEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskStepEvidence {
    pub effect_intent_sequence: u64,
    pub physical_inputs: TaskEvidenceRelation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskEvidenceReport {
    pub page: EventsReport,
    pub inputs: Vec<TaskInputEvidence>,
    pub steps: Vec<TaskStepEvidence>,
    pub window_complete: bool,
    pub corrupt_tail: Option<CorruptTailReport>,
    pub failures: Vec<StabilityFailure>,
    pub gaps: Vec<&'static str>,
    pub diagnostics: Vec<TaskDiagnosticPage>,
    pub diagnostic_gaps: Vec<TaskDiagnosticGap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PerformanceReport {
    pub after_sequence: u64,
    pub through_sequence: u64,
    pub limit: usize,
    pub snapshot_latest_sequence: u64,
    pub scanned_event_count: usize,
    pub scanned_through_sequence: u64,
    pub stutter_count: usize,
    pub clock_jump_count: usize,
    pub summary_count: usize,
    pub rows: Vec<PerformanceRow>,
    pub next_after_sequence: Option<u64>,
    pub has_more: bool,
    pub window_complete: bool,
    pub corrupt_tail: Option<CorruptTailReport>,
    pub gaps: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StabilityReport {
    pub after_sequence: u64,
    pub through_sequence: u64,
    pub limit: usize,
    pub artifact_byte_limit: u64,
    pub snapshot_latest_sequence: u64,
    pub scanned_event_count: usize,
    pub scanned_through_sequence: u64,
    pub scanned_diagnostic_count: usize,
    pub matched_count: usize,
    pub rows: Vec<StabilityRow>,
    pub failures: Vec<StabilityFailure>,
    pub next_after_sequence: Option<u64>,
    pub has_more: bool,
    pub window_complete: bool,
    pub corrupt_tail: Option<CorruptTailReport>,
    pub gaps: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StabilityRow {
    pub event: PersistedEvent,
    pub artifact: ProjectedArtifactReference,
    pub comparison: StabilityComparison,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StabilityFailure {
    // Snapshot verification may fail before the source event is admitted.
    pub source_sequence: Option<u64>,
    pub artifact: ProjectedArtifactReference,
    pub code: &'static str,
    pub operation: &'static str,
}

/// Read-side representation of the existing v1 artifact, without derived values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StabilityComparison {
    pub schema_version: String,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub action_id: ActionId,
    pub step_index: u32,
    pub operation_label: String,
    pub previous_frame_id: FrameId,
    pub current_frame_id: FrameId,
    pub region: StabilityRegion,
    pub comparison_mode: String,
    pub comparison_parameters: serde_json::Map<String, serde_json::Value>,
    pub result: String,
    pub prior_consecutive_unchanged: u32,
    pub new_consecutive_unchanged: u32,
    pub consecutive_unchanged_threshold: u32,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StabilityRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PerformanceRow {
    pub event: PersistedEvent,
    pub observation: PerformanceObservation,
    pub thread_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PerformanceObservation {
    ResourceSample {
        sampled_at_unix_ms: u64,
        ledger_commits: Option<PerformanceLedgerSample>,
        foreground: Option<PerformanceProcessMemory>,
        owned_processes: Vec<PerformanceProcessMemory>,
        third_party_high_load: Vec<PerformanceProcessMemory>,
    },
    Stutter {
        frame_gap_ms: u64,
        capture_latency_ms: Option<u64>,
        recognition_latency_ms: Option<u64>,
        action_effect_latency_ms: Option<u64>,
    },
    ClockJump {
        magnitude_ms: Option<i64>,
        instance_id: Option<String>,
        host_responsiveness_basis_points: Option<u16>,
        third_party_pressure_basis_points: Option<u16>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PerformanceProcessMemory {
    pub pid: u32,
    pub process_name: String,
    pub ownership: PerformanceProcessOwnership,
    pub process_created_at_windows_100ns: Option<u64>,
    pub working_set_bytes: u64,
    pub peak_working_set_bytes: Option<u64>,
}

impl From<&PerformanceProcessSummary> for PerformanceProcessMemory {
    fn from(value: &PerformanceProcessSummary) -> Self {
        Self {
            pid: value.pid,
            process_name: value.process_name.clone(),
            ownership: value.ownership,
            process_created_at_windows_100ns: value.process_created_at_windows_100ns,
            working_set_bytes: value.working_set_bytes,
            peak_working_set_bytes: value.peak_working_set_bytes,
        }
    }
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
    let stability = request.command == ForensicCommand::Stability;
    let export = request.command == ForensicCommand::Export;
    let task_evidence = request.command == ForensicCommand::TaskEvidence;
    let mut artifact_failures = Vec::new();
    let snapshot = GlobalLedger::open_read_only(
        GlobalLedgerReadOnlyConfig::new(request.state_root.join("ledger")),
        |reference| {
            let verified = if stability && reference.kind == ArtifactKind::DiagnosticJson {
                let size = match task_records::is_task_stream(&artifact_root, reference) {
                    Ok(true) => Ok(()),
                    Ok(false) => check_diagnostic_artifact_size(
                        &artifact_root,
                        reference,
                        MAX_STABILITY_ARTIFACT_BYTES,
                    ),
                    Err(error) => Err(ArtifactStoreError::fatal(
                        error.code(),
                        error.operation(),
                        error.to_string(),
                    )),
                };
                size.and_then(|()| verify_projected_read_only(&artifact_root, reference))
            } else {
                verify_projected_read_only(&artifact_root, reference)
            };
            match verified {
                Ok(verified) => Some(verified),
                Err(error) => {
                    if stability || export || task_evidence {
                        artifact_failures.push(StabilityFailure {
                            source_sequence: None,
                            artifact: reference.clone(),
                            code: error.code(),
                            operation: error.operation(),
                        });
                    }
                    None
                }
            }
        },
    )
    .map_err(map_ledger_error)?;

    if export && let Some(failure) = artifact_failures.first() {
        return Err(ForensicError::new(
            failure.code,
            failure.operation,
            "export artifact verification failed; raw content withheld",
        ));
    }

    match request.command {
        ForensicCommand::Open => Ok(ForensicOutput::Machine(ForensicReport::Open(open_report(
            &snapshot,
        )))),
        ForensicCommand::Events => Ok(ForensicOutput::Machine(ForensicReport::Events(
            events_report(&snapshot, request.events)?,
        ))),
        ForensicCommand::Performance => Ok(ForensicOutput::Machine(ForensicReport::Performance(
            Box::new(performance_report(&snapshot, request.events)?),
        ))),
        ForensicCommand::Stability => Ok(ForensicOutput::Machine(ForensicReport::Stability(
            Box::new(stability_report(
                &snapshot,
                &artifact_root,
                request.events,
                artifact_failures,
            )?),
        ))),
        ForensicCommand::TaskEvidence => {
            let mut report = task_evidence_report(&snapshot, request.events, artifact_failures)?;
            task_records::expand(&artifact_root, &request.task_records, &mut report)?;
            Ok(ForensicOutput::Machine(ForensicReport::TaskEvidence(
                Box::new(report),
            )))
        }
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
        ForensicCommand::Export => Ok(ForensicOutput::Human(render_export(
            &snapshot,
            &artifact_root,
        )?)),
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
    Ok(ForensicOutput::Machine(ForensicReport::Replay(Box::new(
        ReplayReport {
            verifier: EVIDENCE_ARCHIVE_VERIFIER,
            zip_byte_count: verification.zip_byte_count,
            zip_sha256: verification.zip_sha256,
            manifest_sha256: verification.manifest_sha256,
            manifest: verification.manifest,
        },
    ))))
}

fn task_evidence_report(
    snapshot: &GlobalLedgerReadOnly,
    request: ForensicEventsRequest,
    failures: Vec<StabilityFailure>,
) -> ForensicResult<TaskEvidenceReport> {
    use actingcommand_contract::{EventPayload, InputPayload, TaskPayload, TaskSemanticFact};
    if request.filter != ForensicEventFilter::default() {
        return Err(ForensicError::new(
            "invalid_event_page",
            "task_evidence",
            "task evidence accepts only sequence page bounds",
        ));
    }
    let page = events_report(snapshot, request)?;
    let limited = page.after_sequence != 0
        || page.next_after_sequence.is_some()
        || page.through_sequence != snapshot.latest_sequence()
        || !snapshot.storage_snapshot().read_complete
        || snapshot.corrupt_tail().is_some();
    let events = &page.events;
    let mut inputs = Vec::new();
    let mut steps = Vec::new();
    let mut gaps = std::collections::BTreeSet::new();
    for event in events {
        if let EventPayload::Input(InputPayload::Intent(intent)) = event.payload() {
            let physical = event.links().action_id().copied();
            let provenance = intent.provenance();
            let step_id = provenance.and_then(|value| value.source_step_action_id);
            let before = provenance.and_then(|value| value.before_frame_id);
            let source_step = if let Some(step_id) = step_id {
                task_evidence_relation(
                    event,
                    events
                        .iter()
                        .filter(|candidate| {
                            candidate.event_type() == EventType::TaskEffectIntent
                                && candidate.links().action_id() == Some(&step_id)
                        })
                        .collect(),
                    limited,
                    true,
                    |candidate| {
                        candidate.sequence() < event.sequence()
                            && candidate.links().frame_id().copied() == before
                            && matches!(candidate.payload(), EventPayload::Task(TaskPayload::Semantic(payload))
                                if matches!(payload.fact(), TaskSemanticFact::EffectIntent { action, .. }
                                    if provenance.is_some_and(|value| &value.input_action == action)))
                    },
                )
            } else {
                TaskEvidenceRelation {
                    state: "not_recorded",
                    source_sequences: Vec::new(),
                }
            };
            let outcome = task_evidence_relation(
                event,
                events
                    .iter()
                    .filter(|candidate| {
                        physical.is_some()
                            && candidate.links().action_id().copied() == physical
                            && matches!(
                                candidate.event_type(),
                                EventType::InputCommitted | EventType::InputFailed
                            )
                    })
                    .collect(),
                limited,
                true,
                |candidate| {
                    candidate.sequence() > event.sequence()
                        && candidate.payload().action() == intent.action()
                },
            );
            let after_candidates = events
                .iter()
                .filter(|candidate| {
                    physical.is_some()
                        && candidate.event_type() == EventType::CaptureRequested
                        && candidate.links().action_id().copied() == physical
                })
                .collect::<Vec<_>>();
            let after_capture = task_evidence_relation(
                event,
                after_candidates.clone(),
                limited,
                true,
                |candidate| {
                    candidate.sequence() > event.sequence()
                        && outcome.state == "linked"
                        && outcome.source_sequences[0] < candidate.sequence()
                        && events.iter().any(|outcome_event| {
                            outcome_event.sequence() == outcome.source_sequences[0]
                                && outcome_event.event_type() == EventType::InputCommitted
                        })
                },
            );
            let after = if after_capture.state == "linked" {
                after_candidates[0].links().frame_id().copied()
            } else {
                None
            };
            let before_frame = task_frame_evidence(events, event, before, limited, true);
            let after_frame = task_frame_evidence(events, event, after, limited, false);
            if provenance.is_none() {
                gaps.insert("provenance_not_recorded");
            }
            for relation in [
                &source_step,
                &outcome,
                &before_frame.capture,
                &before_frame.png,
                &before_frame.capture_summary,
                &after_capture,
                &after_frame.capture,
                &after_frame.png,
                &after_frame.capture_summary,
            ] {
                if matches!(
                    relation.state,
                    "missing" | "identity_conflict" | "ambiguous" | "source_mismatch"
                ) {
                    gaps.insert(relation.state);
                }
            }
            inputs.push(TaskInputEvidence {
                intent_sequence: event.sequence(),
                physical_action_id: physical,
                intent: intent.clone(),
                source_step,
                outcome,
                before_frame,
                after_capture,
                after_frame,
            });
        }
        if event.event_type() == EventType::TaskEffectIntent {
            let physical_inputs = task_evidence_relation(
                event,
                events.iter().filter(|candidate| {
                    matches!(candidate.payload(), EventPayload::Input(InputPayload::Intent(intent))
                        if intent.provenance().is_some_and(|value| value.source_step_action_id.is_some()
                            && value.source_step_action_id.as_ref() == event.links().action_id()))
                }).collect(),
                limited, false,
                |candidate| candidate.sequence() > event.sequence(),
            );
            if matches!(
                physical_inputs.state,
                "missing" | "identity_conflict" | "source_mismatch"
            ) {
                gaps.insert(physical_inputs.state);
            }
            steps.push(TaskStepEvidence {
                effect_intent_sequence: event.sequence(),
                physical_inputs,
            });
        }
    }
    let corrupt_tail = snapshot.corrupt_tail().map(corrupt_tail_report);
    if corrupt_tail.is_some() {
        gaps.insert("corrupt_tail");
    }
    if !snapshot.storage_snapshot().read_complete {
        gaps.insert("storage_read_incomplete");
    }
    if page.through_sequence > snapshot.latest_sequence() {
        gaps.insert("through_sequence_unavailable");
    }
    if !failures.is_empty() {
        gaps.insert("artifact_verification_failed");
    }
    Ok(TaskEvidenceReport {
        window_complete: !limited && gaps.is_empty(),
        page,
        inputs,
        steps,
        corrupt_tail,
        failures,
        gaps: gaps.into_iter().collect(),
        diagnostics: Vec::new(),
        diagnostic_gaps: Vec::new(),
    })
}

fn task_evidence_relation(
    source: &PersistedEvent,
    candidates: Vec<&PersistedEvent>,
    limited: bool,
    unique: bool,
    consistent: impl Fn(&PersistedEvent) -> bool,
) -> TaskEvidenceRelation {
    let state = if candidates.is_empty() {
        if limited {
            "outside_window_or_missing"
        } else {
            "missing"
        }
    } else if candidates.iter().any(|candidate| {
        let left = source.links();
        let right = candidate.links();
        left.request_id().is_none()
            || left.correlation_id().is_none()
            || left.instance_id().is_none()
            || left.lease_id().is_none()
            || left.request_id() != right.request_id()
            || left.correlation_id() != right.correlation_id()
            || left.instance_id() != right.instance_id()
            || left.lease_id() != right.lease_id()
            || left.task_id() != right.task_id()
            || left.run_id() != right.run_id()
    }) {
        "identity_conflict"
    } else if unique && candidates.len() != 1 {
        "ambiguous"
    } else if candidates.iter().any(|candidate| !consistent(candidate)) {
        "source_mismatch"
    } else {
        "linked"
    };
    TaskEvidenceRelation {
        state,
        source_sequences: candidates.iter().map(|event| event.sequence()).collect(),
    }
}

fn task_frame_evidence(
    events: &[PersistedEvent],
    input: &PersistedEvent,
    frame_id: Option<FrameId>,
    limited: bool,
    before: bool,
) -> TaskFrameEvidence {
    use actingcommand_contract::{CapturePayload, EventPayload};
    let Some(frame) = frame_id else {
        let absent = TaskEvidenceRelation {
            state: "not_recorded",
            source_sequences: Vec::new(),
        };
        return TaskFrameEvidence {
            frame_id,
            capture: absent.clone(),
            png: absent.clone(),
            artifacts: Vec::new(),
            capture_summary: absent,
        };
    };
    let capture_candidates = events
        .iter()
        .filter(|event| {
            event.links().frame_id() == Some(&frame)
                && matches!(
                    event.event_type(),
                    EventType::CaptureCompleted | EventType::CaptureFailed
                )
        })
        .collect::<Vec<_>>();
    let mut capture =
        task_evidence_relation(input, capture_candidates.clone(), limited, true, |event| {
            if before {
                event.sequence() < input.sequence()
            } else {
                event.sequence() > input.sequence()
            }
        });
    if capture.state == "linked" && capture_candidates[0].event_type() == EventType::CaptureFailed {
        capture.state = "failed";
    }
    let png_candidates = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::ArtifactVerified
                && event.links().frame_id() == Some(&frame)
                && event.artifacts().iter().any(|artifact| {
                    artifact.kind() == ArtifactKind::CaptureFrame
                        && artifact.producer() == ArtifactProducer::CaptureStore
                })
        })
        .collect::<Vec<_>>();
    let mut png = task_evidence_relation(input, png_candidates.clone(), limited, true, |event| {
        (if before {
            event.sequence() < input.sequence()
        } else {
            event.sequence() > input.sequence()
        }) && event
            .artifacts()
            .iter()
            .filter(|artifact| {
                artifact.kind() == ArtifactKind::CaptureFrame
                    && artifact.producer() == ArtifactProducer::CaptureStore
            })
            .count()
            == 1
            && event.artifacts().iter().all(|artifact| {
                artifact.frame_id() == Some(&frame)
                    && artifact.run_id() == input.links().run_id()
                    && artifact.correlation_id() == input.links().correlation_id()
            })
    });
    if capture.state == "failed" && png_candidates.is_empty() {
        png.state = "not_produced";
    }
    let artifacts = if png.state == "linked" {
        png_candidates[0]
            .artifacts()
            .iter()
            .filter(|artifact| {
                artifact.kind() == ArtifactKind::CaptureFrame
                    && artifact.producer() == ArtifactProducer::CaptureStore
            })
            .map(|artifact| artifact.project(true))
            .collect()
    } else {
        Vec::new()
    };
    let mut capture_summary = task_evidence_relation(
        input,
        events.iter().filter(|event| {
            matches!(event.payload(), EventPayload::Capture(CapturePayload::SummaryCommitted(payload))
                if payload.summary().frames().iter().any(|value| value.artifact().frame_id == Some(frame))
                    || payload.summary().pinned().iter().any(|value| value.artifact().is_some_and(|artifact| artifact.frame_id == Some(frame))))
        }).collect(),
        limited, true, |event| event.sequence() > input.sequence(),
    );
    if input.links().run_id().is_none() && capture_summary.source_sequences.is_empty() {
        capture_summary.state = "not_applicable";
    }
    TaskFrameEvidence {
        frame_id,
        capture,
        png,
        artifacts,
        capture_summary,
    }
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

fn performance_report(
    snapshot: &GlobalLedgerReadOnly,
    request: ForensicEventsRequest,
) -> ForensicResult<PerformanceReport> {
    let through_sequence = request
        .through_sequence
        .unwrap_or_else(|| snapshot.latest_sequence());
    if request.after_sequence > through_sequence || request.filter != ForensicEventFilter::default()
    {
        return Err(ForensicError::new(
            "invalid_event_page",
            "validate_performance_page",
            "performance pages require an ordered sequence range and no event filters",
        ));
    }
    // Snapshot validation remains owned by GlobalLedger. The report's page budget
    // counts raw facts, including unrelated families, before projecting payloads.
    let events = snapshot.events();
    let start = events.partition_point(|event| event.sequence() <= request.after_sequence);
    let end = events.partition_point(|event| event.sequence() <= through_sequence);
    let page_end = start + (end - start).min(request.limit);
    let page = &events[start..page_end];
    let scanned_through_sequence = page
        .last()
        .map_or(request.after_sequence, PersistedEvent::sequence);
    let mut rows = Vec::new();
    let mut stutter_count = 0;
    let mut clock_jump_count = 0;
    let mut summary_count = 0;
    for event in page {
        let observation = if let Some(stutter) = event.payload().performance_stutter() {
            stutter_count += 1;
            PerformanceObservation::Stutter {
                frame_gap_ms: stutter.frame_gap_ms(),
                capture_latency_ms: stutter.capture_latency_ms(),
                recognition_latency_ms: stutter.recognition_latency_ms(),
                action_effect_latency_ms: stutter.action_effect_latency_ms(),
            }
        } else if let Some(control) = event.payload().performance_clock_jump() {
            clock_jump_count += 1;
            PerformanceObservation::ClockJump {
                magnitude_ms: None,
                instance_id: control.instance_id().map(str::to_owned),
                host_responsiveness_basis_points: control.host_responsiveness_basis_points(),
                third_party_pressure_basis_points: control.third_party_pressure_basis_points(),
            }
        } else if let Some(summary) = event.payload().performance_summary() {
            summary_count += 1;
            PerformanceObservation::ResourceSample {
                sampled_at_unix_ms: summary.context().window_end_unix_ms,
                ledger_commits: summary.ledger_commits().cloned(),
                foreground: summary.foreground().map(|value| (&value.process).into()),
                owned_processes: summary.owned_processes().iter().map(Into::into).collect(),
                third_party_high_load: summary
                    .third_party_high_load()
                    .iter()
                    .map(Into::into)
                    .collect(),
            }
        } else {
            continue;
        };
        rows.push(PerformanceRow {
            event: event.clone(),
            observation,
            thread_identity: None,
        });
    }
    let has_more = page_end < end;
    let corrupt_tail = snapshot.corrupt_tail().map(corrupt_tail_report);
    let mut gaps = Vec::new();
    if !snapshot.storage_snapshot().read_complete {
        gaps.push("storage_read_incomplete");
    }
    if corrupt_tail.is_some() {
        gaps.push("corrupt_tail");
    }
    if through_sequence > snapshot.latest_sequence() {
        gaps.push("through_sequence_unavailable");
    }
    Ok(PerformanceReport {
        after_sequence: request.after_sequence,
        through_sequence,
        limit: request.limit,
        snapshot_latest_sequence: snapshot.latest_sequence(),
        scanned_event_count: page.len(),
        scanned_through_sequence,
        stutter_count,
        clock_jump_count,
        summary_count,
        rows,
        next_after_sequence: has_more.then_some(scanned_through_sequence),
        has_more,
        window_complete: !has_more && gaps.is_empty(),
        corrupt_tail,
        gaps,
    })
}

fn check_diagnostic_artifact_size(
    root: &Path,
    reference: &ProjectedArtifactReference,
    byte_limit: u64,
) -> Result<(), ArtifactStoreError> {
    reference.validate().map_err(|_| {
        ArtifactStoreError::fatal(
            "artifact_reference_invalid",
            "bound_stability_artifact",
            "invalid reference",
        )
    })?;
    if reference.byte_count > byte_limit {
        return Err(ArtifactStoreError::fatal(
            if byte_limit == MAX_STABILITY_ARTIFACT_BYTES {
                "stability_artifact_too_large"
            } else {
                "effective_configuration_too_large"
            },
            "bound_stability_artifact",
            "declared bytes exceed limit",
        ));
    }
    let key = reference.object_key().ok_or_else(|| {
        ArtifactStoreError::fatal(
            "artifact_object_key_missing",
            "bound_stability_artifact",
            "missing object key",
        )
    })?;
    // ArtifactStore publishes immutable objects. Check actual size as well as the
    // ledger declaration before its canonical reader allocates the object bytes.
    let metadata = std::fs::metadata(root.join(key)).map_err(|_| {
        ArtifactStoreError::fatal(
            "artifact_read_failed",
            "bound_stability_artifact",
            "cannot inspect referenced object",
        )
    })?;
    if !metadata.is_file() {
        return Err(ArtifactStoreError::fatal(
            "artifact_read_failed",
            "bound_stability_artifact",
            "referenced object is not a file",
        ));
    }
    if metadata.len() > byte_limit {
        return Err(ArtifactStoreError::fatal(
            if byte_limit == MAX_STABILITY_ARTIFACT_BYTES {
                "stability_artifact_too_large"
            } else {
                "effective_configuration_too_large"
            },
            "bound_stability_artifact",
            "stored bytes exceed limit",
        ));
    }
    Ok(())
}

fn stability_report(
    snapshot: &GlobalLedgerReadOnly,
    root: &Path,
    request: ForensicEventsRequest,
    mut failures: Vec<StabilityFailure>,
) -> ForensicResult<StabilityReport> {
    let through_sequence = request
        .through_sequence
        .unwrap_or_else(|| snapshot.latest_sequence());
    if request.after_sequence > through_sequence || request.filter != ForensicEventFilter::default()
    {
        return Err(ForensicError::new(
            "invalid_event_page",
            "validate_stability_page",
            "stability pages require an ordered sequence range and no event filters",
        ));
    }
    let events = snapshot.events();
    let start = events.partition_point(|event| event.sequence() <= request.after_sequence);
    let end = events.partition_point(|event| event.sequence() <= through_sequence);
    let page_end = start + (end - start).min(request.limit);
    let page = &events[start..page_end];
    let scanned_through_sequence = page
        .last()
        .map_or(request.after_sequence, PersistedEvent::sequence);
    let mut rows = Vec::new();
    let mut scanned_diagnostic_count = 0;
    let mut matched_count = 0;
    for event in page {
        for artifact in event
            .artifacts()
            .iter()
            .filter(|artifact| artifact.kind() == ArtifactKind::DiagnosticJson)
        {
            scanned_diagnostic_count += 1;
            let reference = artifact.project(true);
            let result = project_stability(root, event, &reference, &mut matched_count);
            match result {
                Ok(Some(comparison)) => rows.push(StabilityRow {
                    event: event.clone(),
                    artifact: reference,
                    comparison,
                }),
                Ok(None) => {}
                Err(error) => failures.push(StabilityFailure {
                    source_sequence: Some(event.sequence()),
                    artifact: reference,
                    code: error.code(),
                    operation: error.operation(),
                }),
            }
        }
    }
    let has_more = page_end < end;
    let corrupt_tail = snapshot.corrupt_tail().map(corrupt_tail_report);
    let mut gaps = Vec::new();
    if !snapshot.storage_snapshot().read_complete {
        gaps.push("storage_read_incomplete");
    }
    if corrupt_tail.is_some() {
        gaps.push("corrupt_tail");
    }
    if through_sequence > snapshot.latest_sequence() {
        gaps.push("through_sequence_unavailable");
    }
    if !failures.is_empty() {
        gaps.push("artifact_projection_failed");
    }
    Ok(StabilityReport {
        after_sequence: request.after_sequence,
        through_sequence,
        limit: request.limit,
        artifact_byte_limit: MAX_STABILITY_ARTIFACT_BYTES,
        snapshot_latest_sequence: snapshot.latest_sequence(),
        scanned_event_count: page.len(),
        scanned_through_sequence,
        scanned_diagnostic_count,
        matched_count,
        rows,
        failures,
        next_after_sequence: has_more.then_some(scanned_through_sequence),
        has_more,
        window_complete: !has_more && gaps.is_empty(),
        corrupt_tail,
        gaps,
    })
}

fn project_stability(
    root: &Path,
    event: &PersistedEvent,
    reference: &ProjectedArtifactReference,
    matched_count: &mut usize,
) -> ForensicResult<Option<StabilityComparison>> {
    if task_records::is_task_stream(root, reference)? {
        return Ok(None);
    }
    let invalid = |code| {
        ForensicError::new(
            code,
            "project_stability_artifact",
            "diagnostic could not be projected; raw content withheld",
        )
    };
    if reference.redaction_state == ArtifactRedactionState::Pending {
        return Err(invalid("artifact_redaction_pending"));
    }
    check_diagnostic_artifact_size(root, reference, MAX_STABILITY_ARTIFACT_BYTES)
        .map_err(map_artifact_store_error)?;
    let bytes = read_projected_verified(root, reference).map_err(map_artifact_store_error)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| invalid("diagnostic_json_invalid"))?;
    let schema = value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("diagnostic_schema_unavailable"))?;
    if !schema.starts_with(STABILITY_SCHEMA_PREFIX) {
        return Ok(None);
    }
    *matched_count += 1;
    if schema != STABILITY_SCHEMA {
        return Err(invalid("stability_schema_unsupported"));
    }
    let comparison: StabilityComparison =
        serde_json::from_value(value).map_err(|_| invalid("stability_fields_invalid"))?;
    if comparison.comparison_mode != "exact_pixels_v1"
        || !comparison.comparison_parameters.is_empty()
        || !matches!(comparison.result.as_str(), "changed" | "unchanged")
        || comparison.terminal_reason.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "consecutive_unchanged_threshold_reached" | "max_steps_reached"
            )
        })
    {
        return Err(invalid("stability_fields_invalid"));
    }
    let links = event.links();
    if links.task_id() != Some(&comparison.task_id)
        || links.run_id() != Some(&comparison.run_id)
        || links.action_id() != Some(&comparison.action_id)
        || links.frame_id() != Some(&comparison.current_frame_id)
        || reference.run_id.as_ref() != Some(&comparison.run_id)
        || reference.frame_id.as_ref() != Some(&comparison.current_frame_id)
    {
        return Err(invalid("stability_source_links_mismatch"));
    }
    Ok(Some(comparison))
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
        storage_snapshot: Box::new(snapshot.storage_snapshot().clone()),
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

fn render_export(snapshot: &GlobalLedgerReadOnly, root: &Path) -> ForensicResult<String> {
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
        "storage_snapshot: {}",
        serde_json::to_string(&open.storage_snapshot).map_err(|error| {
            ForensicError::new(
                "report_serialization_failed",
                "render_storage_snapshot",
                error.to_string(),
            )
        })?
    )
    .expect("write String");
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
    for event in &events {
        let line = serde_json::to_string(&event).map_err(serialization_error)?;
        writeln!(report, "- {line}").expect("write String");
    }
    writeln!(report, "effective_configuration:").expect("write String");
    for event in &events {
        if event.event_type() != EventType::ArtifactVerified {
            continue;
        }
        for artifact in event.artifacts().iter().filter(|artifact| {
            artifact.kind() == ArtifactKind::DiagnosticJson
                && artifact.producer() == ArtifactProducer::ArtifactStore
        }) {
            let reference = artifact.project(true);
            if task_records::is_task_stream(root, &reference)? {
                continue;
            }
            let invalid = |code| {
                ForensicError::new(
                    code,
                    "project_effective_configuration",
                    "configuration could not be projected; raw content withheld",
                )
            };
            if reference.redaction_state == ArtifactRedactionState::Pending {
                return Err(invalid("artifact_redaction_pending"));
            }
            check_diagnostic_artifact_size(root, &reference, MAX_EFFECTIVE_CONFIGURATION_BYTES)
                .map_err(map_artifact_store_error)?;
            let bytes =
                read_projected_verified(root, &reference).map_err(map_artifact_store_error)?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| invalid("diagnostic_json_invalid"))?;
            let Some(schema) = value
                .get("schema_version")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            if !schema.starts_with("actingcommand.runtime.effective-task-configuration.") {
                continue;
            }
            if schema != EFFECTIVE_CONFIGURATION_SCHEMA {
                return Err(invalid("effective_configuration_schema_unsupported"));
            }
            let record: EffectiveConfigurationRecord = serde_json::from_value(value)
                .map_err(|_| invalid("effective_configuration_fields_invalid"))?;
            let links = event.links();
            if links.request_id() != Some(&record.request_id)
                || links.task_id() != Some(&record.task_id)
                || links.run_id() != Some(&record.run_id)
                || links.frame_id() != record.frame_id.as_ref()
                || links.action_id() != record.action_id.as_ref()
                || reference.run_id.as_ref() != Some(&record.run_id)
                || reference.frame_id.as_ref() != record.frame_id.as_ref()
            {
                return Err(invalid("effective_configuration_source_links_mismatch"));
            }
            match &record.facts {
                EffectiveConfigurationFacts::Initial {
                    capture_observed,
                    input_observed,
                    host_remaining_ms,
                    host_deadline_monotonic_ms,
                    observed_at_monotonic_ms,
                    ..
                } => {
                    if *capture_observed
                        || *input_observed
                        || record.source_sequence.is_some()
                        || record.frame_id.is_some()
                        || record.action_id.is_some()
                        || *host_remaining_ms
                            != host_deadline_monotonic_ms.saturating_sub(*observed_at_monotonic_ms)
                    {
                        return Err(invalid("effective_configuration_fields_invalid"));
                    }
                }
                EffectiveConfigurationFacts::EntryRecovery { package_sha256, .. } => {
                    if package_sha256.len() != 64
                        || !package_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                        || record.source_sequence.is_some()
                        || record.frame_id.is_some()
                        || record.action_id.is_some()
                    {
                        return Err(invalid("effective_configuration_fields_invalid"));
                    }
                }
                EffectiveConfigurationFacts::Capture { .. }
                | EffectiveConfigurationFacts::Input { .. } => {
                    let sequence = record
                        .source_sequence
                        .ok_or_else(|| invalid("effective_configuration_source_missing"))?;
                    let source = snapshot
                        .events()
                        .iter()
                        .find(|source| {
                            source.sequence() == sequence && source.sequence() < event.sequence()
                        })
                        .ok_or_else(|| invalid("effective_configuration_source_missing"))?;
                    let capture =
                        matches!(record.facts, EffectiveConfigurationFacts::Capture { .. });
                    if source.links().request_id() != Some(&record.request_id)
                        || source.links().run_id() != Some(&record.run_id)
                        || (capture
                            && (record.frame_id.is_none()
                                || source.event_type() != EventType::CaptureRequested
                                || source.links().frame_id() != record.frame_id.as_ref()))
                        || (!capture
                            && (record.action_id.is_none()
                                || source.event_type() != EventType::InputCommitted
                                || source.links().action_id() != record.action_id.as_ref()))
                    {
                        return Err(invalid("effective_configuration_source_links_mismatch"));
                    }
                    if capture
                        && !snapshot.events().iter().any(|frame| {
                            frame.event_type() == EventType::ArtifactVerified
                                && frame.sequence() > sequence
                                && frame.sequence() < event.sequence()
                                && frame.links().request_id() == Some(&record.request_id)
                                && frame.links().run_id() == Some(&record.run_id)
                                && frame.links().frame_id() == record.frame_id.as_ref()
                                && frame
                                    .artifacts()
                                    .iter()
                                    .any(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
                        })
                    {
                        return Err(invalid("effective_configuration_capture_evidence_missing"));
                    }
                }
            }
            let line = serde_json::to_string(&json!({"source_sequence":event.sequence(),"artifact":reference,"configuration":record})).map_err(serialization_error)?;
            writeln!(report, "- {line}").expect("write String");
        }
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
