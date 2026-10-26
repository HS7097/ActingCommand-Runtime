// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-WS (brief rd-1026/brief-ln-ws.md; root cause rd-1026/writer-stall/root-cause.md §5; the
// production stop of 2026-10-26 19:12:43): no read can make a write fail. An `ac_events` read with
// a time window and no instance ran on the ledger's single writer thread as one SQLite step that
// scanned the history from sequence 1 (about 1.58 million rows, 15 to 18 s). The read budget was
// looked at only between the rows a statement returned, so nothing stopped it; a policy append
// waited out its reply limit behind it, and the Runtime stopped.
//
// The ledger's part. The roots hold the synthetic ledger (LN-1) in production's shape: capture
// frames stored through the root's artifact store (LN-8d), four instances in turn with one of
// them in seven runs of twenty (about 35 %), three seconds between records, so the history spans
// hours, and one record halfway through the history whose time lies inside the reads' window
// (an inversion). The window lies at 90 to 95 % of the history and holds fewer `changes` records
// than a page asks for, so a scan runs on to the head.
// - T1 growth: for each read form of the incident (no instance with the views `changes`,
//   `events`, none and `errors`; one instance; the run listing; one run), the costliest history
//   statement's SQLite work on 12 000 records is at most 1.5 times that on 3 000;
// - T2 in-step bound: a no-instance window read whose budget runs out inside a statement answers
//   an incomplete page whose cursor is past its start, and at most one look at the clock runs
//   after the budget point;
// - T3 append behind a read: an append captured while such a read runs commits once the read
//   answers at its budget, and its reply is `Ok`, its observation naming the read;
// - T4 paging: paging the no-instance and the instance window reads with a small budget from the
//   first page to the end returns the rows of an unbounded read in its order, each cursor past
//   the one before;
// - T5 time start: a first page with a window starts its first history statement no lower than
//   the first record of the window's start minus one, and returns the rows of an unbounded read,
//   the inverted record among them.
//
// Every bounded read runs on a fake writer clock that moves a fixed step each time it is read,
// and every caller waits up to 600 s for a writer reply, so no real wait can matter; nothing
// sleeps. The statements are recorded on the writer's thread (`WriterStatementRecorder`).

use super::*;
use crate::global::projection::{EventIndexes, PageSelection};
use crate::global::{
    GlobalLedger, LedgerAppendObservationState, LedgerFailurePlan, LedgerMaintenance,
    LedgerMonotonicClock, LedgerOpenTiming, LedgerWritePolicy, LedgerWriterCommandKind,
    Sha256SecretFingerprinter,
};
use crate::{
    LedgerWriterCommandStatements, PersistedEvent, SyntheticInstance, SyntheticInversion,
    SyntheticLedgerSpec, WriterReadHoldGate, WriterStatementRecorder, write_synthetic_ledger,
};
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventId, EventLinksDraft, EventOrigin, EventQuery,
    EventSeverity, EventSource, EventType, FactValue, IdentifierIssuer, InstanceId,
    LedgerMaterialReadState, LedgerPageLimit, LedgerReadScope, LedgerReadSource, LedgerView,
    OriginModule, OwnerEpoch, ProjectionProfile, RunId, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft,
    SanitizedEventDraft,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes) and the writer's owner id: neutral
// fixture words.
const STATE_KEY: &[u8] = b"ledger-ln-ws-fixture";
const OWNER: &str = "ledger-ln-ws-fixture";
// Two histories of the same shape, one four times the other.
const SMALL_RECORDS: u64 = 3_000;
const LARGE_RECORDS: u64 = 12_000;
// The frame store's shape (LN-8d, the scale harness's): one frame in sixteen is stored and live.
const LIVE_FRAME_EVERY: u64 = 16;
// The first record's time (2030-01-01, after any record the root's own writer opens could write)
// and the time between two records: 3 000 records span 2.5 hours, 12 000 records 10 hours.
const FIRST_MS: u64 = 1_893_456_000_000;
const RECORD_INTERVAL_MS: u64 = 3_000;
// The instances in turn, one letter per run: `a` runs seven runs of every twenty.
const INSTANCE_TURNS: &str = "abcadbacdabcadbacdab";
// The write policy of the writer (as the LN-8d gate's) and the open's deadline.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
const OPEN_TIMEOUT_SECONDS: u64 = 600;
// Every caller waits this long for one writer reply: no real wait of these tests reaches it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);
// `ac_events` reads every event page in its forensic profile.
const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
// The 19:12:24 call's limit.
const PAGE_LIMIT: u16 = 100;
// The SQL a history statement starts with (the event row's columns through its alias), and the
// time-index start's (R3).
const HISTORY_STATEMENT: &str = "SELECT e.sequence,";
const TIME_START_STATEMENT: &str = "SELECT MIN(sequence)";
// T1: a history statement's span, small enough that both histories hold whole spans inside the
// window (150 records of the smaller one).
const T1_SCAN_ROWS: usize = 64;
// T2 and T3: the clock is looked at every 100 VM steps, and a history statement spans at most
// 1 024 sequences. T2's budget runs out after 30 looks at a clock that moves 1 ms a look, T3's
// after 100 looks at one that moves 50 ms a look.
const BOUND_CHECK_VM_STEPS: u32 = 100;
const BOUND_SCAN_ROWS: usize = 1_024;
const T2_BUDGET: Duration = Duration::from_millis(30);
const T3_BUDGET: Duration = Duration::from_millis(5_000);
const T3_STEP_MS: u64 = 50;
// T4: a budget of 20 looks at a clock that moves 1 ms a look, a statement spanning at most 256
// sequences, and pages of 10 and 30 records; a broken run ends after this many pages.
const T4_BUDGET: Duration = Duration::from_millis(20);
const T4_SCAN_ROWS: usize = 256;
const T4_MAX_PAGES: usize = 10_000;
// T3: how many times the test looks at the capture queue before it calls the append lost.
const CAPTURE_LOOKS: u64 = 100_000_000;

/// A writer clock in milliseconds that moves `step_ms` each time it is read.
fn stepping_clock(step_ms: u64) -> LedgerMonotonicClock {
    let now = Arc::new(AtomicU64::new(0));
    Arc::new(move || now.fetch_add(step_ms, Ordering::SeqCst))
}

fn instance(letter: char) -> InstanceId {
    serde_json::from_str(&format!("\"instance_{letter:0>32}\"")).expect("a fixture instance id")
}

/// The instances of the history's runs, in turn.
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

/// The reads' window on a history of `records` records: from 90 % of it to 95 %.
fn window(records: u64) -> (u64, u64) {
    let at = |share: u64| FIRST_MS + RECORD_INTERVAL_MS * (records * share / 100);
    (at(90), at(95))
}

/// How a test's writer reads: its budget on its clock (none: a read never stops), how often the
/// in-statement bound looks at the clock and how many sequences one history statement spans.
struct Reads {
    budget: Option<(Duration, LedgerMonotonicClock)>,
    check_vm_steps: u32,
    scan_rows: usize,
}

impl Reads {
    fn unbounded(scan_rows: usize) -> Self {
        Self {
            budget: None,
            check_vm_steps: BOUND_CHECK_VM_STEPS,
            scan_rows,
        }
    }

    fn bounded(budget: Duration, step_ms: u64, scan_rows: usize) -> Self {
        Self {
            budget: Some((budget, stepping_clock(step_ms))),
            check_vm_steps: BOUND_CHECK_VM_STEPS,
            scan_rows,
        }
    }
}

/// A closed formal root holding `records` synthetic records in production's shape, the first
/// of them at `first_sequence`.
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
    records: u64,
    first_sequence: u64,
}

impl Root {
    fn new(records: u64) -> Self {
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
        let (since, _) = window(records);
        let mut spec = SyntheticLedgerSpec::new(records, instances());
        spec.first_timestamp_unix_ms = FIRST_MS;
        spec.record_interval_ms = RECORD_INTERVAL_MS;
        spec.mix.frame_artifact_every_frames = LIVE_FRAME_EVERY;
        spec.inversion = Some(SyntheticInversion {
            record: records / 2,
            timestamp_unix_ms: since + RECORD_INTERVAL_MS / 2,
        });
        let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
        assert_eq!(generated.records, records, "{generated:?}");
        // Runs on three instances at least, and live frames.
        assert!(
            generated.runs >= 4 && generated.stored_frames > 0,
            "{generated:?}"
        );
        Self {
            dir,
            database,
            records,
            first_sequence: generated.first_sequence,
        }
    }

    /// The instance of the records at 92 % of the history: the instance whose run the window's
    /// records belong to, as one instance's were at 19:11:30.
    fn window_instance(&self, ledger: &GlobalLedger) -> InstanceId {
        let at = self.first_sequence + self.records * 92 / 100;
        ledger
            .query(EventQuery {
                from_sequence: Some(at),
                to_sequence: Some(at + 64),
                ..EventQuery::default()
            })
            .expect("the records at 92 %")
            .iter()
            .find_map(|event| event.links().instance_id().copied())
            .expect("a record of an instance in the window")
    }

    /// The first record of the window by the history's own times (at 90 % of it).
    fn window_start(&self) -> u64 {
        self.first_sequence + self.records * 90 / 100
    }

    /// The Runtime's writer of the root, reading as `reads` says.
    fn open(&self, reads: Reads) -> GlobalLedger {
        let state_root = self.dir.path().canonicalize().expect("state root");
        let mut plan = LedgerFailurePlan::new(
            LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
                .expect("write policy"),
            &state_root,
            None,
            epoch(),
            FIRST_MS,
        )
        .with_reply_timeout(REPLY_TIMEOUT)
        .with_read_check_vm_steps(reads.check_vm_steps)
        .with_history_scan_rows(reads.scan_rows);
        if let Some((budget, clock)) = reads.budget {
            plan = plan.with_read_budget(budget).with_clock(clock);
        }
        let deadline = MaintenanceLimits {
            timeout_seconds: OPEN_TIMEOUT_SECONDS,
            ..MaintenanceLimits::default()
        }
        .deadline()
        .expect("deadline");
        LedgerMaintenance::acquire(
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
        .expect("the writer opens")
    }

    /// Form A (root-cause §2.4): no instance, the window, `view`.
    fn form_a(&self, view: Option<LedgerView>) -> EventQuery {
        let (since, until) = window(self.records);
        EventQuery {
            view,
            from_timestamp_unix_ms: Some(since),
            to_timestamp_unix_ms: Some(until),
            ..EventQuery::default()
        }
    }

    /// Form B: one instance in the window.
    fn form_b(&self, instance: InstanceId) -> EventQuery {
        EventQuery {
            instance_id: Some(instance),
            ..self.form_a(None)
        }
    }

    /// Form C: the run listing's read of one event type of one instance since the window's
    /// start.
    fn form_c(&self, instance: InstanceId) -> EventQuery {
        EventQuery {
            event_type: Some(EventType::TaskRequested),
            instance_id: Some(instance),
            from_timestamp_unix_ms: Some(window(self.records).0),
            ..EventQuery::default()
        }
    }
}

/// Form D: one run's records, as run status and evidence read them.
fn form_d(run: RunId) -> EventQuery {
    EventQuery {
        run_id: Some(run),
        ..EventQuery::default()
    }
}

/// The run of `instance`'s last requested task.
fn last_run(ledger: &GlobalLedger, instance: InstanceId) -> RunId {
    let requested = ledger
        .query(EventQuery {
            event_type: Some(EventType::TaskRequested),
            instance_id: Some(instance),
            ..EventQuery::default()
        })
        .expect("the instance's requested tasks");
    *requested
        .last()
        .and_then(|event| event.links().run_id())
        .expect("a requested task with its run")
}

fn first_page(limit: u16) -> RuntimeEventQueryPageRequest {
    RuntimeEventQueryPageRequest::new(limit, None).expect("a first page")
}

/// The one view page among the read commands `recorder` recorded since it was last read.
fn page_read(recorder: &WriterStatementRecorder) -> LedgerWriterCommandStatements {
    let mut pages = recorder
        .take()
        .into_iter()
        .filter(|command| command.command == LedgerWriterCommandKind::ProjectViewPage)
        .collect::<Vec<_>>();
    assert_eq!(pages.len(), 1, "one view page was read: {pages:?}");
    pages.remove(0)
}

/// Every page of `query` from the first to the end, `limit` records a page: the records' ids
/// and the cursors the pages continue after.
fn every_page(ledger: &GlobalLedger, query: &EventQuery, limit: u16) -> (Vec<EventId>, Vec<u64>) {
    let mut ids = Vec::new();
    let mut cursors = Vec::new();
    let mut cursor = None;
    for _ in 0..T4_MAX_PAGES {
        let page = ledger
            .project_view_page(
                query.clone(),
                PROFILE,
                RuntimeEventQueryPageRequest::new(limit, cursor.take()).expect("a page request"),
            )
            .expect("the page");
        ids.extend(page.events().iter().map(|event| event.event_id));
        if !page.has_more() {
            return (ids, cursors);
        }
        let next = page.next_cursor().expect("a page that continues").clone();
        cursors.push(next.after_sequence());
        cursor = Some(next);
    }
    panic!("{query:?} did not end within {T4_MAX_PAGES} pages");
}

/// The same pages read in memory from every record (`EventIndexes`): an unbounded read.
fn unbounded_ids(
    events: &[PersistedEvent],
    indexes: &EventIndexes,
    query: &EventQuery,
    limit: u16,
) -> Vec<EventId> {
    let head = events.last().map_or(0, PersistedEvent::sequence);
    let mut ids = Vec::new();
    let mut cursor = None;
    for _ in 0..T4_MAX_PAGES {
        let request =
            RuntimeEventQueryPageRequest::new(limit, cursor.take()).expect("a page request");
        let page: RuntimeEventQueryPage = indexes
            .project_view_page(
                events,
                query,
                PROFILE,
                &request,
                LedgerReadScope {
                    source: LedgerReadSource::Runtime,
                    material_read: LedgerMaterialReadState::NotRequested,
                    scanned_through_position: head,
                    read_complete: true,
                    limits: Vec::new(),
                },
                PageSelection::from(head),
            )
            .expect("the in-memory page");
        ids.extend(page.events().iter().map(|event| event.event_id));
        if !page.has_more() {
            return ids;
        }
        cursor = Some(page.next_cursor().expect("a page that continues").clone());
    }
    panic!("the in-memory read of {query:?} did not end within {T4_MAX_PAGES} pages");
}

/// A synchronous append as the policy driver makes one: a runtime fact the writer records.
fn runtime_fact(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        FIRST_MS + RECORD_INTERVAL_MS * LARGE_RECORDS,
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
                key: "backend.fixture_key_ln_ws".to_owned(),
                value: FactValue::Integer(1),
                observed_at_unix_ms: FIRST_MS,
                source: OriginModule::Runtime,
                ttl_ms: None,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"ln-ws-fixture-salt").expect("salt"))
    .expect("a fixture draft")
}

/// The costliest history statement of the first page of each incident form on `root`, with the
/// time-index start's work when the page ran one.
fn costliest_statements(root: &Root) -> Vec<(&'static str, u64, String, Option<u64>)> {
    let ledger = root.open(Reads::unbounded(T1_SCAN_ROWS));
    let instance = root.window_instance(&ledger);
    let run = last_run(&ledger, instance);
    let forms = [
        (
            "A changes",
            root.form_a(Some(LedgerView::Changes)),
            PAGE_LIMIT,
        ),
        (
            "A events",
            root.form_a(Some(LedgerView::Events)),
            PAGE_LIMIT,
        ),
        ("A none", root.form_a(None), PAGE_LIMIT),
        (
            "A errors",
            root.form_a(Some(LedgerView::Errors)),
            PAGE_LIMIT,
        ),
        // `summary` and `brief` are the client's renderings of the same read (30 and 60 rows).
        ("B summary", root.form_b(instance), 30),
        ("B brief", root.form_b(instance), 60),
        ("C run listing", root.form_c(instance), 10),
        ("D run evidence", form_d(run), PAGE_LIMIT),
    ];
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("the recorder");
    let mut costliest = Vec::new();
    for (form, query, limit) in forms {
        ledger
            .project_view_page(query, PROFILE, first_page(limit))
            .expect("the first page");
        let read = page_read(&recorder);
        let history = read
            .statements
            .iter()
            .filter(|statement| statement.sql.starts_with(HISTORY_STATEMENT))
            .max_by_key(|statement| statement.vm_steps)
            .unwrap_or_else(|| panic!("{form}: no history statement: {read:?}"));
        let time_start = read
            .statements
            .iter()
            .find(|statement| statement.sql.starts_with(TIME_START_STATEMENT))
            .map(|statement| statement.vm_steps);
        costliest.push((form, history.vm_steps, history.sql.clone(), time_start));
    }
    drop(recorder);
    ledger.close().expect("close");
    costliest
}

#[test]
fn ln_ws_t1_no_history_statement_of_an_incident_read_grows_with_the_history() {
    let small = costliest_statements(&Root::new(SMALL_RECORDS));
    let large = costliest_statements(&Root::new(LARGE_RECORDS));
    let mut grown = Vec::new();
    for ((form, small_steps, _, small_start), (_, large_steps, large_sql, large_start)) in
        small.iter().zip(&large)
    {
        if 2 * large_steps > 3 * small_steps {
            grown.push(format!(
                "{form}: {large_steps} VM steps on {LARGE_RECORDS} records, {small_steps} on \
                 {SMALL_RECORDS} (time-index start {large_start:?} and {small_start:?}): \
                 {large_sql}"
            ));
        }
    }
    assert!(
        grown.is_empty(),
        "the costliest history statement grew more than 1.5 times:\n{}",
        grown.join("\n")
    );
}

#[test]
fn ln_ws_t2_a_window_read_stops_inside_a_statement_at_its_budget_past_its_start() {
    let root = Root::new(SMALL_RECORDS);
    let ledger = root.open(Reads::bounded(T2_BUDGET, 1, BOUND_SCAN_ROWS));
    let head = ledger.latest_sequence().expect("head");
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("the recorder");
    let page = ledger
        .project_view_page(
            root.form_a(Some(LedgerView::Changes)),
            PROFILE,
            first_page(PAGE_LIMIT),
        )
        .expect("the page");
    let read = page_read(&recorder);
    let scope = page.read_scope().expect("a Runtime page has its scope");
    assert!(
        !scope.read_complete && scope.limits.contains(&LedgerPageLimit::ReadBudget),
        "the budget stopped the read: {scope:?}"
    );
    assert!(page.has_more(), "{scope:?}");
    let cursor = page.next_cursor().expect("a page that continues");
    assert!(
        cursor.after_sequence() > 0 && cursor.after_sequence() < head,
        "the cursor {} is past the page's start and before the head {head}",
        cursor.after_sequence()
    );
    assert_eq!(cursor.after_sequence(), scope.scanned_through_position);
    assert!(
        read.bound_callbacks > 0,
        "the read looked at its clock inside its statements: {read:?}"
    );
    assert!(
        read.late_callbacks <= 1,
        "{} looks at the clock ran after the budget point: {read:?}",
        read.late_callbacks
    );
    drop(recorder);
    ledger.close().expect("close");
}

#[test]
fn ln_ws_t3_an_append_captured_behind_an_over_budget_read_commits_once_it_answers() {
    let root = Root::new(SMALL_RECORDS);
    let ledger = root.open(Reads::bounded(T3_BUDGET, T3_STEP_MS, BOUND_SCAN_ROWS));
    let head = ledger.latest_sequence().expect("head");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let query = root.form_a(Some(LedgerView::Changes));
    let hold = WriterReadHoldGate::arm(root.dir.path()).expect("read hold");
    let draft = runtime_fact(&ids);
    let writer = &ledger;
    let (page, (appended, observation)) = thread::scope(|scope| {
        let reader =
            scope.spawn(move || writer.project_view_page(query, PROFILE, first_page(PAGE_LIMIT)));
        assert!(
            hold.wait_held(BARRIER_WAIT),
            "the page read reached the writer"
        );
        let captured = writer.capture_status().captured;
        let appender = scope.spawn(move || writer.append_with_observation(draft));
        // The append is in the capture queue while the writer is in the read.
        let mut looks = 0_u64;
        while writer.capture_status().captured == captured {
            looks += 1;
            assert!(looks < CAPTURE_LOOKS, "the append was not captured");
            thread::yield_now();
        }
        hold.release();
        (
            reader.join().expect("reader"),
            appender.join().expect("appender"),
        )
    });
    let page = page.expect("the page");
    let scope = page.read_scope().expect("a Runtime page has its scope");
    assert!(
        !scope.read_complete && scope.limits.contains(&LedgerPageLimit::ReadBudget),
        "the read answered at its budget: {scope:?}"
    );
    let appended = appended.expect("the append's reply is Ok, never writer_response_timeout");
    assert_eq!(appended.sequence(), head + 1, "the append committed");
    let observation = observation.expect("the append's observation");
    let previous = observation.previous_writer_work;
    assert_eq!(
        previous.command,
        Some(LedgerWriterCommandKind::ProjectViewPage),
        "the writer's command before the append was the read: {observation:?}"
    );
    assert_eq!(
        previous.processing.state,
        LedgerAppendObservationState::Observed,
        "{observation:?}"
    );
    let processing = previous
        .processing
        .finished_at
        .zip(previous.processing.started_at)
        .map(|(finished, started)| finished.saturating_duration_since(started))
        .expect("the read's processing span");
    assert!(
        processing <= T3_BUDGET,
        "the read took {processing:?}, more than its budget {T3_BUDGET:?}"
    );
    ledger.close().expect("close");
}

#[test]
fn ln_ws_t4_paging_window_reads_on_a_small_budget_returns_the_unbounded_rows_in_order() {
    let root = Root::new(SMALL_RECORDS);
    // The unbounded reads, from every record of the history.
    let (forms, unbounded) = {
        let ledger = root.open(Reads::unbounded(T4_SCAN_ROWS));
        let forms = [
            ("A changes", root.form_a(Some(LedgerView::Changes)), 10),
            ("B", root.form_b(root.window_instance(&ledger)), 30),
        ];
        let events = ledger.query(EventQuery::default()).expect("every record");
        ledger.close().expect("close");
        let indexes = EventIndexes::from_events(&events);
        let unbounded = forms
            .iter()
            .map(|(_, query, limit)| unbounded_ids(&events, &indexes, query, *limit))
            .collect::<Vec<_>>();
        (forms, unbounded)
    };
    let ledger = root.open(Reads::bounded(T4_BUDGET, 1, T4_SCAN_ROWS));
    for ((form, query, limit), expected) in forms.iter().zip(&unbounded) {
        assert!(!expected.is_empty(), "{form}: the window holds records");
        let (ids, cursors) = every_page(&ledger, query, *limit);
        assert_eq!(&ids, expected, "{form}: the pages' rows and their order");
        assert!(
            cursors.windows(2).all(|pair| pair[0] < pair[1]),
            "{form}: every cursor is past the one before: {cursors:?}"
        );
    }
    ledger.close().expect("close");
}

#[test]
fn ln_ws_t5_a_window_page_starts_at_the_windows_first_record_and_reads_the_unbounded_rows() {
    let root = Root::new(SMALL_RECORDS);
    let (since, _) = window(root.records);
    let ledger = root.open(Reads::unbounded(BOUND_SCAN_ROWS));
    let events = ledger.query(EventQuery::default()).expect("every record");
    let indexes = EventIndexes::from_events(&events);
    let first = events
        .iter()
        .filter(|event| event.timestamp_unix_ms() >= since)
        .map(PersistedEvent::sequence)
        .min()
        .expect("a record in the window");
    // The inverted record, halfway through the history, is the window's first.
    assert!(
        first < root.window_start(),
        "a record before the window's own start lies in it: {first}, {}",
        root.window_start()
    );
    let inverted = events
        .iter()
        .find(|event| event.sequence() == first)
        .expect("the inverted record");
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("the recorder");
    for (form, query) in [
        ("A none", root.form_a(None)),
        ("A changes", root.form_a(Some(LedgerView::Changes))),
    ] {
        ledger
            .project_view_page(query.clone(), PROFILE, first_page(PAGE_LIMIT))
            .expect("the first page");
        let read = page_read(&recorder);
        let lower = read
            .statements
            .iter()
            .find(|statement| statement.sql.starts_with(HISTORY_STATEMENT))
            .and_then(|statement| statement.after)
            .unwrap_or_else(|| panic!("{form}: no history statement: {read:?}"));
        assert!(
            lower + 1 >= first,
            "{form}: the first history statement starts after {lower}, below {first} - 1, the \
             window's first record"
        );
        let (ids, _) = every_page(&ledger, &query, PAGE_LIMIT);
        assert_eq!(
            ids,
            unbounded_ids(&events, &indexes, &query, PAGE_LIMIT),
            "{form}: the rows of an unbounded read"
        );
        let _ = recorder.take();
    }
    assert!(
        every_page(&ledger, &root.form_a(None), PAGE_LIMIT)
            .0
            .contains(inverted.event_id()),
        "the inverted record is among the window's rows"
    );
    drop(recorder);
    ledger.close().expect("close");
}
