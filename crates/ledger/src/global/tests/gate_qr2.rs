// SPDX-License-Identifier: AGPL-3.0-only

// #422 QR-2 (v0.12.23; brief rd-1028/brief-qr2-overview-pages.md, investigation
// rd-1028/qr2-investigation.md §2.1 and §4 R1 to R3). On production v0.12.21 every page of the
// overview's run block and of `ac_diagnose` derived its `run_recovery`, which reads up to 1 025
// records of every run the page names from sequence 0, and read the Lab relations (every Lab
// anchor of the history) and a first page's window start again, though no reader of the run
// listing or of a run's status reads the groups. The required states, the ledger's part:
// - T1: a page asked without run recovery reads no run's records for it. On a history whose runs
//   hold more records than a recovery context, it runs no statement of a run's context and has no
//   group, and it has the default page's rows (their views among them), cursor and read scope;
//   the writer's page and the in-memory page asked so have no group either. The default page
//   still reads every run's context and derives its groups, as before.
// - T2: a reader-pool thread reads the Lab relations and a window's first record once per
//   snapshot: a later page at the same snapshot reads neither, a page after the snapshot moved
//   reads both again, and each page is, byte for byte, the writer's own page at its snapshot (its
//   rows' views among it).
//
// The roots are formal roots with the synthetic ledger (LN-1), read by the Runtime's writer and its
// reader pool of one reader, without a read budget: no read stops at a clock, and nothing sleeps.

use super::*;
use crate::global::projection::{EventIndexes, PageSelection};
use crate::global::{
    GlobalLedger, LedgerFailurePlan, LedgerMaintenance, LedgerOpenTiming, LedgerWritePolicy,
    LedgerWriterCommandKind, MAX_QUERY_PAGE_EVENTS, Sha256SecretFingerprinter,
};
use crate::{
    LedgerWriterCommandStatements, PersistedEvent, SyntheticInstance, SyntheticLedgerSpec,
    WriterStatementRecorder, write_synthetic_ledger,
};
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventQuery, EventSeverity,
    EventSource, EventType, FactValue, IdentifierIssuer, InstanceId, LedgerMaterialReadState,
    LedgerReadScope, LedgerReadSource, LedgerRecoveryGap, OriginModule, OwnerEpoch,
    ProjectionProfile, RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimeFactRecord,
    RuntimeFactScope, RuntimePayloadDraft, SanitizedEventDraft,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes) and the writer's owner id: neutral
// fixture words.
const STATE_KEY: &[u8] = b"ledger-qr2-fixture-key";
const OWNER: &str = "ledger-qr2-fixture";
// The first record's time (2030-01-01) and the time between two records.
const FIRST_MS: u64 = 1_893_456_000_000;
const RECORD_INTERVAL_MS: u64 = 3_000;
// The instances of the history's runs, in turn.
const INSTANCE_TURNS: &str = "ab";
// The writer's write policy and the open's deadline, as QR-1's.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
const OPEN_TIMEOUT_SECONDS: u64 = 600;
// Every caller waits this long for one reply; no read of these tests comes near it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);
const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
const PAGE_LIMIT: u16 = 64;
// T1: the frames of each run. A frame writes seven records at least, so each run holds more
// records than a page's recovery context (`MAX_QUERY_PAGE_EVENTS`); the history holds about four
// such runs.
const LONG_RUN_FRAMES: u64 = 160;
const T1_RECORDS: u64 = 6_000;
// T2: a history of the default mix's runs.
const T2_RECORDS: u64 = 2_000;
// A history statement that selects by a run's link: a run's recovery context (`run_rows`).
const RUN_CONTEXT_CONDITION: &str = "l.run_id=?";
// The Lab relations' statements (`read_lab_relations`): its two anchor statements and its run
// statement.
const LAB_ANCHOR_STATEMENT: &str =
    "FROM ledger_events a JOIN ledger_links al ON al.sequence=a.sequence";
const LAB_RUN_STATEMENT: &str = "SELECT r.run_id FROM ledger_links r";
const LAB_RELATION_STATEMENTS: usize = 3;
// The statement of a window's first record (`time_start`).
const WINDOW_START_STATEMENT: &str =
    "SELECT MIN(sequence) FROM ledger_events INDEXED BY ledger_view_index_time_v1";

fn instance(letter: char) -> InstanceId {
    serde_json::from_str(&format!("\"instance_{letter:0>32}\"")).expect("a fixture instance id")
}

fn instances() -> Vec<SyntheticInstance> {
    INSTANCE_TURNS
        .chars()
        .map(|letter| SyntheticInstance {
            alias: format!("fixture-instance-{letter}"),
            instance_id: instance(letter),
        })
        .collect()
}

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

/// A formal root, closed, holding `records` synthetic records, each run of `frames` frames (the
/// default mix's when `None`).
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
}

impl Root {
    fn new(records: u64, frames: Option<u64>) -> Self {
        let dir = tempfile::tempdir().expect("state root");
        let database = Arc::new(
            RuntimeStateStore::open_database(dir.path(), STATE_KEY).expect("State database"),
        );
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
        let mut spec = SyntheticLedgerSpec::new(records, instances());
        spec.first_timestamp_unix_ms = FIRST_MS;
        spec.record_interval_ms = RECORD_INTERVAL_MS;
        if let Some(frames) = frames {
            spec.mix.frames_per_run_min = frames;
            spec.mix.frames_per_run_max = frames;
        }
        let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
        assert_eq!(generated.records, records, "{generated:?}");
        assert!(generated.runs >= 3, "{generated:?}");
        Self { dir, database }
    }

    /// The Runtime's writer of the root (a failure plan: it has a reader pool), with one reader,
    /// so that every page is read by the same reader, and no read budget.
    fn open(&self) -> GlobalLedger {
        let state_root = self.dir.path().canonicalize().expect("state root");
        let plan = LedgerFailurePlan::new(
            LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
                .expect("write policy"),
            &state_root,
            None,
            epoch(),
            FIRST_MS,
        )
        .with_reply_timeout(REPLY_TIMEOUT)
        .with_reader_threads_for_test(1);
        let deadline = MaintenanceLimits {
            timeout_seconds: OPEN_TIMEOUT_SECONDS,
            ..MaintenanceLimits::default()
        }
        .deadline()
        .expect("deadline");
        let ledger = LedgerMaintenance::acquire(
            self.dir.path(),
            false,
            MaintenanceLimits::default(),
            deadline,
        )
        .expect("maintenance owner")
        .open_writer_timed(
            Arc::clone(&self.database),
            OWNER.into(),
            Some(plan),
            &mut LedgerOpenTiming::default(),
        )
        .expect("the writer opens");
        assert!(
            ledger.reads_off_writer(),
            "the Runtime's writer reads its pages on its reader pool: {:?}",
            ledger.reader_pool_finding()
        );
        ledger
    }
}

fn first_page(limit: u16) -> RuntimeEventQueryPageRequest {
    RuntimeEventQueryPageRequest::new(limit, None).expect("a first page")
}

fn at_snapshot(
    request: RuntimeEventQueryPageRequest,
    snapshot: u64,
) -> RuntimeEventQueryPageRequest {
    request
        .at_snapshot(snapshot)
        .expect("a page at the snapshot")
}

/// A page's bytes, as a client receives them.
fn bytes(page: &RuntimeEventQueryPage) -> Vec<u8> {
    serde_json::to_vec(page).expect("a page serializes")
}

/// The one view page the reader pool read since `recorder` was last read (a pool page is
/// recorded before it is answered).
fn pool_page_read(recorder: &WriterStatementRecorder) -> LedgerWriterCommandStatements {
    let mut pages = recorder
        .take()
        .into_iter()
        .filter(|command| command.command == LedgerWriterCommandKind::ProjectViewPage)
        .collect::<Vec<_>>();
    assert_eq!(pages.len(), 1, "one view page was read");
    pages.remove(0)
}

/// How many of `read`'s statements `selects` selects by their SQL text.
fn statements(read: &LedgerWriterCommandStatements, selects: impl Fn(&str) -> bool) -> usize {
    read.statements
        .iter()
        .filter(|statement| selects(&statement.sql))
        .count()
}

fn run_context(sql: &str) -> bool {
    sql.contains(RUN_CONTEXT_CONDITION)
}

fn lab_relations(sql: &str) -> bool {
    sql.contains(LAB_ANCHOR_STATEMENT) || sql.starts_with(LAB_RUN_STATEMENT)
}

fn window_start(sql: &str) -> bool {
    sql.starts_with(WINDOW_START_STATEMENT)
}

/// One reader-pool page of `query`, with the statements of the Lab relations and of a window
/// start it ran.
fn counted_page(
    ledger: &GlobalLedger,
    recorder: &WriterStatementRecorder,
    query: &EventQuery,
    request: RuntimeEventQueryPageRequest,
) -> (RuntimeEventQueryPage, usize, usize) {
    let page = ledger
        .project_view_page(query.clone(), PROFILE, request)
        .expect("a pool page");
    let read = pool_page_read(recorder);
    assert!(
        page.events().iter().any(|row| row.links.run_id().is_some()),
        "the page has a row that names a run"
    );
    (
        page,
        statements(&read, lab_relations),
        statements(&read, window_start),
    )
}

/// A synchronous append after the history: a runtime fact the writer records.
fn runtime_fact(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        FIRST_MS + RECORD_INTERVAL_MS * (T2_RECORDS + 1),
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        RuntimePayloadDraft::fact_recorded(
            RuntimeFactRecord {
                scope: RuntimeFactScope::Runtime,
                key: "backend.fixture_key_qr2".to_owned(),
                value: FactValue::Integer(1),
                observed_at_unix_ms: FIRST_MS,
                source: OriginModule::Runtime,
                ttl_ms: None,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"qr2-fixture-salt").expect("salt"))
    .expect("a fixture draft")
}

#[test]
fn qr2_t1_a_page_without_run_recovery_reads_no_runs_records_and_keeps_its_rows() {
    let root = Root::new(T1_RECORDS, Some(LONG_RUN_FRAMES));
    let ledger = root.open();
    // Every run's main admission: each row names its run.
    let query = EventQuery {
        event_type: Some(EventType::TaskRequested),
        ..EventQuery::default()
    };
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("the recorder");

    // The default page, as before: it reads the context of every run it names, and each run
    // that holds more records than a context (`MAX_QUERY_PAGE_EVENTS`) has a group, limited.
    let whole = ledger
        .project_view_page(query.clone(), PROFILE, first_page(PAGE_LIMIT))
        .expect("the default page");
    let default_read = pool_page_read(&recorder);
    let snapshot = whole.snapshot_ledger_position();
    assert!(
        whole.events().len() >= 3 && !whole.has_more(),
        "every run's admission on one page: {} rows",
        whole.events().len()
    );
    let groups = whole.run_recovery();
    assert!(
        groups.len() >= 2
            && groups
                .iter()
                .all(|group| group.gaps.contains(&LedgerRecoveryGap::ContextLimit)),
        "a group per run of more than {MAX_QUERY_PAGE_EVENTS} records, each limited: {groups:?}"
    );
    let contexts = statements(&default_read, run_context);
    assert!(
        contexts >= groups.len(),
        "the default page read the runs' contexts: {contexts} statements"
    );

    // The same page without run recovery: no statement of a run's context, no group, and
    // everything else the default page's.
    let request = at_snapshot(first_page(PAGE_LIMIT).without_run_recovery(), snapshot);
    let page = ledger
        .project_view_page(query.clone(), PROFILE, request.clone())
        .expect("the page without run recovery");
    let read = pool_page_read(&recorder);
    assert_eq!(
        statements(&read, run_context),
        0,
        "the page without run recovery read a run's context"
    );
    assert!(page.run_recovery().is_empty(), "{:?}", page.run_recovery());
    assert_eq!(page.snapshot_ledger_position(), snapshot);
    assert_eq!(page.events(), whole.events(), "the rows and their views");
    assert_eq!(page.has_more(), whole.has_more());
    assert_eq!(page.next_cursor(), whole.next_cursor());
    assert_eq!(page.read_scope(), whole.read_scope());
    drop(recorder);

    // The writer's page asked so is the pool's, byte for byte; the in-memory page asked so has
    // no group either and the same rows, where the in-memory default page has groups.
    let writer = ledger
        .project_view_page_on_writer_for_test(query.clone(), PROFILE, request.clone())
        .expect("the writer's page");
    assert_eq!(
        bytes(&writer),
        bytes(&page),
        "the writer's page is the pool's"
    );
    let events = ledger.query(EventQuery::default()).expect("every record");
    let head = events.last().map_or(0, PersistedEvent::sequence);
    assert_eq!(head, snapshot, "no record since the pages");
    let indexes = EventIndexes::from_events(&events);
    let scope = LedgerReadScope {
        source: LedgerReadSource::Runtime,
        material_read: LedgerMaterialReadState::NotRequested,
        scanned_through_position: head,
        read_complete: true,
        limits: Vec::new(),
    };
    let in_memory = |request: &RuntimeEventQueryPageRequest| {
        indexes
            .project_view_page(
                &events,
                &query,
                PROFILE,
                request,
                scope.clone(),
                PageSelection::from(head),
            )
            .expect("the in-memory page")
    };
    let ids = |page: &RuntimeEventQueryPage| {
        page.events()
            .iter()
            .map(|row| row.event_id)
            .collect::<Vec<_>>()
    };
    let without = in_memory(&request);
    assert!(
        without.run_recovery().is_empty(),
        "{:?}",
        without.run_recovery()
    );
    assert_eq!(ids(&without), ids(&page), "the in-memory page's rows");
    assert!(
        !in_memory(&at_snapshot(first_page(PAGE_LIMIT), head))
            .run_recovery()
            .is_empty(),
        "the in-memory default page derives its groups"
    );
    ledger.close().expect("close");
}

#[test]
fn qr2_t2_a_reader_reads_the_lab_relations_and_a_window_start_once_per_snapshot() {
    let root = Root::new(T2_RECORDS, None);
    let ledger = root.open();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    // Two listing queries over one window (from a quarter of the history on), as the run listing
    // lists one fact type per query at one snapshot.
    let since = FIRST_MS + RECORD_INTERVAL_MS * (T2_RECORDS / 4);
    let window = |event_type: EventType| EventQuery {
        event_type: Some(event_type),
        from_timestamp_unix_ms: Some(since),
        ..EventQuery::default()
    };
    let admissions = window(EventType::TaskRequested);
    let starts = window(EventType::TaskStarted);
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("the recorder");

    // The reader's first page at the head reads the Lab relations and the window's start.
    let (first, lab, start) = counted_page(&ledger, &recorder, &admissions, first_page(PAGE_LIMIT));
    let snapshot = first.snapshot_ledger_position();
    assert_eq!(
        (lab, start),
        (LAB_RELATION_STATEMENTS, 1),
        "the first page at snapshot {snapshot}: Lab relation and window start statements"
    );
    // Later pages at that snapshot, of another type and of the same one, read neither.
    let (second, lab, start) = counted_page(
        &ledger,
        &recorder,
        &starts,
        at_snapshot(first_page(PAGE_LIMIT), snapshot),
    );
    assert_eq!((lab, start), (0, 0), "a later page at the same snapshot");
    let (again, lab, start) = counted_page(
        &ledger,
        &recorder,
        &admissions,
        at_snapshot(first_page(PAGE_LIMIT), snapshot),
    );
    assert_eq!((lab, start), (0, 0), "the first page again at its snapshot");
    assert_eq!(bytes(&again), bytes(&first), "the same page");

    // The snapshot moves: the reader's next page reads both again, and a later page at the new
    // snapshot neither.
    let appended = ledger.append(runtime_fact(&ids)).expect("an append");
    assert_eq!(appended.sequence(), snapshot + 1);
    let (moved, lab, start) = counted_page(&ledger, &recorder, &admissions, first_page(PAGE_LIMIT));
    assert_eq!(moved.snapshot_ledger_position(), snapshot + 1);
    assert_eq!(
        (lab, start),
        (LAB_RELATION_STATEMENTS, 1),
        "the first page after the snapshot moved"
    );
    let (moved_starts, lab, start) = counted_page(
        &ledger,
        &recorder,
        &starts,
        at_snapshot(first_page(PAGE_LIMIT), snapshot + 1),
    );
    assert_eq!((lab, start), (0, 0), "a later page at the new snapshot");
    drop(recorder);

    // Each pool page is the writer's own page at its snapshot, byte for byte: the kept relations
    // and window start changed none of its rows, their views or its cursor.
    for (read, query, page, at) in [
        ("first", &admissions, &first, snapshot),
        ("second", &starts, &second, snapshot),
        ("moved", &admissions, &moved, snapshot + 1),
        ("moved starts", &starts, &moved_starts, snapshot + 1),
    ] {
        let writer = ledger
            .project_view_page_on_writer_for_test(
                query.clone(),
                PROFILE,
                at_snapshot(first_page(PAGE_LIMIT), at),
            )
            .unwrap_or_else(|error| panic!("{read}: the writer's page: {error:?}"));
        assert_eq!(
            bytes(page),
            bytes(&writer),
            "{read}: the pool's page is the writer's"
        );
    }
    ledger.close().expect("close");
}
