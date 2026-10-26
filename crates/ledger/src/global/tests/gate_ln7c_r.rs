// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c slice R (`rd-1026/ln7c/final-query.md` §7 row R, §3; Alice's ruling 70): the frame
//! index `ledger_frame_refs` of the pointers a record carries only in its payload.
//!
//! - Each pointer kind gives its row(s), written in the record's batch transaction; a summary with
//!   N frames gives N rows.
//! - A backfilled synthetic root (LN-1) holds the rows the synthetic appender writes incrementally.
//! - Records appended by a writer that skips the rows (as an older build does) are filled at the
//!   next open, and the extent moves to the head.
//! - A root without the extent is fully backfilled (its untrusted rows replaced).
//! - A doctored row is a non-fatal scrub finding `ledger_index_mismatch`: the Runtime keeps
//!   running, the extent is marked untrusted and the next open rebuilds the index
//!   (`ledger-maintenance verify` still refuses the same damage, and marks the extent too).
//! - The table never fails an open or an append (§11.7, coordinator's fix round): an extent past
//!   the head is rebuilt with one warning; a batch the extent does not end before commits without
//!   its rows, the extent stays and the next open fills the gap with one warning; a fill that meets
//!   a damaged record stops short of it, the open succeeds with one warning and the next open
//!   tries again.
//!
//! Real formal SQLite roots; frames are stored through the root's ArtifactStore where a record
//! needs a real capture frame reference (summaries, an artifact publication).

use super::missing_material::{formal_root, material_store};
use crate::PersistedEvent;
use crate::global::sqlite::frame_refs::LedgerFrameRefsSkipGate;
use crate::global::{
    GlobalLedger, GlobalLedgerError, LedgerFailurePlan, LedgerMaintenance, LedgerOpenTiming,
    LedgerScrubPolicy, LedgerWritePolicy, Sha256SecretFingerprinter, SteppingScrubClock,
};
use crate::{
    LedgerCode, LedgerLocation, SyntheticInstance, SyntheticLedgerSpec, write_synthetic_ledger,
};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactProducer,
    ArtifactRedactionState, AuditInput, CapturePayloadDraft, CapturePersistedEvidence,
    CapturePinnedEvidence, CaptureSummaryRecord, ClientActionKind, ClientActionRecord,
    ClientActionValue, ClientPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, EventSeverity, EventSource, EventType, EvidenceCompleteness,
    FactContent, FactPayloadDraft, FactRecord, FactScope, FactValue, IdentifierIssuer, InputAction,
    InputPayloadDraft, IssuedCorrelationId, IssuedRunId, MonitorPayloadDraft, OriginModule,
    OwnerEpoch, PinnedFrameReason, ProjectedArtifactReference, RESOURCE_ADJUSTMENT_DETECTOR_PREFIX,
    RESOURCE_READING_DETECTOR_PREFIX, RetentionClass, TaskPayloadDraft,
    resource_adjustment_snapshot_id, resource_reading_snapshot_id,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const GATE_WAIT: Duration = Duration::from_secs(30);
// The State key, the writers' owner id and the fixture instance: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-ln7c-r-fixture";
const OWNER: &str = "ledger-ln7c-r-fixture";
const INSTANCE_ID: &str = "\"instance_000000000000000000000000000000a1\"";
const AT_MS: u64 = 1_752_147_200_000;
// The synthetic history of R3, and the fill's pages: several pages of pointer records.
const SYNTHETIC_RECORDS: u64 = 3_000;
const PAGE_ROWS: usize = 4;
// The scrub of R6, as LN-9 paces it: chunks of eight rows, four rows a second.
const CHUNK_ROWS: usize = 8;
const ROWS_PER_SECOND: u64 = 4;
const CHUNK_MS: u64 = 2_000;
const PASS_INTERVAL_MS: u64 = 3_600_000;

/// One stored row: sequence, target kind, target id, pointer, run.
type Row = (u64, String, String, String, Option<String>);

fn fingerprinter() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"ledger-ln7c-r-fixture").expect("fingerprinter")
}

/// An identifier's canonical text.
fn text<T: serde::Serialize>(id: &T) -> String {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => text,
        other => panic!("identifier text: {other:?}"),
    }
}

/// A stored sequence (`ordered-u64-v1`) and back.
fn stored(sequence: u64) -> i64 {
    (sequence ^ (1_u64 << 63)) as i64
}

fn sequence_of(stored: i64) -> u64 {
    (stored as u64) ^ (1_u64 << 63)
}

fn row(sequence: u64, kind: &str, target: &str, pointer: &str, run: Option<&str>) -> Row {
    (
        sequence,
        kind.to_owned(),
        target.to_owned(),
        pointer.to_owned(),
        run.map(str::to_owned),
    )
}

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

/// Every row of the frame index, in key order of sequence.
fn rows(database: &RuntimeDatabase) -> Vec<Row> {
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("the shared connection");
    let mut statement = connection
        .prepare(
            "SELECT sequence,target_kind,target_id,pointer,run_id FROM ledger_frame_refs ORDER BY sequence,target_kind,target_id,pointer",
        )
        .expect("the frame index");
    statement
        .query_map([], |stored| -> rusqlite::Result<Row> {
            Ok((
                sequence_of(stored.get(0)?),
                stored.get(1)?,
                stored.get(2)?,
                stored.get(3)?,
                stored.get(4)?,
            ))
        })
        .expect("the frame index rows")
        .collect::<Result<Vec<Row>, _>>()
        .expect("the frame index rows")
}

/// The extent row (`from_sequence`, `through_sequence`), when there is one.
fn extent(database: &RuntimeDatabase) -> Option<(u64, u64)> {
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("the shared connection");
    let mut statement = connection
        .prepare("SELECT from_sequence,through_sequence FROM ledger_frame_refs_extent")
        .expect("the extent");
    let extents = statement
        .query_map([], |stored| {
            Ok((sequence_of(stored.get(0)?), sequence_of(stored.get(1)?)))
        })
        .expect("the extent row")
        .collect::<Result<Vec<_>, _>>()
        .expect("the extent row");
    assert!(extents.len() <= 1, "one extent row: {extents:?}");
    extents.first().copied()
}

/// Changes the root through the shared connection, between commits.
fn doctor(database: &RuntimeDatabase, sql: &str) {
    database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("the shared connection")
        .execute_batch(sql)
        .expect("doctor the fixture");
}

/// The writer of an existing formal root, opened anew as a start opens it; with `page_rows`, the
/// Runtime's writer whose history pages (and frame index fill) hold that many rows.
fn reopen(database: &Arc<RuntimeDatabase>, page_rows: Option<usize>) -> GlobalLedger {
    let plan = page_rows.map(|rows| {
        LedgerFailurePlan::new(
            LedgerWritePolicy::new(1, vec![0], 120_000, 1_000, 1_073_741_824)
                .expect("write policy"),
            &database.root().canonicalize().expect("state root"),
            None,
            epoch(),
            AT_MS,
        )
        .with_history_page_rows(rows)
    });
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(
        database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer_timed(
        Arc::clone(database),
        OWNER.into(),
        plan,
        &mut LedgerOpenTiming::default(),
    )
    .expect("the writer opens anew")
}

/// `reopen`, then the head that writer reads, and its close.
fn reopen_and_close(database: &Arc<RuntimeDatabase>, page_rows: Option<usize>) -> u64 {
    let ledger = reopen(database, page_rows);
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    head
}

/// The writer reopened as a start opens it, with history pages (and frame index pages) of
/// `page_rows` rows and an open that authenticates its newest `tail` records; the head it reads
/// and the open's timings, after its close.
fn reopen_noted(
    database: &Arc<RuntimeDatabase>,
    page_rows: usize,
    tail: usize,
) -> (u64, LedgerOpenTiming) {
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], 120_000, 1_000, 1_073_741_824).expect("write policy"),
        &database.root().canonicalize().expect("state root"),
        None,
        epoch(),
        AT_MS,
    )
    .with_history_page_rows(page_rows)
    .with_open_verify_tail_records(tail);
    let limits = MaintenanceLimits::default();
    let mut timing = LedgerOpenTiming::default();
    let ledger = LedgerMaintenance::acquire(
        database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer_timed(Arc::clone(database), OWNER.into(), Some(plan), &mut timing)
    .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    (head, timing)
}

/// Changes one digit inside the stored record's event id: the JSON stays valid and decodable, so
/// only the record's hash, tag and index columns tell.
fn flip_event_id_digit(database: &RuntimeDatabase, sequence: u64) {
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("the shared connection");
    let mut record: Vec<u8> = connection
        .query_row(
            "SELECT canonical_record FROM ledger_events WHERE sequence=?1",
            [stored(sequence)],
            |row| row.get(0),
        )
        .expect("stored record");
    let digit = record
        .windows(5)
        .position(|window| window == b"\"evt_")
        .expect("event id string")
        + 5;
    record[digit] = if record[digit] == b'0' { b'1' } else { b'0' };
    connection
        .execute(
            "UPDATE ledger_events SET canonical_record=?1 WHERE sequence=?2",
            rusqlite::params![record, stored(sequence)],
        )
        .expect("damage the record");
}

/// The open's warning about the frame index: what it found, the sequence and the failure's code.
fn warning(timing: &LedgerOpenTiming) -> Option<(&'static str, Option<u64>, Option<&'static str>)> {
    timing.frame_refs.map(|note| {
        assert!(
            note.outcome().is(LedgerCode::LedgerIndexMismatch),
            "the warning's code: {:?}",
            note.outcome()
        );
        (
            note.stage.as_str(),
            note.sequence,
            note.cause.map(|(code, _)| code),
        )
    })
}

/// A run: its id, the links of its records and their correlation.
struct Run {
    id: IssuedRunId,
    links: EventLinksDraft,
    correlation: IssuedCorrelationId,
}

/// A stored capture frame: its reference and the links of its records.
struct Frame {
    reference: ProjectedArtifactReference,
    links: EventLinksDraft,
}

impl Frame {
    fn artifact_text(&self) -> String {
        text(&self.reference.artifact_id)
    }

    fn frame_text(&self) -> String {
        text(
            &self
                .reference
                .frame_id
                .expect("a capture frame has a frame id"),
        )
    }
}

/// Appends the ArtifactStore's events and keeps the frame's `artifact.verified`.
struct Sink<'a> {
    ledger: &'a GlobalLedger,
    verified: Option<PersistedEvent>,
}

impl ArtifactEventSink for Sink<'_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let draft = draft.sanitize(&fingerprinter()).map_err(|error| {
            ArtifactStoreError::fatal("fixture_sanitize_failed", "ln7c_r_sink", error.to_string())
        })?;
        let event = self.ledger.append(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "ln7c_r_sink", error.to_string())
        })?;
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event);
        }
        Ok(())
    }
}

/// The records of the fixtures, appended to `ledger`.
struct Records<'a> {
    ledger: &'a GlobalLedger,
    ids: IdentifierIssuer,
}

impl<'a> Records<'a> {
    fn new(ledger: &'a GlobalLedger) -> Self {
        Self {
            ledger,
            ids: IdentifierIssuer::new().expect("identifier issuer"),
        }
    }

    fn append(
        &self,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        let draft = EventDraft::new(
            self.ids.mint_event_id().expect("event"),
            AT_MS,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&fingerprinter())
        .expect("fixture draft");
        self.ledger.append(draft).expect("fixture append")
    }

    /// Links in their own request and correlation.
    fn scoped(&self) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_request_id(self.ids.mint_request_id().expect("request"))
            .with_correlation_id(self.ids.mint_correlation_id().expect("correlation"))
    }

    /// A run of the fixture instance and its `task.started`.
    fn run(&self) -> Run {
        let id = self.ids.mint_run_id().expect("run");
        let correlation = self.ids.mint_correlation_id().expect("correlation");
        let links = EventLinksDraft::default()
            .with_instance_id(self.ids.mint_instance_id().expect("instance"))
            .with_request_id(self.ids.mint_request_id().expect("request"))
            .with_correlation_id(correlation)
            .with_task_id(self.ids.mint_task_id().expect("task"))
            .with_run_id(id);
        self.append(
            runtime(),
            links.clone(),
            TaskPayloadDraft::started(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
        Run {
            id,
            links,
            correlation,
        }
    }

    /// A capture frame of `run`, stored through `artifacts`.
    fn frame(&self, artifacts: &ArtifactStore, run: &Run) -> Frame {
        let frame = self.ids.mint_frame_id().expect("frame");
        let links = run.links.clone().with_frame_id(frame);
        let artifact_links = ArtifactLinksDraft::default()
            .with_frame_id(frame)
            .with_correlation_id(run.correlation)
            .with_run_id(run.id);
        let mut sink = Sink {
            ledger: self.ledger,
            verified: None,
        };
        let bytes = format!("ln7c r frame {}", text(frame.transport())).into_bytes();
        artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    &bytes,
                    ArtifactWriteContext::new(artifact_links, links.clone(), AT_MS),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CaptureStore,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::Pending,
                    ),
                ),
                &mut sink,
            )
            .expect("frame publication");
        let reference = sink
            .verified
            .take()
            .expect("verified receipt")
            .artifacts()
            .iter()
            .find(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
            .expect("capture frame reference")
            .project(true);
        Frame { reference, links }
    }

    /// The run's capture summary naming `frames` (in order) with `pins` (frame index, reason).
    fn summary(&self, run: &Run, frames: &[&Frame], pins: &[(usize, PinnedFrameReason)]) -> u64 {
        let evidence = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                CapturePersistedEvidence::new(index as u64, frame.reference.clone())
                    .expect("summary frame")
            })
            .collect::<Vec<_>>();
        let pinned = pins
            .iter()
            .map(|(index, reason)| {
                CapturePinnedEvidence::new(
                    Some(*index as u64),
                    *reason,
                    Some(frames[*index].reference.clone()),
                )
                .expect("summary pin")
            })
            .collect::<Vec<_>>();
        let count = frames.len() as u64;
        let summary = CaptureSummaryRecord::new(
            count,
            0,
            0,
            count,
            EvidenceCompleteness::Complete,
            evidence,
            pinned,
        )
        .expect("summary");
        self.append(
            runtime(),
            run.links.clone(),
            CapturePayloadDraft::summary_committed(summary, AuditInput::new()).into(),
        )
        .sequence()
    }

    /// A `fact.published` of one instance fact with `detector`, `snapshot` and `content`, with
    /// system links as the Runtime publishes a reading.
    fn publish(&self, detector: String, snapshot: String, content: FactContent) -> u64 {
        let record = FactRecord {
            scope: FactScope::Instance {
                instance_id: "fixture-instance-a".to_owned(),
            },
            key: "resource.ln7c_fixture_counter".to_owned(),
            content,
            observed_at_unix_ms: AT_MS,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: detector,
            source_snapshot_id: snapshot,
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "5".repeat(64),
            invalidate_on: Vec::new(),
        };
        self.append(
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::FactStore,
                EventActor::Runtime,
            ),
            self.scoped()
                .with_action_id(self.ids.mint_action_id().expect("action")),
            FactPayloadDraft::published(record, AuditInput::new()).into(),
        )
        .sequence()
    }

    /// An `input.intent` whose provenance names `before` as its before-frame.
    fn intent(&self, links: EventLinksDraft, before: actingcommand_contract::FrameId) -> u64 {
        self.append(
            runtime(),
            links.with_action_id(self.ids.mint_action_id().expect("action")),
            InputPayloadDraft::intent_with_provenance(
                InputAction::Tap { x: 10, y: 20 },
                None,
                None,
                Some(before),
                AuditInput::new(),
            )
            .into(),
        )
        .sequence()
    }

    /// A `capture.dedup_window` linked to `representative` that preserves `preserved`.
    fn marker(&self, representative: &EventLinksDraft, preserved: &EventLinksDraft) -> u64 {
        self.append(
            EventOrigin::new(
                EventSource::System,
                OriginModule::CapturePipeline,
                EventActor::System,
            ),
            representative.clone(),
            CapturePayloadDraft::dedup_window_preserving_material(
                preserved,
                1_000,
                AuditInput::new(),
            )
            .into(),
        )
        .sequence()
    }

    /// A `client.action` with `control` and the path-safe `value`, from `origin`.
    fn confirmation(&self, origin: EventOrigin, control: &str, value: String) -> u64 {
        let record = ClientActionRecord::new(
            "mcp",
            control,
            ClientActionKind::Command,
            None,
            Some(ClientActionValue::PathSafeString(value)),
        )
        .expect("client action");
        self.append(
            origin,
            self.scoped(),
            ClientPayloadDraft::action(record, AuditInput::new()).into(),
        )
        .sequence()
    }

    /// `monitor.probe_requested`, a record without pointers.
    fn plain(&self) -> u64 {
        self.append(
            runtime(),
            EventLinksDraft::default(),
            MonitorPayloadDraft::requested(AuditInput::new()).into(),
        )
        .sequence()
    }
}

fn runtime() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

fn agent() -> EventOrigin {
    EventOrigin::new(
        EventSource::Adapter,
        OriginModule::Governance,
        EventActor::Agent,
    )
}

fn cli() -> EventOrigin {
    EventOrigin::new(EventSource::Cli, OriginModule::Governance, EventActor::Cli)
}

/// A frame link of a frame that is not stored (an input's before-frame, a marker's frame).
fn frame_links(records: &Records<'_>) -> (EventLinksDraft, String) {
    let frame = records.ids.mint_frame_id().expect("frame");
    (
        records.scoped().with_frame_id(frame),
        text(frame.transport()),
    )
}

/// The rows of the records an older writer leaves without rows: two inputs, a marker and a run
/// confirmation; returns their expected rows.
fn pointer_records(records: &Records<'_>) -> Vec<Row> {
    let (first, first_frame) = frame_links(records);
    let (second, second_frame) = frame_links(records);
    let run = records.ids.mint_run_id().expect("run");
    let run_text = text(run.transport());
    records.plain();
    let input = records.intent(records.scoped(), *first.frame_id().expect("frame"));
    let marker = records.marker(&first, &second);
    records.plain();
    let confirmed = records.confirmation(cli(), "evidence_read_run", run_text.clone());
    let second_input = records.intent(records.scoped(), *second.frame_id().expect("frame"));
    vec![
        row(input, "frame", &first_frame, "before_frame", None),
        row(marker, "frame", &second_frame, "preserved_frame", None),
        row(confirmed, "run", &run_text, "read_confirmed", None),
        row(second_input, "frame", &second_frame, "before_frame", None),
    ]
}

/// The rows of `sequences` among `all`.
fn rows_of(all: &[Row], sequences: &BTreeSet<u64>) -> Vec<Row> {
    all.iter()
        .filter(|row| sequences.contains(&row.0))
        .cloned()
        .collect()
}

#[test]
fn ln7c_r1_each_pointer_kind_gives_its_rows_in_the_records_transaction() {
    let dir = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(dir.path());
    let artifacts = material_store(dir.path());
    let records = Records::new(&ledger);
    let run = records.run();
    let first = records.frame(&artifacts, &run);
    let second = records.frame(&artifacts, &run);
    let run_text = text(run.id.transport());
    let (a1, a2) = (first.artifact_text(), second.artifact_text());
    let (f1, f2) = (first.frame_text(), second.frame_text());

    // A summary of both frames, the first pinned before an input and the second as recognition
    // evidence (which exempts nothing, so it gives no row).
    let summary = records.summary(
        &run,
        &[&first, &second],
        &[
            (0, PinnedFrameReason::PreInput),
            (1, PinnedFrameReason::RecognitionEvidence),
        ],
    );
    let reading = records.publish(
        format!("{RESOURCE_READING_DETECTOR_PREFIX}ln7c_task/ln7c_reading"),
        resource_reading_snapshot_id(&run_text, &f1, "ln7c_reading"),
        FactContent::Inline {
            value: FactValue::Integer(1),
        },
    );
    let base = text(records.ids.mint_event_id().expect("event").transport());
    let adjustment = records.publish(
        format!("{RESOURCE_ADJUSTMENT_DETECTOR_PREFIX}ln7c_task/ln7c_adjustment"),
        resource_adjustment_snapshot_id(&run_text, &f2, "ln7c_adjustment", &base),
        FactContent::Inline {
            value: FactValue::Integer(2),
        },
    );
    let published = records.publish(
        "ln7c_detector".to_owned(),
        "ln7c_snapshot".to_owned(),
        FactContent::Artifact {
            artifact: first.reference.clone(),
        },
    );
    let input = records.intent(
        first.links.clone(),
        first.reference.frame_id.expect("frame id"),
    );
    let marker = records.marker(&first.links, &second.links);
    let frame_confirmed = records.confirmation(agent(), "evidence_read_frame", a1.clone());
    let run_confirmed = records.confirmation(cli(), "evidence_read_run", run_text.clone());
    // From an origin that is not a confirmer's: no row.
    records.confirmation(runtime(), "evidence_read_frame", a2.clone());
    let head = ledger.latest_sequence().expect("head");

    let mut expected = vec![
        row(summary, "artifact", &a1, "summary_frame", None),
        row(summary, "artifact", &a1, "summary_pin", None),
        row(summary, "artifact", &a2, "summary_frame", None),
        row(reading, "frame", &f1, "reading", Some(&run_text)),
        row(adjustment, "frame", &f2, "published_frame", None),
        row(published, "artifact", &a1, "published_artifact", None),
        row(input, "frame", &f1, "before_frame", None),
        row(marker, "frame", &f2, "preserved_frame", None),
        row(frame_confirmed, "artifact", &a1, "read_confirmed", None),
        row(run_confirmed, "run", &run_text, "read_confirmed", None),
    ];
    expected.sort();
    assert_eq!(
        rows(&database),
        expected,
        "the pointers' rows, none for the other records"
    );
    assert_eq!(
        extent(&database),
        Some((1, head)),
        "the extent follows each batch"
    );
    ledger.close().expect("close the writer");
}

#[test]
fn ln7c_r2_a_summary_with_n_frames_gives_n_rows() {
    const FRAMES: usize = 6;
    let dir = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(dir.path());
    let artifacts = material_store(dir.path());
    let records = Records::new(&ledger);
    let run = records.run();
    let frames = (0..FRAMES)
        .map(|_| records.frame(&artifacts, &run))
        .collect::<Vec<_>>();
    let summary = records.summary(&run, &frames.iter().collect::<Vec<_>>(), &[]);
    let mut expected = frames
        .iter()
        .map(|frame| {
            row(
                summary,
                "artifact",
                &frame.artifact_text(),
                "summary_frame",
                None,
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(expected.len(), FRAMES);
    assert_eq!(rows(&database), expected, "one row per summary frame");
    ledger.close().expect("close the writer");
}

/// A closed formal root holding the synthetic ledger (LN-1); with `skip`, written by an appender
/// that writes no frame index rows.
fn synthetic_root(skip: bool) -> (TempDir, Arc<RuntimeDatabase>, u64) {
    let dir = tempfile::tempdir().expect("state root");
    let database =
        Arc::new(RuntimeStateStore::open_database(dir.path(), STATE_KEY).expect("State database"));
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        dir.path(),
        true,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner");
    maintenance
        .initialize_empty(&database)
        .expect("formal ledger");
    maintenance
        .open_writer(Arc::clone(&database), OWNER.into())
        .expect("formal writer")
        .close()
        .expect("close the formal writer");
    let spec = SyntheticLedgerSpec::new(
        SYNTHETIC_RECORDS,
        vec![SyntheticInstance {
            alias: "fixture-instance-a".to_owned(),
            instance_id: serde_json::from_str(INSTANCE_ID).expect("a fixture instance id"),
        }],
    );
    let gate = skip.then(|| LedgerFrameRefsSkipGate::arm(dir.path()).expect("the skip gate"));
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    drop(gate);
    (dir, database, generated.head_sequence)
}

#[test]
fn ln7c_r3_a_backfilled_synthetic_root_holds_the_rows_written_incrementally() {
    let (_incremental_dir, incremental, head) = synthetic_root(false);
    let (_backfilled_dir, backfilled, backfilled_head) = synthetic_root(true);
    assert_eq!(head, backfilled_head, "the same history");
    let written = rows(&incremental);
    assert!(
        written.len() >= 4 * PAGE_ROWS,
        "pointer records over several pages: {} rows",
        written.len()
    );
    assert_eq!(extent(&incremental), Some((1, head)));
    assert!(
        rows(&backfilled).is_empty(),
        "the skipping appender wrote no row"
    );

    // Without its extent the root is backfilled at the next open, a few pointer records a page.
    doctor(&backfilled, "DELETE FROM ledger_frame_refs_extent");
    let reopened = reopen_and_close(&backfilled, Some(PAGE_ROWS));
    assert!(reopened >= head);
    assert_eq!(
        rows(&backfilled),
        written,
        "the backfill writes the rows the appender writes"
    );
    assert_eq!(
        extent(&backfilled),
        Some((1, reopened)),
        "the extent is the whole history"
    );
}

#[test]
fn ln7c_r4_records_appended_without_rows_are_filled_at_reopen_and_the_extent_moves() {
    let dir = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(dir.path());
    let before = pointer_records(&Records::new(&ledger));
    let kept = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    assert_eq!(rows(&database), before);

    // A writer that skips the rows, as an older build's does.
    let gate = LedgerFrameRefsSkipGate::arm(dir.path()).expect("the skip gate");
    let older = reopen(&database, None);
    let after = pointer_records(&Records::new(&older));
    let head = older.latest_sequence().expect("head");
    older.close().expect("close the older writer");
    drop(gate);
    assert!(head > kept);
    assert_eq!(
        rows(&database),
        before,
        "no row for the records it appended"
    );
    assert_eq!(extent(&database), Some((1, kept)), "the extent stays");

    // The next open fills the gap, a few pointer records a page.
    let reopened = reopen_and_close(&database, Some(PAGE_ROWS));
    let filled = rows(&database);
    let new = (kept + 1..=head).collect::<BTreeSet<_>>();
    assert_eq!(rows_of(&filled, &new), after, "the gap's rows");
    assert_eq!(
        rows_of(&filled, &(1..=kept).collect()),
        before,
        "the rows before the gap"
    );
    assert_eq!(
        extent(&database),
        Some((1, reopened)),
        "the extent moved to the head"
    );
}

#[test]
fn ln7c_r5_a_root_without_the_extent_is_fully_backfilled() {
    let dir = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(dir.path());
    let records = Records::new(&ledger);
    let mut written = pointer_records(&records);
    written.extend(pointer_records(&records));
    let plain = records.plain();
    ledger.close().expect("close the writer");
    assert_eq!(rows(&database), written);

    // A copy without the extent row: its rows are untrusted, a stray one among them.
    doctor(
        &database,
        &format!(
            "DELETE FROM ledger_frame_refs_extent; INSERT INTO ledger_frame_refs (target_kind,target_id,sequence,pointer,run_id) VALUES ('frame','frame_000000000000000000000000000000f1',{},'before_frame',NULL)",
            stored(plain)
        ),
    );
    assert_eq!(extent(&database), None);
    let reopened = reopen_and_close(&database, Some(PAGE_ROWS));
    assert_eq!(
        rows(&database),
        written,
        "every record's rows, the stray one gone"
    );
    assert_eq!(
        extent(&database),
        Some((1, reopened)),
        "the extent is the whole history"
    );
}

// A formal root as acsetup leaves it, opened as a start does with a scrub on `clock`.
fn scrubbed_root(clock: &Arc<SteppingScrubClock>) -> (TempDir, Arc<RuntimeDatabase>, GlobalLedger) {
    let dir = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(dir.path());
    ledger.close().expect("close the formal writer");
    let scrub = LedgerScrubPolicy::new(ROWS_PER_SECOND, CHUNK_ROWS, PASS_INTERVAL_MS)
        .expect("scrub policy")
        .with_clock(Arc::clone(clock) as Arc<dyn crate::global::LedgerScrubClock>);
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], 120_000, 1_000, 1_073_741_824).expect("write policy"),
        &dir.path().canonicalize().expect("state root"),
        None,
        epoch(),
        AT_MS,
    )
    .with_scrub(scrub);
    let limits = MaintenanceLimits::default();
    let ledger = LedgerMaintenance::acquire(
        dir.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer_timed(
        Arc::clone(&database),
        OWNER.into(),
        Some(plan),
        &mut LedgerOpenTiming::default(),
    )
    .expect("the Runtime's writer");
    (dir, database, ledger)
}

// The full verification of the closed root (`ledger-maintenance verify`).
fn verify(root: &Path, database: &RuntimeDatabase) -> GlobalLedgerError {
    let limits = MaintenanceLimits::default();
    let maintenance =
        LedgerMaintenance::acquire(root, false, limits, limits.deadline().expect("deadline"))
            .expect("maintenance owner");
    let refused = maintenance
        .verify_formal(database, &mut LedgerOpenTiming::default())
        .expect_err("the full verification refuses the doctored row");
    maintenance.close().expect("close the maintenance owner");
    refused
}

#[test]
fn ln7c_r6_a_doctored_row_is_a_non_fatal_scrub_finding_and_the_next_open_rebuilds_the_index() {
    let clock = SteppingScrubClock::new();
    let (dir, database, ledger) = scrubbed_root(&clock);
    let records = Records::new(&ledger);
    let mut written = Vec::new();
    for _ in 0..4 {
        written.extend(pointer_records(&records));
    }
    let doctored = written[0].0;
    let head = ledger.latest_sequence().expect("head");
    assert!(
        clock.wait_idle(GATE_WAIT),
        "the scrub waits for its first pass"
    );
    assert!(
        ledger.scrub_frame_refs_finding().is_none(),
        "nothing found yet"
    );

    // The input's row names another frame.
    doctor(
        &database,
        &format!(
            "UPDATE ledger_frame_refs SET target_id='frame_000000000000000000000000000000f2' WHERE sequence={} AND pointer='before_frame'",
            stored(doctored)
        ),
    );
    // One pass: every chunk is due by its last chunk's time; the next pass is not.
    clock.run_until(head.div_ceil(CHUNK_ROWS as u64).saturating_sub(1) * CHUNK_MS);
    assert!(clock.wait_idle(GATE_WAIT), "the pass ends");

    // The finding is recorded, non-fatal; the scrub and the Runtime go on.
    let finding = ledger
        .scrub_frame_refs_finding()
        .expect("the finding is recorded");
    assert!(
        !finding.is_fatal(),
        "the derived index's finding is not fatal"
    );
    assert_eq!(
        (finding.code(), finding.operation()),
        (
            LedgerCode::LedgerIndexMismatch.as_str(),
            LedgerLocation::VerifyLedgerFrameRefs.as_str()
        )
    );
    let detail = finding.detail().unwrap_or_default();
    assert!(
        detail.contains("phase=scrub")
            && detail.contains("table=ledger_frame_refs")
            && detail.contains(&format!("sequence={doctored}"))
            && detail.contains("extent=untrusted"),
        "what and where: {detail}"
    );
    assert!(
        ledger.scrub_finding().is_none(),
        "no fatal finding: the host latches nothing"
    );
    assert!(!clock.has_ended(), "the scrub goes on to its next pass");

    // The extent is marked untrusted: it covers no record.
    assert_eq!(
        extent(&database),
        Some((head + 1, head)),
        "the extent is marked untrusted"
    );
    // The writer keeps appending; its batches extend the marked extent.
    let after = pointer_records(&records);
    let appended = ledger.latest_sequence().expect("head");
    assert_eq!(appended, head + 6, "every batch committed");
    assert_eq!(extent(&database), Some((head + 1, appended)));
    ledger.close().expect("close the writer");

    // The next open rebuilds the index by its backfill and ends with matching rows.
    let (reopened, _) = reopen_noted(&database, PAGE_ROWS, 4_096);
    assert_eq!(reopened, appended);
    written.extend(after);
    assert_eq!(
        rows(&database),
        written,
        "the index ends with matching rows"
    );
    assert_eq!(
        extent(&database),
        Some((1, reopened)),
        "over the whole history"
    );

    // `ledger-maintenance verify`, a tool run, still fails on the same damage, and marks the
    // extent untrusted as well.
    doctor(
        &database,
        &format!(
            "UPDATE ledger_frame_refs SET target_id='frame_000000000000000000000000000000f2' WHERE sequence={} AND pointer='before_frame'",
            stored(doctored)
        ),
    );
    let full = verify(dir.path(), &database);
    assert!(full.is_fatal(), "the tool run fails");
    assert_eq!(
        (full.code(), full.operation()),
        (finding.code(), finding.operation()),
        "with the scrub's code and operation"
    );
    assert!(
        full.detail()
            .unwrap_or_default()
            .contains("extent=untrusted"),
        "{:?}",
        full.detail()
    );
    assert_eq!(extent(&database), Some((reopened + 1, reopened)));
}

#[test]
fn ln7c_r7_an_extent_past_the_head_is_rebuilt_with_its_warning() {
    let dir = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(dir.path());
    let records = Records::new(&ledger);
    let mut written = pointer_records(&records);
    written.extend(pointer_records(&records));
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    assert_eq!(rows(&database), written);

    doctor(
        &database,
        &format!(
            "UPDATE ledger_frame_refs_extent SET through_sequence={}",
            stored(head + 5)
        ),
    );
    let (reopened, timing) = reopen_noted(&database, PAGE_ROWS, 4_096);
    assert_eq!(reopened, head, "the open succeeds");
    assert_eq!(
        warning(&timing),
        Some((
            LedgerLocation::VerifyLedgerFrameRefs.as_str(),
            Some(head + 5),
            None
        )),
        "one warning names the untrusted extent"
    );
    assert_eq!(rows(&database), written, "the index is rebuilt");
    assert_eq!(extent(&database), Some((1, head)), "over the whole history");
}

#[test]
fn ln7c_r8_a_batch_the_extent_does_not_end_before_commits_and_is_filled_at_reopen() {
    let dir = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(dir.path());
    let records = Records::new(&ledger);
    let before = pointer_records(&records);
    let kept = ledger.latest_sequence().expect("head");

    // The extent no longer ends at the head: the next batches do not continue it, and commit.
    doctor(
        &database,
        &format!(
            "UPDATE ledger_frame_refs_extent SET through_sequence={}",
            stored(kept - 1)
        ),
    );
    let after = pointer_records(&records);
    let head = ledger.latest_sequence().expect("head");
    assert_eq!(head, kept + 6, "every batch committed");
    assert_eq!(rows(&database), before, "the batches wrote no rows");
    assert_eq!(extent(&database), Some((1, kept - 1)), "the extent stays");
    ledger.close().expect("close the writer");

    let (reopened, timing) = reopen_noted(&database, PAGE_ROWS, 4_096);
    assert_eq!(reopened, head, "the open succeeds");
    assert_eq!(
        warning(&timing),
        Some((
            LedgerLocation::WriteLedgerFrameRefs.as_str(),
            Some(kept - 1),
            None
        )),
        "one warning names the gap"
    );
    let mut filled = before;
    filled.extend(after);
    assert_eq!(rows(&database), filled, "the gap is filled");
    assert_eq!(
        extent(&database),
        Some((1, head)),
        "the extent moved to the head"
    );
}

#[test]
fn ln7c_r9_a_fill_that_meets_a_damaged_record_stops_short_of_it_and_the_open_succeeds() {
    // The open authenticates only its newest records; the damaged one is older.
    const TAIL: usize = 4;
    let dir = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(dir.path());
    let records = Records::new(&ledger);
    let mut written = Vec::new();
    for _ in 0..3 {
        written.extend(pointer_records(&records));
    }
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    let damaged = written[0].0;
    assert!(damaged + (TAIL as u64) < head, "below the open's tail");

    // Without its extent the root is backfilled at the next open, one pointer record a page.
    doctor(&database, "DELETE FROM ledger_frame_refs_extent");
    flip_event_id_digit(&database, damaged);
    let (reopened, timing) = reopen_noted(&database, 1, TAIL);
    assert_eq!(reopened, head, "the open succeeds");
    assert_eq!(
        warning(&timing),
        Some((
            LedgerLocation::FillLedgerFrameRefs.as_str(),
            Some(damaged),
            Some(LedgerCode::LedgerRecordMismatch.as_str())
        )),
        "one warning names the record the fill stopped at"
    );
    let (from, through) = extent(&database).expect("the extent left");
    assert!(
        from > damaged && through == head,
        "the extent stops short of the damaged record and never covers the history: {from}..={through}"
    );
    assert!(
        rows(&database).iter().all(|row| row.0 > damaged),
        "no row at or below the damaged record"
    );

    // The next open tries again, and stops there again.
    let (_, again) = reopen_noted(&database, 1, TAIL);
    assert_eq!(
        warning(&again).map(|(_, sequence, _)| sequence),
        Some(Some(damaged))
    );
}
