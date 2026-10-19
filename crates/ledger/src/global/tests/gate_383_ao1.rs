// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 AO-1, test rows AO1-a to AO1-k (model-ledger-observer v1.4.1 §14.2;
// `contracts/ledger-observer.md` §9, §10): a failed ledger write is read back and tried again at
// once; when the configured attempts are used up, or the store reports an integrity failure,
// every record not yet committed goes into the dump file, the ledger's own `runtime.failed`
// last, and the writer stops through its failure plan's sink; the next start imports the dump
// before any new record, skips what the ledger holds already, records the import and deletes the
// dump.
//
// The store is a formal SQLite root (the candidate root for AO1-f). Write failures come from the
// write fault gate, a held attempt from the writer stall gate; both are released by the test,
// never by a timer. The stall watch reads a clock the test drives. `GATE_WAIT` only bounds a
// broken run. Every wait between attempts is 0 ms.

use crate::global::{
    GlobalLedger, GlobalLedgerConfig, GlobalLedgerError, LEDGER_DUMP_FILE_NAME,
    LEDGER_DUMP_VERSION, LedgerFailurePlan, LedgerFailureSink, LedgerMaintenance, LedgerOpenTiming,
    LedgerWritePolicy, Sha256SecretFingerprinter, fallback_dump_path,
};
use crate::{LedgerCode, LedgerWriteFaultGate, PersistedEvent, WriterStallGate};
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventId, EventLinksDraft, EventOrigin, EventQuery,
    EventSeverity, EventSource, EventType, IdentifierIssuer, MonitorPayloadDraft, OriginModule,
    OwnerEpoch, SanitizedEventDraft,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tempfile::TempDir;

const GATE_WAIT: Duration = Duration::from_secs(30);
// The State integrity key and the writer's owner id: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-ao1-fixture";
const OWNER: &str = "ledger-ao1-fixture";
// TCLOCK: the records' time and the epoch's start.
const AT_MS: u64 = 1_752_147_200_000;
// The commit the dump's header names: a neutral 40-hex value.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
// `flush_attempt_stall_ms`, `health_probe_interval_ms` and `dump_max_bytes` of the plans here.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
// How long a read waits for the held writer in AO1-f before it gives up.
const READ_WAIT: Duration = Duration::from_millis(200);

// What the writer reports through its failure plan's sink.
struct Reports {
    retried: Mutex<Vec<Outcome>>,
    stops: Mutex<mpsc::Sender<Outcome>>,
}

impl LedgerFailureSink for Reports {
    fn write_retried(&self, outcome: &Outcome) {
        self.retried
            .lock()
            .expect("retried reports")
            .push(outcome.clone());
    }

    fn stop(&self, outcome: &Outcome) {
        let _ = self
            .stops
            .lock()
            .expect("stop reports")
            .send(outcome.clone());
    }
}

fn reports() -> (Arc<Reports>, mpsc::Receiver<Outcome>) {
    let (stops, stopped) = mpsc::channel();
    (
        Arc::new(Reports {
            retried: Mutex::new(Vec::new()),
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

// `monitor.probe_requested`, an observational record without links.
fn draft(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        MonitorPayloadDraft::requested(AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"ledger-ao1-fixture").expect("salt"))
    .expect("an observational draft")
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
        // The writer opens once and closes, as on a formal root (the assembly of
        // `missing_material.rs`): its owner metadata is what every later start reads.
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

    // `attempts` attempts with no wait between them.
    fn plan(&self, attempts: u32, reports: &Arc<Reports>) -> LedgerFailurePlan {
        LedgerFailurePlan::new(
            LedgerWritePolicy::new(
                attempts,
                vec![0; attempts as usize],
                STALL_MS,
                PROBE_MS,
                DUMP_MAX_BYTES,
            )
            .expect("write policy"),
            &self.state_root(),
            Some(self.fallback.path()),
            epoch(),
            AT_MS,
        )
        .with_runtime_commit(COMMIT)
        .with_sink(Arc::clone(reports) as Arc<dyn LedgerFailureSink>)
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
}

fn after(ledger: &GlobalLedger, head: u64) -> Vec<PersistedEvent> {
    ledger
        .query(EventQuery {
            from_sequence: Some(head + 1),
            ..EventQuery::default()
        })
        .expect("ledger records")
}

fn value(outcome: &Outcome, key: &str) -> Option<Value> {
    outcome.value(key).cloned()
}

fn path_value(path: &Path) -> Value {
    json!(path.to_string_lossy())
}

// One record line as written: the record member's exact bytes and their hash.
#[derive(Deserialize)]
struct Line<'a> {
    #[serde(borrow)]
    record: &'a RawValue,
    sha256: String,
}

// A dump as the test reads it; every line's hash and the trailer's total hash are checked.
struct Dump {
    header: Value,
    records: Vec<Value>,
    trailer: Option<Value>,
}

impl Dump {
    fn read(path: &Path) -> Self {
        let text = fs::read_to_string(path).expect("the dump file");
        let mut lines = text.lines();
        let header = serde_json::from_str(lines.next().expect("a header")).expect("header");
        let mut records = Vec::new();
        let mut trailer = None;
        let mut hash = Sha256::new();
        for line in lines {
            let parsed: Value = serde_json::from_str(line).expect("a JSON line");
            if let Some(found) = parsed.get("trailer") {
                trailer = Some(found.clone());
                continue;
            }
            let line_in: Line<'_> = serde_json::from_str(line).expect("a record line");
            assert_eq!(
                format!("{:x}", Sha256::digest(line_in.record.get().as_bytes())),
                line_in.sha256,
                "every record line's hash verifies"
            );
            hash.update(line.as_bytes());
            hash.update(b"\n");
            records.push(serde_json::from_str(line_in.record.get()).expect("a record"));
        }
        if let Some(trailer) = &trailer {
            assert_eq!(
                trailer["lines_sha256"],
                json!(format!("{:x}", hash.finalize())),
                "the trailer's hash covers the record lines"
            );
            assert_eq!(trailer["count"], json!(records.len()));
        }
        Self {
            header,
            records,
            trailer,
        }
    }

    fn event_ids(&self) -> Vec<Value> {
        self.records
            .iter()
            .map(|record| record["draft"]["event_id"].clone())
            .collect()
    }
}

fn id_value(id: &EventId) -> Value {
    serde_json::to_value(id).expect("event id")
}

fn event_type_value(event_type: EventType) -> Value {
    serde_json::to_value(event_type).expect("event type")
}

// The last record of a sealed dump is the ledger's own `runtime.failed` with
// `ledger_write_failed`.
fn assert_ends_with_the_fatal_record(dump: &Dump) {
    let last = dump.records.last().expect("a record");
    assert_eq!(
        last["draft"]["event_type"],
        event_type_value(EventType::RuntimeFailed)
    );
    assert!(
        last["draft"]["payload"]
            .to_string()
            .contains(LedgerCode::LedgerWriteFailed.as_str()),
        "the fatal record carries ledger_write_failed"
    );
    let captures = dump
        .records
        .iter()
        .map(|record| record["capture_seq"].clone())
        .collect::<Vec<_>>();
    let expected = (1..=dump.records.len())
        .map(|seq| json!(seq))
        .collect::<Vec<_>>();
    assert_eq!(captures, expected, "records are in capture order from 1");
}

// What one real exhausted write left: the dump's record ids in capture order (the fatal
// record's last), its bytes, and the ledger head before it.
struct Dumped {
    ids: Vec<Value>,
    bytes: Vec<u8>,
    head: u64,
}

// A dump written by a real exhausted write: the first record fails every one of its two
// attempts while two more appends wait behind it.
fn dump_by_failure(root: &Root, ids: &IdentifierIssuer) -> Dumped {
    let (reports, stops) = reports();
    let ledger = root.open(root.plan(2, &reports)).expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let faults = root.faults();
    let stall = WriterStallGate::arm(root.directory.path()).expect("writer stall gate");
    let first = draft(ids);
    let queued = [draft(ids), draft(ids)];
    let mut expected = vec![id_value(first.event_id())];
    expected.extend(queued.iter().map(|draft| id_value(draft.event_id())));
    ledger
        .append_deferred(first)
        .expect("the first write is accepted");
    assert!(
        stall.wait_held_commit(GATE_WAIT),
        "the first write is held in its attempt"
    );
    for draft in queued {
        ledger.append_deferred(draft).expect("queued behind it");
    }
    faults.fail_writes(2);
    stall.release();
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("the replies");
    assert!(summary.committed.is_empty());
    assert_eq!(summary.failed.len(), 3);
    assert!(ledger.close().is_err(), "the writer ended with its failure");
    let dump = Dump::read(&root.dump_path());
    assert_eq!(dump.event_ids()[..3], expected[..]);
    Dumped {
        ids: dump.event_ids(),
        bytes: fs::read(root.dump_path()).expect("dump bytes"),
        head,
    }
}

// AO1-a: a write that fails twice, fewer times than its five attempts, commits on its third:
// the ledger holds the record once, one `ledger_write_retried` reports two attempts, no dump is
// written and the writer goes on.
#[test]
fn ao1_a_a_write_that_fails_fewer_times_than_its_attempts_commits_once() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = reports();
    let ledger = root.open(root.plan(5, &reports)).expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let faults = root.faults();
    faults.fail_writes(2);

    let record = draft(&ids);
    let id = *record.event_id();
    let committed = ledger.append(record).expect("the third attempt commits");

    assert_eq!(*committed.event_id(), id);
    assert_eq!(faults.met(), 2);
    let records = after(&ledger, head);
    assert_eq!(records.len(), 1);
    assert_eq!(*records[0].event_id(), id);
    let retried = reports.retried.lock().expect("retried reports").clone();
    assert_eq!(retried.len(), 1);
    assert!(retried[0].is(LedgerCode::LedgerWriteRetried));
    assert_eq!(value(&retried[0], "attempts"), Some(json!(2)));
    assert!(stops.try_recv().is_err(), "the writer does not stop");
    assert!(!root.dump_path().exists() && !root.fallback_path().exists());
    ledger
        .append(draft(&ids))
        .expect("the writer goes on with the next record");
    ledger.close().expect("close");
}

// AO1-b: a write whose two attempts both fail gives up: the dump holds that record, the two
// appends queued behind it in capture order and the ledger's own `runtime.failed`
// (`ledger_write_failed`) last; every line's hash and the trailer's verify; the writer stops
// through the sink with the dump's path; no append is reported committed.
#[test]
fn ao1_b_exhausted_attempts_dump_every_unflushed_record_and_stop() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = reports();
    let ledger = root.open(root.plan(2, &reports)).expect("the writer opens");
    let faults = root.faults();
    let stall = WriterStallGate::arm(root.directory.path()).expect("writer stall gate");
    let first = draft(&ids);
    let queued = [draft(&ids), draft(&ids)];
    let mut expected = vec![id_value(first.event_id())];
    expected.extend(queued.iter().map(|draft| id_value(draft.event_id())));
    ledger.append_deferred(first).expect("accepted");
    assert!(stall.wait_held_commit(GATE_WAIT));
    for draft in queued {
        ledger.append_deferred(draft).expect("queued");
    }
    faults.fail_writes(2);
    stall.release();

    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    assert_eq!(value(&stop, "attempts"), Some(json!(2)));
    assert_eq!(value(&stop, "io_kind"), Some(json!("storage_full")));
    assert_eq!(value(&stop, "path"), Some(path_value(&root.dump_path())));
    assert_eq!(faults.met(), 2);
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("the replies");
    assert!(
        summary.committed.is_empty(),
        "no append is reported committed"
    );
    assert_eq!(summary.failed.len(), 3);
    assert!(
        summary
            .failed
            .iter()
            .all(|error| error.code() == LedgerCode::LedgerWriteFailed.as_str())
    );
    assert!(ledger.close().is_err());

    let dump = Dump::read(&root.dump_path());
    assert_eq!(dump.header["dump_version"], json!(LEDGER_DUMP_VERSION));
    assert_eq!(dump.header["runtime_commit"], json!(COMMIT));
    assert_eq!(dump.header["started_at_unix_ms"], json!(AT_MS));
    assert_eq!(dump.records.len(), 4);
    assert_eq!(dump.event_ids()[..3], expected[..]);
    assert_ends_with_the_fatal_record(&dump);
    let trailer = dump.trailer.expect("a sealed dump has its trailer");
    assert_eq!(
        trailer["reason_code"],
        json!(LedgerCode::LedgerWriteFailed.as_str())
    );
}

// AO1-c: an attempt that committed while its result was reported lost is read back before any
// second attempt: the ledger holds the record once, the head moves to it and no second attempt
// is made.
#[test]
fn ao1_c_a_commit_reported_as_failed_is_read_back_not_written_again() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, _stops) = reports();
    let ledger = root.open(root.plan(5, &reports)).expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let faults = root.faults();
    faults.commit_then_fail(1);

    let record = draft(&ids);
    let id = *record.event_id();
    let committed = ledger.append(record).expect("the read back finds it");

    assert_eq!(*committed.event_id(), id);
    assert_eq!(committed.sequence(), head + 1);
    assert_eq!(faults.met(), 1, "no second attempt");
    let records = after(&ledger, head);
    assert_eq!(records.len(), 1);
    assert_eq!(*records[0].event_id(), id);
    assert_eq!(ledger.latest_sequence().expect("head"), head + 1);
    let next = ledger.append(draft(&ids)).expect("the writer goes on");
    assert_eq!(next.sequence(), head + 2);
    ledger.close().expect("close");
}

// AO1-d: an integrity failure of the store is not tried again: the writer stops at its first
// attempt and the dump holds the record in flight and the append queued behind it.
#[test]
fn ao1_d_an_integrity_failure_is_not_retried_and_dumps_the_unflushed_records() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports, stops) = reports();
    let ledger = root.open(root.plan(5, &reports)).expect("the writer opens");
    let faults = root.faults();
    let stall = WriterStallGate::arm(root.directory.path()).expect("writer stall gate");
    let first = draft(&ids);
    let second = draft(&ids);
    let expected = [id_value(first.event_id()), id_value(second.event_id())];
    ledger.append_deferred(first).expect("accepted");
    assert!(stall.wait_held_commit(GATE_WAIT));
    ledger.append_deferred(second).expect("queued");
    faults.integrity_failure(1);
    stall.release();

    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    assert_eq!(value(&stop, "attempts"), Some(json!(1)));
    assert!(
        stop.causes().iter().any(|link| {
            link.outcome().code().as_str() == LedgerCode::LedgerMetaMismatch.as_str()
        }),
        "the integrity failure is the cause"
    );
    assert_eq!(faults.met(), 1, "no second attempt");
    assert!(reports.retried.lock().expect("retried").is_empty());
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("the replies");
    assert!(summary.committed.is_empty());
    assert!(ledger.close().is_err());
    let dump = Dump::read(&root.dump_path());
    assert_eq!(dump.event_ids()[..2], expected[..]);
    assert_eq!(dump.records.len(), 3);
    assert_ends_with_the_fatal_record(&dump);
}

// AO1-e: the number of attempts follows the configuration: two and five.
#[test]
fn ao1_e_the_attempts_follow_the_configuration() {
    for attempts in [2_u32, 5] {
        let root = Root::new();
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let (reports, stops) = reports();
        let ledger = root
            .open(root.plan(attempts, &reports))
            .expect("the writer opens");
        let faults = root.faults();
        faults.fail_writes(10);

        let failed = ledger.append(draft(&ids)).expect_err("every attempt fails");

        assert_eq!(failed.code(), LedgerCode::LedgerWriteFailed.as_str());
        let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
        assert_eq!(value(&stop, "attempts"), Some(json!(attempts)));
        assert_eq!(faults.met(), attempts as usize);
        assert!(ledger.close().is_err());
    }
}

// AO1-f: an attempt that does not return (held at the stall gate) for longer than
// `flush_attempt_stall_ms` by the plan's clock is stopped by the probe: the dump holds the record
// in flight and the append queued behind it. A read waiting meanwhile gets the registered error
// `writer_response_timeout`, which is not fatal.
#[test]
fn ao1_f_a_stalled_attempt_is_dumped_by_the_probe_and_a_waiting_read_gets_an_error() {
    let root = tempfile::tempdir().expect("candidate root");
    let fallback = tempfile::tempdir().expect("fallback folder");
    let database = Arc::new(
        RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
            root.path(),
            b"ledger-ao1-stall",
            |_| Ok(()),
            |_| Ok(()),
        )
        .expect("candidate physical owner"),
    );
    let state_root = root.path().canonicalize().expect("state root");
    let clock = Arc::new(AtomicU64::new(0));
    let reading = Arc::clone(&clock);
    let (reports, stops) = reports();
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
            .expect("write policy"),
        &state_root,
        Some(fallback.path()),
        epoch(),
        AT_MS,
    )
    .with_runtime_commit(COMMIT)
    .with_sink(Arc::clone(&reports) as Arc<dyn LedgerFailureSink>)
    .with_clock(Arc::new(move || reading.load(Ordering::SeqCst)));
    let ledger = GlobalLedger::open_sqlite_candidate(
        GlobalLedgerConfig::new(root.path(), OWNER)
            .with_failure_plan(plan)
            .with_reply_timeout(READ_WAIT),
        Arc::clone(&database),
    )
    .expect("the writer opens");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let stall = WriterStallGate::arm(root.path()).expect("writer stall gate");
    let first = draft(&ids);
    let second = draft(&ids);
    let expected = [id_value(first.event_id()), id_value(second.event_id())];
    ledger.append_deferred(first).expect("accepted");
    assert!(stall.wait_held_commit(GATE_WAIT));
    ledger.append_deferred(second).expect("queued");

    let read = ledger
        .latest_sequence()
        .expect_err("the held writer does not answer");
    assert_eq!(read.code(), LedgerCode::WriterResponseTimeout.as_str());
    assert!(!read.is_fatal(), "a waiting read's error is not fatal");
    assert!(stops.try_recv().is_err(), "within the stall limit no stop");

    clock.store(STALL_MS + 1, Ordering::SeqCst);
    let stop = stops
        .recv_timeout(GATE_WAIT)
        .expect("the probe stops the writer");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    assert_eq!(value(&stop, "timeout_ms"), Some(json!(STALL_MS)));
    assert_eq!(value(&stop, "attempts"), Some(json!(1)));
    let dump_path = state_root.join(LEDGER_DUMP_FILE_NAME);
    assert_eq!(value(&stop, "path"), Some(path_value(&dump_path)));
    let dump = Dump::read(&dump_path);
    assert_eq!(dump.event_ids()[..2], expected[..]);
    assert_eq!(dump.records.len(), 3);
    assert_ends_with_the_fatal_record(&dump);

    stall.release();
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("the replies");
    assert!(summary.committed.is_empty());
    assert_eq!(summary.failed.len(), 2);
    assert!(ledger.close().is_err());
}

// AO1-g: a start with a dump imports it before any new record: its records in capture order
// right after the previous last record, then `ledger.dump_imported` with the count; the dump is
// deleted.
#[test]
fn ao1_g_a_start_imports_the_dump_in_capture_order_records_it_and_deletes_it() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let dumped = dump_by_failure(&root, &ids);

    let ledger = root
        .open(root.plan(5, &reports().0))
        .expect("the next start imports the dump");

    let records = after(&ledger, dumped.head);
    let imported = records
        .iter()
        .take(4)
        .map(|record| id_value(record.event_id()))
        .collect::<Vec<_>>();
    assert_eq!(imported, dumped.ids);
    assert_eq!(records[3].event_type(), EventType::RuntimeFailed);
    assert_eq!(records[4].event_type(), EventType::LedgerDumpImported);
    let outcome = records[4]
        .payload()
        .outcome()
        .expect("the import's outcome");
    assert!(outcome.is(LedgerCode::LedgerDumpImported));
    assert_eq!(value(outcome, "count"), Some(json!(4)));
    assert_eq!(value(outcome, "duplicate_count"), Some(json!(0)));
    assert_eq!(value(outcome, "dump_sealed"), Some(json!(true)));
    assert!(!root.dump_path().exists(), "the dump is deleted");
    ledger.close().expect("close");
}

// AO1-h: an import cut short (its third record's write gives up) leaves the dump as it was; the
// next start imports the rest: every event id once, `ledger.dump_imported` counts the two it
// imported and the two it skipped, and the dump is deleted.
#[test]
fn ao1_h_an_import_cut_short_is_finished_by_the_next_start_once_per_event() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let dumped = dump_by_failure(&root, &ids);
    let faults = root.faults();
    faults.pass_first(2);
    faults.fail_writes(1);

    let refused = root
        .open(root.plan(1, &reports().0))
        .expect_err("the import's third write gives up");
    assert_eq!(refused.code(), LedgerCode::LedgerWriteFailed.as_str());
    assert_eq!(
        fs::read(root.dump_path()).expect("the dump stays"),
        dumped.bytes
    );
    drop(faults);

    let ledger = root
        .open(root.plan(5, &reports().0))
        .expect("the next start finishes the import");
    let records = after(&ledger, dumped.head);
    for id in &dumped.ids {
        assert_eq!(
            records
                .iter()
                .filter(|record| id_value(record.event_id()) == *id)
                .count(),
            1,
            "every event id once"
        );
    }
    let imported = records
        .iter()
        .find(|record| record.event_type() == EventType::LedgerDumpImported)
        .expect("the import's record");
    let outcome = imported.payload().outcome().expect("the import's outcome");
    assert_eq!(value(outcome, "count"), Some(json!(2)));
    assert_eq!(value(outcome, "duplicate_count"), Some(json!(2)));
    assert!(!root.dump_path().exists());
    ledger.close().expect("close");
}

// AO1-i: a dump without its trailer whose last line is cut imports its verified part with a
// `dump_incomplete` gap of unknown count and is deleted; a dump whose second record line does
// not verify imports the first record with a `dump_invalid` gap and is renamed and kept,
// unchanged.
#[test]
fn ao1_i_a_damaged_dump_imports_its_verified_part_with_a_gap() {
    for invalid in [false, true] {
        let root = Root::new();
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let dumped = dump_by_failure(&root, &ids);
        let text = String::from_utf8(dumped.bytes.clone()).expect("UTF-8 dump");
        let lines = text.lines().collect::<Vec<_>>();
        // Header, four records, trailer.
        assert_eq!(lines.len(), 6);
        let damaged = if invalid {
            // The second record line's hash no longer matches its record.
            let line = lines[2];
            let at = line.find("\"sha256\":\"").expect("hash member") + 10;
            let flipped = if &line[at..=at] == "0" { "1" } else { "0" };
            let changed = format!("{}{}{}", &line[..at], flipped, &line[at + 1..]);
            let mut changed_lines = lines.clone();
            changed_lines[2] = &changed;
            format!("{}\n", changed_lines.join("\n"))
        } else {
            // No trailer, and the third record line cut in half.
            let cut = &lines[3][..lines[3].len() / 2];
            format!("{}\n{}\n{}\n{}", lines[0], lines[1], lines[2], cut)
        };
        fs::write(root.dump_path(), &damaged).expect("damage the dump");

        let ledger = root
            .open(root.plan(5, &reports().0))
            .expect("the next start imports the verified part");

        let records = after(&ledger, dumped.head);
        let kept = if invalid { 1 } else { 2 };
        let imported = records
            .iter()
            .take(kept)
            .map(|record| id_value(record.event_id()))
            .collect::<Vec<_>>();
        assert_eq!(imported, dumped.ids[..kept]);
        let gap = &records[kept];
        assert_eq!(gap.event_type(), EventType::LedgerGapRecorded);
        let gap_outcome = gap.payload().outcome().expect("the gap's outcome");
        let summary = &records[kept + 1];
        assert_eq!(summary.event_type(), EventType::LedgerDumpImported);
        let summary_outcome = summary.payload().outcome().expect("the import's outcome");
        assert_eq!(value(summary_outcome, "count"), Some(json!(kept)));
        assert_eq!(value(summary_outcome, "dump_sealed"), Some(json!(false)));
        let actingcommand_contract::EventPayload::Ledger(
            actingcommand_contract::LedgerPayload::GapRecorded(payload),
        ) = gap.payload()
        else {
            panic!("a gap record");
        };
        assert_eq!(payload.record_count(), None, "the lost count is unknown");
        assert!(!root.dump_path().exists());
        if invalid {
            assert!(gap_outcome.is(LedgerCode::LedgerDumpInvalid));
            let kept_path = value(gap_outcome, "kept_path").expect("kept_path");
            let kept_path = PathBuf::from(kept_path.as_str().expect("a path"));
            assert_eq!(
                fs::read_to_string(&kept_path).expect("the kept dump"),
                damaged,
                "the kept dump is unchanged"
            );
        } else {
            assert!(gap_outcome.is(LedgerCode::LedgerDumpIncomplete));
        }
        ledger.close().expect("close");
    }
}

// AO1-j: a start whose import cannot write (every attempt fails) refuses with
// `ledger_write_failed` and leaves the dump's bytes as they were; a dump of a newer
// `dump_version` refuses the start with `ledger_dump_unsupported` and stays untouched.
#[test]
fn ao1_j_a_start_refuses_a_dump_it_cannot_write_or_read_and_leaves_it_untouched() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let dumped = dump_by_failure(&root, &ids);
    let faults = root.faults();
    faults.fail_writes(10);
    let refused = root
        .open(root.plan(2, &reports().0))
        .expect_err("the import cannot write");
    assert_eq!(refused.code(), LedgerCode::LedgerWriteFailed.as_str());
    assert_eq!(fs::read(root.dump_path()).expect("the dump"), dumped.bytes);
    drop(faults);

    let text = String::from_utf8(dumped.bytes).expect("UTF-8 dump");
    let newer = text.replacen(
        &format!("\"dump_version\":{LEDGER_DUMP_VERSION}"),
        &format!("\"dump_version\":{}", LEDGER_DUMP_VERSION + 1),
        1,
    );
    assert_ne!(newer, text);
    fs::write(root.dump_path(), &newer).expect("a newer dump");
    let refused = root
        .open(root.plan(2, &reports().0))
        .expect_err("a newer dump refuses the start");
    assert_eq!(refused.code(), LedgerCode::LedgerDumpUnsupported.as_str());
    assert!(refused.is_fatal());
    assert_eq!(
        fs::read_to_string(root.dump_path()).expect("the dump"),
        newer
    );
}

// AO1-k: a state root that cannot take the dump (a full disk) sends it to the fallback folder,
// and the next start imports it from there; when neither place can take it the stop is
// `ledger_dump_failed` with both places, caused by `ledger_write_failed`.
#[test]
fn ao1_k_a_state_root_that_cannot_take_the_dump_sends_it_to_the_fallback_folder() {
    let root = Root::new();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (reports_one, stops) = reports();
    let ledger = root
        .open(root.plan(1, &reports_one))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let faults = root.faults();
    faults.refuse_dump_at(&root.dump_path());
    faults.fail_writes(1);
    let record = draft(&ids);
    let id = id_value(record.event_id());
    ledger.append(record).expect_err("the only attempt fails");
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    assert_eq!(
        value(&stop, "path"),
        Some(path_value(&root.fallback_path()))
    );
    assert!(!root.dump_path().exists());
    assert_eq!(Dump::read(&root.fallback_path()).event_ids()[0], id);
    assert!(ledger.close().is_err());
    drop(faults);

    let ledger = root
        .open(root.plan(1, &reports().0))
        .expect("the next start imports from the fallback folder");
    assert_eq!(id_value(after(&ledger, head)[0].event_id()), id);
    assert!(
        !root.fallback_path().exists(),
        "the fallback dump is deleted"
    );

    // Neither place can take the dump.
    let faults = root.faults();
    faults.refuse_dump_at(&root.dump_path());
    faults.refuse_dump_at(&root.fallback_path());
    ledger.close().expect("close");
    let (reports_two, stops) = reports();
    let ledger = root
        .open(root.plan(1, &reports_two))
        .expect("the writer opens");
    faults.fail_writes(1);
    ledger
        .append(draft(&ids))
        .expect_err("the only attempt fails");
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerDumpFailed));
    assert_eq!(
        value(&stop, "paths"),
        Some(json!([
            root.dump_path().to_string_lossy(),
            root.fallback_path().to_string_lossy()
        ]))
    );
    assert!(stop.causes().iter().any(|link| {
        link.relation().as_str() == CauseRelation::CausedBy.as_str()
            && link.outcome().is(LedgerCode::LedgerWriteFailed)
    }));
    assert!(!root.dump_path().exists() && !root.fallback_path().exists());
    assert!(ledger.close().is_err());
}

// #411 A24 (ledger key 50, register §1.139 A24-3): a record's capture time is its producer's, not
// monotonic in capture order: a run's diagnostic `artifact.created` is stamped with the run's
// start (`RUN_START_MS`) and written after a record of the run's end (`LATEST_MS`, the run took
// 329 s). A gap starts at the latest time, the maximum timestamp, not at the last record's.
const RUN_START_MS: u64 = AT_MS;
const LATEST_MS: u64 = AT_MS + 329_000;

// `monitor.probe_requested` at `at_unix_ms`, as `draft` writes it at the fixture's time.
fn draft_at(ids: &IdentifierIssuer, at_unix_ms: u64) -> SanitizedEventDraft {
    stamped(
        ids,
        at_unix_ms,
        MonitorPayloadDraft::requested(AuditInput::new()).into(),
    )
}

// A run's diagnostic `artifact.created`, stamped with the run's start.
fn created_at(ids: &IdentifierIssuer, at_unix_ms: u64) -> SanitizedEventDraft {
    stamped(
        ids,
        at_unix_ms,
        actingcommand_contract::ArtifactPayloadDraft::created(AuditInput::new()).into(),
    )
}

fn stamped(
    ids: &IdentifierIssuer,
    at_unix_ms: u64,
    payload: actingcommand_contract::EventPayloadDraft,
) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        at_unix_ms,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        payload,
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"ledger-ao1-fixture").expect("salt"))
    .expect("a stamped draft")
}

// The gap records among `records`.
fn gap_payloads(records: &[PersistedEvent]) -> Vec<actingcommand_contract::LedgerGapPayload> {
    records
        .iter()
        .filter_map(|record| match record.payload() {
            actingcommand_contract::EventPayload::Ledger(
                actingcommand_contract::LedgerPayload::GapRecorded(gap),
            ) => Some(gap.clone()),
            _ => None,
        })
        .collect()
}

// A24, the tail gap: an epoch whose last committed record (the run's diagnostic record, stamped
// with its start) is earlier than the record before it ends without its close and without a dump;
// the next start's `tail_unconfirmed` gap starts at the later record's time. The dump gap: an
// exhausted write dumps the run's end record, then its diagnostic record and one more; the dump's
// last record line is cut; the import's `dump_incomplete` gap starts at the run's end, the latest
// verified capture time, not at the last verified record's.
#[test]
fn a24_write_failure_gap_starts_at_the_latest_time() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let root = Root::new();
    let (reports_one, stops) = reports();
    let ledger = root
        .open(root.plan(1, &reports_one))
        .expect("the writer opens");
    ledger
        .append(draft_at(&ids, LATEST_MS))
        .expect("the run's end record");
    ledger
        .append(created_at(&ids, RUN_START_MS))
        .expect("the run's diagnostic record, stamped with its start");
    let faults = root.faults();
    faults.refuse_dump_at(&root.dump_path());
    faults.refuse_dump_at(&root.fallback_path());
    faults.fail_writes(1);
    ledger
        .append(draft_at(&ids, LATEST_MS))
        .expect_err("the only attempt fails");
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerDumpFailed), "no dump is written");
    assert!(ledger.close().is_err(), "the epoch ends without its close");
    drop(faults);
    let ledger = root
        .open(root.plan(1, &reports().0))
        .expect("the next start");
    let tails = gap_payloads(&after(&ledger, 0))
        .into_iter()
        .filter(|gap| gap.gap_kind() == actingcommand_contract::LedgerGapKind::TailUnconfirmed)
        .collect::<Vec<_>>();
    assert_eq!(tails.len(), 1, "one tail gap");
    assert_eq!(
        tails[0].gap_start_unix_ms(),
        LATEST_MS,
        "the tail gap starts at the latest time"
    );
    ledger.close().expect("close");

    let root = Root::new();
    let (reports_two, stops) = reports();
    let ledger = root
        .open(root.plan(2, &reports_two))
        .expect("the writer opens");
    let head = ledger.latest_sequence().expect("head");
    let faults = root.faults();
    let stall = WriterStallGate::arm(root.directory.path()).expect("writer stall gate");
    ledger
        .append_deferred(draft_at(&ids, LATEST_MS))
        .expect("the run's end record is accepted");
    assert!(
        stall.wait_held_commit(GATE_WAIT),
        "the first write is held in its attempt"
    );
    for draft in [created_at(&ids, RUN_START_MS), draft_at(&ids, RUN_START_MS)] {
        ledger.append_deferred(draft).expect("queued behind it");
    }
    faults.fail_writes(2);
    stall.release();
    let stop = stops.recv_timeout(GATE_WAIT).expect("the writer stops");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("the replies");
    assert!(summary.committed.is_empty());
    assert!(ledger.close().is_err(), "the writer ended with its failure");
    drop(faults);
    let text = fs::read_to_string(root.dump_path()).expect("the dump");
    let lines = text.lines().collect::<Vec<_>>();
    // Header, the three records, the fatal record, trailer.
    assert_eq!(lines.len(), 6);
    // No trailer, and the third record line cut in half: the run's end record and its diagnostic
    // record verify.
    let cut = &lines[3][..lines[3].len() / 2];
    fs::write(
        root.dump_path(),
        format!("{}\n{}\n{}\n{}", lines[0], lines[1], lines[2], cut),
    )
    .expect("cut the dump");
    let ledger = root
        .open(root.plan(5, &reports().0))
        .expect("the next start imports the verified part");
    let gaps = gap_payloads(&after(&ledger, head));
    assert_eq!(gaps.len(), 1, "one dump gap");
    assert_eq!(
        gaps[0].gap_kind(),
        actingcommand_contract::LedgerGapKind::DumpIncomplete
    );
    assert_eq!(
        gaps[0].gap_start_unix_ms(),
        LATEST_MS,
        "the dump gap starts at the latest verified capture time"
    );
    ledger.close().expect("close");
}
