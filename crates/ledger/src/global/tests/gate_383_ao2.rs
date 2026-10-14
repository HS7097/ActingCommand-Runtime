// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 AO-2, test rows AO2-a to AO2-n (model-ledger-observer v1.4.1 §14.2;
// `contracts/ledger-observer.md` §2.1, §7, §8, §10, §11.1): every record passes one capture
// queue in capture order; its memory segment is watched by three tiers with hysteresis, the
// spill thread moves the oldest records to the dump file, the writer flushes the dump segment
// first and then memory, in batches that keep the index table `ledger_index` in their own
// transaction; a refused capture is written as its gap, a revision hole is reported, a start
// after an unconfirmed tail writes its gap, the flush barrier waits for one record and the close
// drains the queue.
//
// The store is a formal SQLite root. The flush is held with the writer stall gate and the spill
// thread with the write fault gate; both are released by the test, never by a timer. Every
// budget is a record count, so each tier is crossed by a known capture; the byte budget stays
// far away. `GATE_WAIT` only bounds a broken run.

use crate::global::{
    GlobalLedger, GlobalLedgerError, LEDGER_DUMP_FILE_NAME, LedgerCapturePolicy, LedgerFailurePlan,
    LedgerFailureSink, LedgerMaintenance, LedgerOpenTiming, LedgerWritePolicy,
    Sha256SecretFingerprinter, WatermarkRatios, fallback_dump_path,
};
use crate::{LedgerCode, LedgerLocation, LedgerWriteFaultGate, PersistedEvent, WriterStallGate};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ApplicationPayloadDraft, AuditInput, CaptureItem, CaptureTicket, CaptureTier,
    CommandPayloadDraft, DiagnosticCode, DiagnosticDetailDraft, EffectDisposition, EventAction,
    EventActor, EventDraft, EventId, EventLinksDraft, EventOrigin, EventPayload, EventPayloadDraft,
    EventQuery, EventSeverity, EventSource, EventType, FactValue, IdentifierIssuer,
    LedgerBacklogTier, LedgerCapture, LedgerFlushBarrier, LedgerGapKind, LedgerPayload,
    LedgerSegmentEdge, LedgerSource, MonitorPayloadDraft, OriginModule, OwnerEpoch,
    RuntimeFactRecord, RuntimeFactScope, RuntimeLifecyclePhase, RuntimePayloadDraft,
    SanitizedEventDraft, SecretFingerprinter, SourceRevision, TaggedStateStore,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const GATE_WAIT: Duration = Duration::from_secs(30);
// The State integrity key and the writer's owner id: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-ao2-fixture";
const OWNER: &str = "ledger-ao2-fixture";
// The epoch's start in the dump header.
const AT_MS: u64 = 1_752_147_200_000;
// `flush_attempt_stall_ms`, `health_probe_interval_ms` and `dump_max_bytes` of the plans here.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
// The record budget of most plans here: tier 1 at 60 records (released at 54), tier 2 at 75
// (released at 67), tier 3 at 90 (released at 81), the hard limit at 100.
const RECORDS: u64 = 100;
// The tagged and the untagged workbench keys of the fixture (`tag.workbench_patterns`).
const TAGGED_KEY: &str = "host.scheduling_pause";
const UNTAGGED_KEY: &str = "backend.ready";
// A key outside every runtime fact family: its record fails sanitizing.
const REFUSED_KEY: &str = "unknown.family";

// What the writer reports through its failure plan's sink.
struct Reports {
    stops: Mutex<mpsc::Sender<Outcome>>,
}

impl LedgerFailureSink for Reports {
    fn write_retried(&self, _outcome: &Outcome) {}

    fn stop(&self, outcome: &Outcome) {
        let _ = self
            .stops
            .lock()
            .expect("stop reports")
            .send(outcome.clone());
    }
}

fn failure_sink() -> (Arc<Reports>, mpsc::Receiver<Outcome>) {
    let (stops, stopped) = mpsc::channel();
    (
        Arc::new(Reports {
            stops: Mutex::new(stops),
        }),
        stopped,
    )
}

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

// The capture policy of the plans here: `records` in memory at most, the other values the
// defaults (a byte budget of 64 MiB the fixture's records never come near).
fn policy(records: u64) -> LedgerCapturePolicy {
    LedgerCapturePolicy {
        buffer_records: records,
        ..LedgerCapturePolicy::default()
    }
}

fn fingerprinter() -> Arc<dyn SecretFingerprinter + Send + Sync> {
    Arc::new(Sha256SecretFingerprinter::new(b"ledger-ao2-fixture").expect("salt"))
}

fn origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

fn item(
    ids: &IdentifierIssuer,
    payload: EventPayloadDraft,
    links: EventLinksDraft,
    source: Option<SourceRevision>,
) -> CaptureItem {
    CaptureItem {
        event_id: ids.mint_event_id().expect("event id"),
        severity: EventSeverity::Info,
        payload,
        links,
        origin: origin(),
        source,
    }
}

// `monitor.probe_requested`: an observation record (NORMAL sync, never tagged).
fn observation(ids: &IdentifierIssuer) -> CaptureItem {
    observation_from(ids, None)
}

fn observation_from(ids: &IdentifierIssuer, source: Option<SourceRevision>) -> CaptureItem {
    item(
        ids,
        observation_payload(),
        EventLinksDraft::default(),
        source,
    )
}

// `command.received`: a decision record (FULL sync), not tagged.
fn decision(ids: &IdentifierIssuer) -> CaptureItem {
    item(
        ids,
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        EventLinksDraft::default(),
        None,
    )
}

// `runtime.fact_recorded` of the runtime scope.
fn workbench(
    ids: &IdentifierIssuer,
    key: &str,
    value: i64,
    source: Option<SourceRevision>,
) -> CaptureItem {
    item(
        ids,
        workbench_payload(key, value),
        EventLinksDraft::default(),
        source,
    )
}

// A record whose sanitizing fails (its key belongs to no runtime fact family).
fn refused(ids: &IdentifierIssuer, source: Option<SourceRevision>) -> CaptureItem {
    workbench(ids, REFUSED_KEY, 1, source)
}

// The close record G16 captures (`runtime.lifecycle_observed`, the shutdown request).
fn close_record(ids: &IdentifierIssuer) -> CaptureItem {
    item(
        ids,
        RuntimePayloadDraft::lifecycle_observed(
            epoch(),
            RuntimeLifecyclePhase::ShutdownRequested,
            AuditInput::new(),
        )
        .into(),
        EventLinksDraft::default(),
        None,
    )
}

// A `runtime.failed` for a reason outside the ledger, as G16 captures it.
fn runtime_failed(ids: &IdentifierIssuer) -> CaptureItem {
    item(
        ids,
        RuntimePayloadDraft::failed(
            DiagnosticCode::RuntimeDiagnostic,
            EffectDisposition::Indeterminate,
            Outcome::new(SharedCode::RuntimeFatal),
            DiagnosticDetailDraft::new(
                "runtime_stop",
                LedgerLocation::LedgerFlushCaptured.location(),
                "runtime",
                LedgerLocation::LedgerFlushCaptured.as_str(),
            ),
            AuditInput::new(),
        )
        .into(),
        EventLinksDraft::default(),
        None,
    )
}

// The base record of a new lineage of `source` (§12 step 7).
fn base_record(ids: &IdentifierIssuer, source: LedgerSource, lineage: OwnerEpoch) -> CaptureItem {
    item(
        ids,
        RuntimePayloadDraft::lifecycle_observed(
            epoch(),
            RuntimeLifecyclePhase::TaggedStateLoaded {
                stores: vec![TaggedStateStore {
                    source,
                    previous_lineage: None,
                    previous_revision: None,
                    lineage,
                    loaded_count: 0,
                }],
                outcome: None,
            },
            AuditInput::new(),
        )
        .into(),
        EventLinksDraft::default(),
        None,
    )
}

fn revision(source: LedgerSource, lineage: OwnerEpoch, revision: u64) -> Option<SourceRevision> {
    Some(SourceRevision {
        source,
        lineage,
        revision,
    })
}

// A sanitized draft for a direct append, at the fixture's time.
fn sanitized(ids: &IdentifierIssuer, payload: EventPayloadDraft) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        origin(),
        EventLinksDraft::default(),
        payload,
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"ledger-ao2-fixture").expect("salt"))
    .expect("a sanitized draft")
}

fn observation_payload() -> EventPayloadDraft {
    MonitorPayloadDraft::requested(AuditInput::new()).into()
}

fn workbench_payload(key: &str, value: i64) -> EventPayloadDraft {
    RuntimePayloadDraft::fact_recorded(
        RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: key.to_owned(),
            value: FactValue::Integer(value),
            observed_at_unix_ms: AT_MS,
            source: OriginModule::Runtime,
            ttl_ms: None,
        },
        AuditInput::new(),
    )
    .into()
}

// A formal root as acsetup leaves it, with a fallback folder of its own.
struct Root {
    directory: TempDir,
    fallback: TempDir,
    database: Arc<RuntimeDatabase>,
}

impl Root {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("state root");
        let database = Arc::new(
            RuntimeStateStore::open_database(directory.path(), STATE_KEY).expect("State database"),
        );
        let limits = MaintenanceLimits::default();
        let maintenance = LedgerMaintenance::acquire(
            directory.path(),
            true,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner");
        maintenance
            .initialize_empty(&database)
            .expect("formal ledger");
        // The writer opens once and closes, as on a formal root: its owner metadata is what
        // every later start reads.
        maintenance
            .open_writer(Arc::clone(&database), OWNER.into())
            .expect("formal writer")
            .close()
            .expect("close the formal writer");
        Self {
            directory,
            fallback: tempfile::tempdir().expect("fallback folder"),
            database,
        }
    }

    fn state_root(&self) -> PathBuf {
        self.directory.path().canonicalize().expect("state root")
    }

    fn dump_path(&self) -> PathBuf {
        self.state_root().join(LEDGER_DUMP_FILE_NAME)
    }

    fn fallback_path(&self) -> PathBuf {
        fallback_dump_path(self.fallback.path(), &self.state_root())
    }

    // `attempts` attempts with no wait between them, `capture` for the capture queue.
    fn plan(
        &self,
        attempts: u32,
        capture: LedgerCapturePolicy,
        dump_max_bytes: u64,
        reports: &Arc<Reports>,
    ) -> LedgerFailurePlan {
        LedgerFailurePlan::new(
            LedgerWritePolicy::new(
                attempts,
                vec![0; attempts as usize],
                STALL_MS,
                PROBE_MS,
                dump_max_bytes,
            )
            .expect("write policy"),
            &self.state_root(),
            Some(self.fallback.path()),
            epoch(),
            AT_MS,
        )
        .with_sink(Arc::clone(reports) as Arc<dyn LedgerFailureSink>)
        .with_capture_policy(capture)
    }

    // A start: the writer of the running Runtime with `plan`.
    fn open(&self, plan: LedgerFailurePlan) -> Result<GlobalLedger, GlobalLedgerError> {
        let limits = MaintenanceLimits::default();
        LedgerMaintenance::acquire(
            self.directory.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )?
        .open_writer_timed(
            Arc::clone(&self.database),
            OWNER.into(),
            Some(plan),
            &mut LedgerOpenTiming::default(),
        )
    }

    fn faults(&self) -> LedgerWriteFaultGate {
        LedgerWriteFaultGate::arm(self.directory.path()).expect("write fault gate")
    }

    fn stall(&self) -> WriterStallGate {
        WriterStallGate::arm(self.directory.path()).expect("writer stall gate")
    }

    // The row of `ledger_index` for (`role`, `key`): its ledger sequence and event id.
    fn index_row(&self, role: &str, key: &str) -> Option<(u64, String)> {
        let connection = self
            .database
            .connection(LedgerLocation::LedgerIndexRows.location())
            .expect("database connection");
        let mut statement = connection
            .prepare("SELECT sequence, event_id FROM ledger_index WHERE role=?1 AND key=?2")
            .expect("index read");
        let mut rows = statement
            .query_map(rusqlite::params![role, key], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .expect("index rows");
        rows.next()
            .map(|row| row.expect("an index row"))
            .map(|(sequence, event_id)| ((sequence as u64) ^ (1_u64 << 63), event_id))
    }

    // Whether the ledger's table holds a record with `event_id`.
    fn holds_record(&self, event_id: &EventId) -> bool {
        let connection = self
            .database
            .connection(LedgerLocation::LedgerIndexRows.location())
            .expect("database connection");
        connection
            .query_row(
                "SELECT count(*) FROM ledger_events WHERE event_id=?1",
                rusqlite::params![id_text(event_id)],
                |row| row.get::<_, i64>(0),
            )
            .expect("record count")
            > 0
    }
}

fn id_text(id: &EventId) -> String {
    match serde_json::to_value(id).expect("event id") {
        Value::String(text) => text,
        other => panic!("an event id is a text: {other}"),
    }
}

// Every record after `head`.
fn after(ledger: &GlobalLedger, head: u64) -> Vec<PersistedEvent> {
    ledger
        .query(EventQuery {
            from_sequence: Some(head + 1),
            ..EventQuery::default()
        })
        .expect("ledger records")
}

// Every record after `head` once the queue is settled: each read drains what was captured
// before it, the findings the drain itself captured included, so reads repeat until two agree.
fn settled(ledger: &GlobalLedger, head: u64) -> Vec<PersistedEvent> {
    let mut records = after(ledger, head);
    for _ in 0..8 {
        let again = after(ledger, head);
        if again.len() == records.len() {
            return again;
        }
        records = again;
    }
    records
}

fn capture_seq(record: &PersistedEvent) -> u64 {
    record.capture().expect("a captured record").capture_seq
}

fn outcome_value(outcome: &Outcome, key: &str) -> Option<Value> {
    outcome.value(key).cloned()
}

// The health findings of `code`, with their segment edge, in ledger order.
fn findings(
    records: &[PersistedEvent],
    code: LedgerCode,
) -> Vec<(Option<LedgerSegmentEdge>, Outcome)> {
    records
        .iter()
        .filter_map(|record| match record.payload() {
            EventPayload::Ledger(LedgerPayload::HealthObserved(payload)) => {
                let outcome = record.payload().outcome()?;
                outcome
                    .is(code)
                    .then(|| (payload.segment_edge(), outcome.clone()))
            }
            _ => None,
        })
        .collect()
}

fn gaps(records: &[PersistedEvent]) -> Vec<&PersistedEvent> {
    records
        .iter()
        .filter(|record| record.event_type() == EventType::LedgerGapRecorded)
        .collect()
}

// Ledger order is capture order: the capture numbers rise with the sequences.
fn assert_capture_order(records: &[PersistedEvent]) {
    let numbers = records.iter().map(capture_seq).collect::<Vec<_>>();
    let mut sorted = numbers.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(numbers, sorted, "ledger order is capture order");
}

// Each ticket's record holds its capture number.
fn assert_tickets_written(records: &[PersistedEvent], tickets: &[CaptureTicket]) {
    for ticket in tickets {
        let record = records
            .iter()
            .find(|record| record.event_id() == &ticket.event_id)
            .expect("every captured record is in the ledger");
        assert_eq!(capture_seq(record), ticket.capture_seq);
    }
}

// One record line of a dump: its capture number, event id and event type.
struct DumpLine {
    capture_seq: u64,
    event_id: Value,
    event_type: Value,
}

// The record lines of a dump file and whether it has its trailer.
fn read_dump(path: &Path) -> (Vec<DumpLine>, bool) {
    let text = fs::read_to_string(path).expect("the dump file");
    let mut lines = Vec::new();
    let mut trailer = false;
    for line in text.lines().skip(1) {
        let parsed: Value = serde_json::from_str(line).expect("a JSON line");
        if parsed.get("trailer").is_some() {
            trailer = true;
            continue;
        }
        let record = &parsed["record"];
        lines.push(DumpLine {
            capture_seq: record["capture_seq"].as_u64().expect("a capture number"),
            event_id: record["draft"]["event_id"].clone(),
            event_type: record["draft"]["event_type"].clone(),
        });
    }
    (lines, trailer)
}

fn event_type_value(event_type: EventType) -> Value {
    serde_json::to_value(event_type).expect("event type")
}

// AO2-a: with the flush held and a small budget, captures go on: crossing tier 1 starts the busy
// segment, crossing tier 2 makes the spill thread move the oldest records to the dump file down to
// the tier-2 release line and starts the spill segment; every capture returns at once. Once the
// flush goes on, every record is in the ledger in capture order, there is no gap, both segments
// end and the dump file is deleted.
#[test]
fn ao2_a_a_held_flush_spills_the_oldest_records_and_the_ledger_keeps_capture_order() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    faults.hold_spills();
    let stall = root.stall();

    let mut tickets = vec![capture.capture(observation(&ids))];
    assert!(
        stall.wait_held_commit(GATE_WAIT),
        "the flush holds record 1"
    );
    // Records 2 to 61: the 60th record in memory crosses tier 1; its finding is the 61st.
    for _ in 0..59 {
        tickets.push(capture.capture(observation(&ids)));
    }
    assert_eq!(ledger.capture_status().tier, CaptureTier::BelowTier1);
    tickets.push(capture.capture(observation(&ids)));
    let status = ledger.capture_status();
    assert_eq!(status.tier, CaptureTier::Tier1);
    assert_eq!(status.memory_records, 61);
    // Records 63 to 76: the 75th record in memory crosses tier 2.
    for _ in 0..14 {
        tickets.push(capture.capture(observation(&ids)));
    }
    assert!(
        faults.wait_spill_reached(GATE_WAIT),
        "the spill thread spills"
    );
    let status = ledger.capture_status();
    assert_eq!(
        status.memory_records, 67,
        "memory is down to the tier-2 release line"
    );
    faults.release_spills();
    assert!(faults.wait_spills_written(1, GATE_WAIT));
    let (spilled, sealed) = read_dump(&root.dump_path());
    assert!(!sealed, "a spill writes no trailer");
    assert_eq!(
        spilled
            .iter()
            .map(|line| line.capture_seq)
            .collect::<Vec<_>>(),
        (2..=9).collect::<Vec<_>>(),
        "the dump holds the oldest records after the one in flight"
    );
    assert_eq!(ledger.capture_status().dump_records, 8);

    stall.release();
    let records = settled(&ledger, head);
    assert_tickets_written(&records, &tickets);
    assert_capture_order(&records);
    assert!(gaps(&records).is_empty(), "nothing was lost");
    let busy = findings(&records, LedgerCode::LedgerCaptureBufferBusy);
    assert_eq!(
        busy.iter().map(|(edge, _)| *edge).collect::<Vec<_>>(),
        vec![
            Some(LedgerSegmentEdge::Started),
            Some(LedgerSegmentEdge::Ended)
        ]
    );
    assert_eq!(outcome_value(&busy[0].1, "count"), Some(json!(60)));
    let spills = findings(&records, LedgerCode::LedgerCaptureSpilled);
    assert_eq!(
        spills.iter().map(|(edge, _)| *edge).collect::<Vec<_>>(),
        vec![
            Some(LedgerSegmentEdge::Started),
            Some(LedgerSegmentEdge::Ended)
        ]
    );
    assert_eq!(outcome_value(&spills[1].1, "count"), Some(json!(8)));
    assert!(!root.dump_path().exists(), "the dump file is deleted");
    assert!(stops.try_recv().is_err(), "the writer does not stop");
    ledger.close().expect("close");
}

// The tier a writer with `ratios` reaches after `records` captures while its flush is held.
fn tier_after(ratios: WatermarkRatios, records: usize) -> CaptureTier {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(
            1,
            LedgerCapturePolicy {
                ratios,
                ..policy(RECORDS)
            },
            DUMP_MAX_BYTES,
            &reports,
        ))
        .expect("the writer opens");
    let capture = ledger.capture_handle(fingerprinter());
    let stall = root.stall();
    capture.capture(observation(&ids));
    assert!(stall.wait_held_commit(GATE_WAIT));
    for _ in 0..records {
        capture.capture(observation(&ids));
    }
    let tier = capture.tier();
    stall.release();
    ledger.close().expect("close");
    tier
}

// AO2-b: memory going back and forth between the tier-2 activation line and its release line
// starts the spill segment once and ends it once. The tiers follow their configuration. A
// configuration whose tier 1 is not below tier 2, or whose tiers 2 and 3 leave less than one
// spill batch between them, is refused with `invalid_ledger_config`.
#[test]
fn ao2_b_one_spill_segment_while_memory_moves_between_the_tier_2_lines() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    let stall = root.stall();
    capture.capture(observation(&ids));
    assert!(stall.wait_held_commit(GATE_WAIT));
    // 74 records and the busy finding: 75 in memory, tier 2; the spill takes memory to 67 and
    // its start finding makes 68.
    for _ in 0..74 {
        capture.capture(observation(&ids));
    }
    assert!(faults.wait_spills_written(1, GATE_WAIT));
    assert_eq!(ledger.capture_status().memory_records, 68);
    // Twice more up to the activation line and back to the release line.
    for spills in 2..=3 {
        for _ in 0..7 {
            capture.capture(observation(&ids));
        }
        assert!(faults.wait_spills_written(spills, GATE_WAIT));
        assert_eq!(ledger.capture_status().memory_records, 67);
        capture.capture(observation(&ids));
    }
    stall.release();
    let records = settled(&ledger, head);
    assert_capture_order(&records);
    assert_eq!(
        findings(&records, LedgerCode::LedgerCaptureSpilled)
            .iter()
            .map(|(edge, _)| *edge)
            .collect::<Vec<_>>(),
        vec![
            Some(LedgerSegmentEdge::Started),
            Some(LedgerSegmentEdge::Ended)
        ],
        "one start and one end"
    );
    ledger.close().expect("close");

    // Two tier configurations: tier 1 activates at its configured ratio.
    let default_ratios = LedgerCapturePolicy::default().ratios;
    let lower = WatermarkRatios {
        tier1: 0.40,
        ..default_ratios
    };
    assert_eq!(tier_after(default_ratios, 59), CaptureTier::BelowTier1);
    assert_eq!(tier_after(default_ratios, 60), CaptureTier::Tier1);
    assert_eq!(tier_after(lower, 39), CaptureTier::BelowTier1);
    assert_eq!(tier_after(lower, 40), CaptureTier::Tier1);

    // Refused configurations.
    for broken in [
        LedgerCapturePolicy {
            ratios: WatermarkRatios {
                tier1: 0.75,
                ..default_ratios
            },
            ..policy(RECORDS)
        },
        LedgerCapturePolicy {
            buffer_bytes: 16 * 1024 * 1024,
            spill_batch_bytes: 4 * 1024 * 1024,
            ..policy(RECORDS)
        },
    ] {
        let root = Root::new();
        let refused = root
            .open(root.plan(1, broken, DUMP_MAX_BYTES, &reports))
            .expect_err("the configuration is refused");
        assert_eq!(refused.code(), LedgerCode::InvalidLedgerConfig.as_str());
    }
}

// AO2-c: decision and observation records captured in a fixed interleaving are in the ledger in
// their capture order.
#[test]
fn ao2_c_decision_and_observation_records_keep_their_capture_order() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let capture = ledger.capture_handle(fingerprinter());
    let stall = root.stall();
    let mut tickets = vec![capture.capture(decision(&ids))];
    assert!(stall.wait_held_commit(GATE_WAIT));
    for decides in [false, true, false, false, true, true, false] {
        tickets.push(capture.capture(if decides {
            decision(&ids)
        } else {
            observation(&ids)
        }));
    }
    stall.release();
    let records = settled(&ledger, head);
    assert_eq!(records.len(), tickets.len());
    assert_eq!(
        records
            .iter()
            .map(|record| *record.event_id())
            .collect::<Vec<_>>(),
        tickets
            .iter()
            .map(|ticket| ticket.event_id)
            .collect::<Vec<_>>()
    );
    assert_tickets_written(&records, &tickets);
    assert_capture_order(&records);
    ledger.close().expect("close");
}

// AO2-d: the flush barrier on a captured record waits while the flush is held and returns once
// the record is committed, with its sequence; a capture meanwhile returns at once; the barrier
// itself writes nothing (the capture count is unchanged).
#[test]
fn ao2_d_the_flush_barrier_waits_for_its_record_and_writes_nothing() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let capture = ledger.capture_handle(fingerprinter());
    let stall = root.stall();
    let waited = capture.capture(decision(&ids));
    assert!(stall.wait_held_commit(GATE_WAIT));

    let (done, finished) = mpsc::channel();
    let barrier = capture.clone();
    let waiter = thread::spawn(move || {
        let _ = done.send(barrier.wait_committed(&waited));
    });
    let other = capture.capture(observation(&ids));
    assert_eq!(other.capture_seq, waited.capture_seq + 1);
    assert!(
        finished.try_recv().is_err(),
        "the barrier waits while the record's commit is held"
    );
    let captured = ledger.capture_status().captured;

    stall.release();
    let committed = finished
        .recv_timeout(GATE_WAIT)
        .expect("the barrier returns")
        .expect("the record is committed");
    waiter.join().expect("the waiter ends");
    assert_eq!(committed.event_id, waited.event_id);
    let records = settled(&ledger, head);
    let record = records
        .iter()
        .find(|record| record.event_id() == &waited.event_id)
        .expect("the record");
    assert_eq!(committed.sequence, record.sequence());
    assert_eq!(
        ledger.capture_status().captured,
        captured,
        "the barrier captured nothing"
    );
    assert_eq!(records.len(), 2);
    ledger.close().expect("close");
}

// AO2-e: per (source, lineage) the revisions are contiguous. A jump from 7 to 9 without a
// refused record's gap is `ledger_store_revision_skipped`; a hole a refused record's gap covers
// is not; a new lineage after its base record is not; the end of an earlier lineage that never
// continues is not. The check is the same for every registered source.
#[test]
fn ao2_e_a_revision_hole_is_reported_unless_a_gap_covers_it() {
    for source in [
        LedgerSource::InstanceFacts,
        LedgerSource::RuntimeFacts,
        LedgerSource::DecisionMemory,
        LedgerSource::FactRefreshTransitions,
    ] {
        let root = Root::new();
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let (reports, _stops) = failure_sink();
        let ledger = root
            .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
            .expect("the writer opens");
        let head = ledger.latest_sequence().expect("head");
        let capture = ledger.capture_handle(fingerprinter());
        let skipped = epoch();
        let covered = epoch();
        let fresh = epoch();
        for number in (1..=7).chain([9]) {
            capture.capture(observation_from(&ids, revision(source, skipped, number)));
        }
        capture.capture(observation_from(&ids, revision(source, covered, 1)));
        capture.capture(refused(&ids, revision(source, covered, 2)));
        capture.capture(observation_from(&ids, revision(source, covered, 3)));
        capture.capture(base_record(&ids, source, fresh));
        for number in 1..=2 {
            capture.capture(observation_from(&ids, revision(source, fresh, number)));
        }
        let records = settled(&ledger, head);
        let reported = findings(&records, LedgerCode::LedgerStoreRevisionSkipped);
        assert_eq!(reported.len(), 1, "one hole is reported");
        let outcome = &reported[0].1;
        assert_eq!(
            outcome_value(outcome, "ledger_source"),
            Some(serde_json::to_value(source).expect("source"))
        );
        assert_eq!(
            outcome_value(outcome, "source_lineage"),
            Some(serde_json::to_value(skipped).expect("lineage"))
        );
        assert_eq!(outcome_value(outcome, "expected_revision"), Some(json!(8)));
        assert_eq!(outcome_value(outcome, "source_revision"), Some(json!(9)));
        ledger.close().expect("close");
    }
}

// The last committed record's time before a writer stops without a dump.
fn stop_without_dump(root: &Root, ids: &IdentifierIssuer) -> u64 {
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let capture = ledger.capture_handle(fingerprinter());
    let committed = capture
        .wait_committed(&capture.capture(observation(ids)))
        .expect("committed");
    let last = ledger
        .query(EventQuery {
            from_sequence: Some(committed.sequence),
            ..EventQuery::default()
        })
        .expect("the record")
        .last()
        .expect("a record")
        .timestamp_unix_ms();
    let faults = root.faults();
    faults.refuse_dump_at(&root.dump_path());
    faults.refuse_dump_at(&root.fallback_path());
    faults.fail_writes(1);
    capture.capture(observation(ids));
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerDumpFailed), "no dump is written");
    assert!(ledger.close().is_err());
    last
}

// AO2-f: a start after an epoch that ended without its close and without a complete dump writes
// one `ledger_tail_unconfirmed` gap from the last committed record's time to this start, the count
// unknown. An epoch that ended in a complete dump gets no tail gap.
#[test]
fn ao2_f_an_unconfirmed_tail_gets_one_gap_and_a_complete_dump_none() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let last = stop_without_dump(&root, &ids);
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the next start");
    let records = after(&ledger, 0);
    let tails = gaps(&records)
        .into_iter()
        .filter_map(|record| match record.payload() {
            EventPayload::Ledger(LedgerPayload::GapRecorded(gap))
                if gap.gap_kind() == LedgerGapKind::TailUnconfirmed =>
            {
                Some(gap.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tails.len(), 1, "one tail gap");
    assert_eq!(tails[0].gap_start_unix_ms(), last);
    assert!(tails[0].gap_end_unix_ms() >= last);
    assert_eq!(tails[0].record_count(), None, "the count is unknown");
    ledger.close().expect("close");

    // An epoch that ended in a complete dump.
    let root = Root::new();
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    faults.fail_writes(1);
    capture.capture(observation(&ids));
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    assert!(ledger.close().is_err());
    drop(faults);
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the next start imports the dump");
    let records = after(&ledger, 0);
    assert!(
        records
            .iter()
            .any(|record| record.event_type() == EventType::LedgerDumpImported)
    );
    assert!(
        gaps(&records).is_empty(),
        "an epoch that ended in a complete dump has no tail gap"
    );
    ledger.close().expect("close");
}

// AO2-g: a capture that fails sanitizing on the flush thread is not written: a single-record
// `record_rejected` gap at its capture time and number takes its place with
// `ledger_record_rejected`; the records around it are written and the capture itself returned
// as usual.
#[test]
fn ao2_g_a_refused_capture_becomes_its_gap_and_the_others_are_written() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let capture = ledger.capture_handle(fingerprinter());
    let before = capture.capture(observation(&ids));
    let refused_ticket = capture.capture(refused(&ids, None));
    let behind = capture.capture(observation(&ids));
    let records = settled(&ledger, head);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].event_id(), &before.event_id);
    assert_eq!(records[2].event_id(), &behind.event_id);
    let gap = &records[1];
    assert_eq!(capture_seq(gap), refused_ticket.capture_seq);
    let EventPayload::Ledger(LedgerPayload::GapRecorded(payload)) = gap.payload() else {
        panic!("a gap record");
    };
    assert_eq!(payload.gap_kind(), LedgerGapKind::RecordRejected);
    assert_eq!(payload.record_count(), Some(1));
    assert_eq!(payload.event_id(), Some(&refused_ticket.event_id));
    assert_eq!(payload.gap_start_unix_ms(), payload.gap_end_unix_ms());
    assert_eq!(payload.gap_start_unix_ms(), gap.timestamp_unix_ms());
    let outcome = gap.payload().outcome().expect("the gap's outcome");
    assert!(outcome.is(LedgerCode::LedgerRecordRejected));
    assert_eq!(
        outcome_value(outcome, "event_id"),
        Some(json!(id_text(&refused_ticket.event_id)))
    );
    assert!(stops.try_recv().is_err(), "the writer goes on");
    ledger.close().expect("close");
}

// The committed batches after the first record of a writer whose flush batch is `records`. The
// records are direct appends: their drafts are sanitized already, so the flush takes them in
// batches of exactly the configured size.
fn batches_of(records: u64, appends: usize) -> Vec<usize> {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(
            1,
            LedgerCapturePolicy {
                flush_batch_records: records,
                ..policy(RECORDS)
            },
            DUMP_MAX_BYTES,
            &reports,
        ))
        .expect("the writer opens");
    let faults = root.faults();
    let stall = root.stall();
    ledger
        .append_deferred(sanitized(&ids, observation_payload()))
        .expect("accepted");
    assert!(stall.wait_held_commit(GATE_WAIT));
    for _ in 0..appends {
        ledger
            .append_deferred(sanitized(&ids, observation_payload()))
            .expect("accepted");
    }
    stall.release();
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("the replies");
    assert_eq!(summary.committed.len(), appends + 1);
    let batches = faults
        .commits()
        .into_iter()
        .skip(1)
        .map(|(sequences, _)| sequences.len())
        .collect();
    ledger.close().expect("close");
    batches
}

// A writer whose dump may hold `dump_max_bytes`, its flush held, its first spill reached.
fn spill_with_limit(
    dump_max_bytes: u64,
) -> (
    Root,
    GlobalLedger,
    LedgerWriteFaultGate,
    WriterStallGate,
    mpsc::Receiver<Outcome>,
) {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), dump_max_bytes, &reports))
        .expect("the writer opens");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    let stall = root.stall();
    capture.capture(observation(&ids));
    assert!(stall.wait_held_commit(GATE_WAIT));
    for _ in 0..74 {
        capture.capture(observation(&ids));
    }
    assert!(faults.wait_spill_reached(GATE_WAIT));
    (root, ledger, faults, stall, stops)
}

// AO2-h: the memory segment's budget, the flush batch and the dump file's limit come from the
// configuration (two configurations each).
#[test]
fn ao2_h_the_budget_the_batches_and_the_dump_limit_follow_the_configuration() {
    // The budget: tier 1 at 60 % of `buffer_records`.
    for (records, activation) in [(RECORDS, 60), (50, 30)] {
        let root = Root::new();
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let (reports, _stops) = failure_sink();
        let ledger = root
            .open(root.plan(1, policy(records), DUMP_MAX_BYTES, &reports))
            .expect("the writer opens");
        let capture = ledger.capture_handle(fingerprinter());
        let stall = root.stall();
        capture.capture(observation(&ids));
        assert!(stall.wait_held_commit(GATE_WAIT));
        for _ in 1..activation {
            capture.capture(observation(&ids));
        }
        assert_eq!(capture.tier(), CaptureTier::BelowTier1);
        capture.capture(observation(&ids));
        assert_eq!(capture.tier(), CaptureTier::Tier1);
        stall.release();
        ledger.close().expect("close");
    }
    // The flush batch below tier 1.
    assert_eq!(batches_of(4, 10), vec![4, 4, 2]);
    assert_eq!(batches_of(3, 10), vec![3, 3, 3, 1]);
    // The dump file's limit: a spill that fits is written, one that does not stops the writer.
    let (_root, ledger, faults, stall, stops) = spill_with_limit(DUMP_MAX_BYTES);
    assert!(faults.wait_spills_written(1, GATE_WAIT));
    assert!(stops.try_recv().is_err(), "the spill fits");
    stall.release();
    ledger.close().expect("close");
    let (_root, ledger, _faults, stall, stops) = spill_with_limit(1024);
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    stall.release();
    assert!(ledger.close().is_err());
}

// AO2-i: the close: with records in the dump segment and in memory and the close record captured
// last, the close drains both and then closes the ledger; the close record is the epoch's last
// record and the index's `last_close`.
#[test]
fn ao2_i_the_close_drains_both_segments_and_the_close_record_is_last() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    let stall = root.stall();
    let mut tickets = vec![capture.capture(observation(&ids))];
    assert!(stall.wait_held_commit(GATE_WAIT));
    for _ in 0..74 {
        tickets.push(capture.capture(observation(&ids)));
    }
    assert!(faults.wait_spills_written(1, GATE_WAIT));
    for _ in 0..3 {
        tickets.push(capture.capture(observation(&ids)));
    }
    let close = capture.capture(close_record(&ids));
    tickets.push(close);
    assert!(ledger.capture_status().dump_records > 0);
    assert!(ledger.capture_status().memory_records > 0);

    let (closed, finished) = mpsc::channel();
    let closer = thread::spawn(move || {
        let _ = closed.send(ledger.close());
    });
    stall.release();
    finished
        .recv_timeout(GATE_WAIT)
        .expect("the close returns")
        .expect("the close drains and closes");
    closer.join().expect("the closer ends");
    assert!(!root.dump_path().exists(), "the dump segment is drained");

    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the next start");
    let records = after(&ledger, head);
    assert_tickets_written(&records, &tickets);
    let epoch_records = records
        .iter()
        .filter(|record| record.capture().is_some())
        .collect::<Vec<_>>();
    let last = epoch_records.last().expect("the epoch's records");
    assert_eq!(last.event_id(), &close.event_id, "the close record is last");
    assert_eq!(
        root.index_row("lifecycle", "last_close"),
        Some((last.sequence(), id_text(&close.event_id)))
    );
    ledger.close().expect("close");
}

// AO2-j: several threads capture at once (released together by a barrier): ledger order is
// capture order and the capture numbers have no hole.
#[test]
fn ao2_j_concurrent_captures_keep_capture_order_without_holes() {
    const THREADS: usize = 4;
    const EACH: usize = 50;
    let root = Root::new();
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(1_000), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let start = Arc::new(Barrier::new(THREADS));
    let workers = (0..THREADS)
        .map(|_| {
            let capture = ledger.capture_handle(fingerprinter());
            let start = Arc::clone(&start);
            thread::spawn(move || {
                let ids = IdentifierIssuer::new().expect("identifier issuer");
                start.wait();
                (0..EACH)
                    .map(|_| capture.capture(observation(&ids)))
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let tickets = workers
        .into_iter()
        .flat_map(|worker| worker.join().expect("a capturing thread"))
        .collect::<Vec<_>>();
    let records = settled(&ledger, head);
    assert_eq!(records.len(), THREADS * EACH);
    assert_tickets_written(&records, &tickets);
    assert_eq!(
        records.iter().map(capture_seq).collect::<Vec<_>>(),
        (1..=(THREADS * EACH) as u64).collect::<Vec<_>>(),
        "ledger order is capture order, without holes"
    );
    ledger.close().expect("close");
}

// AO2-k: with the spill thread held, memory crosses tier 3: `ledger_capture_buffer_overloaded`
// starts and captures still return at once. Memory at the hard limit stops the writer by §9.2:
// the dump holds every record not committed, the ledger's own `runtime.failed` last. A spill that
// cannot be written stops it too, and a dump that cannot be written either is
// `ledger_dump_failed`.
#[test]
fn ao2_k_a_held_spill_reaches_tier_3_and_the_hard_limit_stops_by_the_dump() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    faults.hold_spills();
    let stall = root.stall();
    let mut tickets = vec![capture.capture(observation(&ids))];
    assert!(stall.wait_held_commit(GATE_WAIT));
    for _ in 0..74 {
        tickets.push(capture.capture(observation(&ids)));
    }
    assert!(faults.wait_spill_reached(GATE_WAIT));
    assert_eq!(ledger.capture_status().memory_records, 67);
    // 23 more: 90 in memory, tier 3; its finding makes 91.
    for _ in 0..23 {
        tickets.push(capture.capture(observation(&ids)));
    }
    assert_eq!(capture.tier(), CaptureTier::Tier3);
    assert_eq!(ledger.capture_status().memory_records, 91);
    // 9 more: 100 in memory, the hard limit.
    for _ in 0..9 {
        tickets.push(capture.capture(observation(&ids)));
    }
    assert_eq!(ledger.capture_status().memory_records, 100);
    assert!(
        stops.try_recv().is_err(),
        "the held spill thread stops nothing yet"
    );
    faults.release_spills();
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    assert_eq!(
        outcome_value(&stop, "path"),
        Some(json!(root.dump_path().to_string_lossy()))
    );
    let (lines, sealed) = read_dump(&root.dump_path());
    assert!(sealed, "the dump is sealed");
    let last = lines.last().expect("a record line");
    assert_eq!(last.event_type, event_type_value(EventType::RuntimeFailed));
    let mut dumped = lines
        .iter()
        .map(|line| line.event_id.clone())
        .collect::<Vec<_>>();
    dumped.sort_by_key(Value::to_string);
    for ticket in &tickets {
        let id = serde_json::to_value(ticket.event_id).expect("event id");
        assert!(
            dumped
                .binary_search_by_key(&id.to_string(), Value::to_string)
                .is_ok(),
            "every capture is in the dump"
        );
    }
    stall.release();
    assert!(ledger.close().is_err());

    // A spill that cannot be written, and a dump that cannot be written either.
    let root = Root::new();
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    faults.refuse_dump_at(&root.dump_path());
    faults.refuse_dump_at(&root.fallback_path());
    let stall = root.stall();
    capture.capture(observation(&ids));
    assert!(stall.wait_held_commit(GATE_WAIT));
    for _ in 0..74 {
        capture.capture(observation(&ids));
    }
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerDumpFailed));
    assert!(
        stop.causes()
            .iter()
            .any(|link| link.outcome().is(LedgerCode::LedgerWriteFailed))
    );
    stall.release();
    assert!(ledger.close().is_err());
}

// AO2-l: the index table. After a batch with tagged records the `state` row of a key names its
// latest record; a key the patterns do not tag has no row; an intent's commit adds its `open`
// row and its result's commit deletes it. A write that fails before its commit leaves neither the
// record nor its row.
#[test]
fn ao2_l_the_index_rows_follow_the_committed_records() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let capture = ledger.capture_handle(fingerprinter());
    let action = ids.mint_action_id().expect("action id");
    let links = EventLinksDraft::default().with_action_id(action);
    capture.capture(workbench(&ids, TAGGED_KEY, 1, None));
    let latest = capture.capture(workbench(&ids, TAGGED_KEY, 2, None));
    capture.capture(workbench(&ids, UNTAGGED_KEY, 1, None));
    let intent = capture.capture(item(
        &ids,
        ApplicationPayloadDraft::intent(EventAction::ApplicationLaunch, AuditInput::new()).into(),
        links.clone(),
        None,
    ));
    let records = settled(&ledger, head);
    let latest_record = records
        .iter()
        .find(|record| record.event_id() == &latest.event_id)
        .expect("the latest record");
    let tagged_key = format!("runtime_facts/runtime/{TAGGED_KEY}");
    assert_eq!(
        root.index_row("state", &tagged_key),
        Some((latest_record.sequence(), id_text(&latest.event_id)))
    );
    assert_eq!(
        root.index_row("state", &format!("runtime_facts/runtime/{UNTAGGED_KEY}")),
        None
    );
    let intent_id = id_text(&intent.event_id);
    assert!(root.index_row("open", &intent_id).is_some());
    capture.capture(item(
        &ids,
        ApplicationPayloadDraft::completed(
            EventAction::ApplicationLaunch,
            EffectDisposition::Performed,
            AuditInput::new(),
        )
        .into(),
        links,
        None,
    ));
    settled(&ledger, head);
    assert_eq!(
        root.index_row("open", &intent_id),
        None,
        "the result deletes the intent's row"
    );

    let faults = root.faults();
    faults.fail_writes(1);
    let lost = capture.capture(workbench(&ids, TAGGED_KEY, 3, None));
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    assert!(!root.holds_record(&lost.event_id), "no record");
    assert_eq!(
        root.index_row("state", &tagged_key),
        Some((latest_record.sequence(), id_text(&latest.event_id))),
        "no row of the failed write"
    );
    assert!(ledger.close().is_err());
}

// AO2-m: a tagged record commits on arrival: alone when nothing else waits, and at the end of its
// batch when records wait behind it, FULL synced either way; a batch of observation records is
// NORMAL synced.
#[test]
fn ao2_m_a_tagged_record_commits_on_arrival_with_full_sync() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, _stops) = failure_sink();
    let ledger = root
        .open(root.plan(1, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    let alone = capture
        .wait_committed(&capture.capture(workbench(&ids, TAGGED_KEY, 1, None)))
        .expect("committed");
    assert_eq!(faults.commits(), vec![(vec![alone.sequence], true)]);

    // Direct appends: sanitized already, they are taken in one batch up to the tagged record.
    let stall = root.stall();
    ledger
        .append_deferred(sanitized(&ids, observation_payload()))
        .expect("accepted");
    assert!(stall.wait_held_commit(GATE_WAIT));
    for payload in [
        observation_payload(),
        observation_payload(),
        workbench_payload(TAGGED_KEY, 2),
        observation_payload(),
        observation_payload(),
    ] {
        ledger
            .append_deferred(sanitized(&ids, payload))
            .expect("accepted");
    }
    stall.release();
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("the replies");
    let sequences = summary
        .committed
        .iter()
        .map(|committed| committed.sequence())
        .collect::<Vec<_>>();
    assert_eq!(sequences.len(), 6);
    assert_eq!(
        faults.commits()[1..].to_vec(),
        vec![
            (sequences[..1].to_vec(), false),
            (sequences[1..4].to_vec(), true),
            (sequences[4..].to_vec(), false),
        ],
        "the batch ends at the tagged record, FULL synced"
    );
    ledger.close().expect("close");
}

// AO2-n: a fatal stop for a reason outside the ledger: its `runtime.failed`, captured as G16
// captures it, is drained before the writer closes and is the epoch's last record. When the
// store fails every attempt of that drain, the dump holds that record and the ledger's own
// `runtime.failed` (`ledger_write_failed`), in capture order.
#[test]
fn ao2_n_a_fatal_record_is_drained_last_or_dumped_in_capture_order() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = failure_sink();
    let ledger = root
        .open(root.plan(2, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let capture = ledger.capture_handle(fingerprinter());
    for _ in 0..3 {
        capture.capture(observation(&ids));
    }
    let failed = capture.capture(runtime_failed(&ids));
    ledger.close().expect("the close drains");
    let ledger = root
        .open(root.plan(2, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the next start");
    let records = after(&ledger, head);
    let last = records
        .iter()
        .rfind(|record| record.capture().is_some())
        .expect("the epoch's records");
    assert_eq!(last.event_id(), &failed.event_id);
    assert_eq!(last.event_type(), EventType::RuntimeFailed);
    ledger.close().expect("close");

    // Every attempt of the drain fails.
    let root = Root::new();
    let ledger = root
        .open(root.plan(2, policy(RECORDS), DUMP_MAX_BYTES, &reports))
        .expect("the writer opens");
    let capture = ledger.capture_handle(fingerprinter());
    let faults = root.faults();
    let stall = root.stall();
    let first = capture.capture(observation(&ids));
    assert!(stall.wait_held_commit(GATE_WAIT));
    faults.fail_writes(10);
    let failed = capture.capture(runtime_failed(&ids));
    stall.release();
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    let (lines, sealed) = read_dump(&root.dump_path());
    assert!(sealed);
    assert_eq!(
        lines
            .iter()
            .map(|line| line.capture_seq)
            .collect::<Vec<_>>(),
        vec![
            first.capture_seq,
            failed.capture_seq,
            failed.capture_seq + 1
        ],
        "in capture order"
    );
    assert_eq!(
        lines[1].event_id,
        serde_json::to_value(failed.event_id).expect("event id")
    );
    assert_eq!(
        lines[1].event_type,
        event_type_value(EventType::RuntimeFailed)
    );
    assert_eq!(
        lines[2].event_type,
        event_type_value(EventType::RuntimeFailed)
    );
    assert!(ledger.close().is_err());
}
