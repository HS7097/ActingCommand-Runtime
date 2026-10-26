// SPDX-License-Identifier: AGPL-3.0-only

// #419 DIAG-RUN (brief rd-1026/brief-diag-run.md; root cause rd-1026/stop2/root-cause.md §5; the
// production stop of 2026-10-26 23:23:04): a run's read reads its run. At the end of every run
// the task diagnostic reads the run's `artifact.verified` records (`artifact.verified` + request +
// task + run, pages of 256 from sequence 0). Without statistics SQLite drove that statement by
// the event type index from sequence 0 and looked up each entry's link row: it walked every
// `artifact.verified` record of the history inside its first step, about 0.3 s warm and more
// than the writer's 10 s reply limit cold, and the Runtime stopped. `locate_error_frame` and
// `linked_policy_run_events` read the same way.
//
// The ledger's part, on the synthetic ledger (LN-1) in production's shape: three instances in
// turn, runs of 24 frames with every second frame stored (`artifact.verified` about one record in
// nineteen), then runs the test appends through the writer, each frame a capture and its stored
// frame, each run with its lease grant:
// - T1 plan: the statement of each per-run read form (the diagnostic's; the error frame's; one
//   frame's artifact; the lease grant of `linked_policy_run_events`; event types with a request;
//   an artifact of one instance's run) is driven by a per-run link index, never by the event
//   type, time, severity, origin or instance index, and passes the F6 plan check; forms A and C
//   of stop 1 (no per-run link) keep today's statement;
// - T2 growth: each form's first page for a short run at the head takes at most 1.5 times the
//   SQLite work on 12 000 records that it takes on 3 000 (it was linear in the history's
//   `artifact.verified` records, about 4 times);
// - T3 equality: paging each form from 0 to the end, in pages of 256 and of 2, returns the
//   records the query selects, in ledger order, for a short run at the head, a run with more
//   than 256 `artifact.verified` records interleaved with two other runs, and a run with a record
//   whose time is earlier than the whole history's.

use super::plan_check::check_plan;
use super::*;
use crate::global::tests::sealed_global_ledger::GlobalLedgerSink;
use crate::global::{GlobalLedger, LedgerMaintenance, Sha256SecretFingerprinter};
use crate::{SyntheticInstance, SyntheticLedgerSpec, write_synthetic_ledger};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreResult, ArtifactWriteContext,
    ArtifactWriteRequest,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactProducer,
    ArtifactRedactionState, AuditInput, CapturePayloadDraft, CorrelationId, EffectDisposition,
    EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventSeverity, EventSource,
    FrameId, IdentifierIssuer, InstanceId, IssuedCorrelationId, IssuedRunId, LeasePayloadDraft,
    OriginModule, RequestId, RetentionClass, TaskId,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits};
use actingcommand_runtime_state::RuntimeStateStore;
use rusqlite::StatementStatus;
use std::path::Path;
use tempfile::TempDir;

// The State key, the writer's owner id and the fixture salt: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-diag-run-fixture";
const OWNER: &str = "ledger-diag-run-fixture";
const SALT: &[u8] = b"ledger-diag-run-fixture-salt";
// Two synthetic histories of the same shape, one four times the other.
const SMALL_RECORDS: u64 = 3_000;
const LARGE_RECORDS: u64 = 12_000;
// The synthetic runs: 24 frames, every second one stored, on three instances in turn.
const FRAMES_PER_RUN: u64 = 24;
const STORED_FRAME_EVERY: u64 = 2;
const INSTANCE_LETTERS: &str = "abc";
// The appended runs: a short run at the head; a long run with more than a page of stored frames,
// interleaved frame by frame with two other runs, one of which has a frame stamped before the
// whole history.
const HEAD_FRAMES: usize = 8;
const LONG_FRAMES: usize = 260;
const INVERTED_FRAME: usize = 20;
// The appended records' times (after the synthetic history's), and the inverted frame's time
// (the synthetic history's first, 2025-06-15).
const APPENDED_FIRST_MS: u64 = 1_760_000_000_000;
const RECORD_INTERVAL_MS: u64 = 500;
const INVERTED_MS: u64 = 1_749_945_600_000;
// The frame size every capture names.
const FRAME_WIDTH: u32 = 1_280;
const FRAME_HEIGHT: u32 = 720;
// The task diagnostic's page, and `linked_policy_run_events`' page.
const PAGE_ROWS: usize = 256;
const LINKED_PAGE_ROWS: usize = 2;
// A broken paging loop ends after this many pages.
const MAX_PAGES: usize = 10_000;
// The per-run link indexes (every link but the instance), and the indexes no per-run read may
// use: the view indexes of the event row and the instance link's.
const PER_RUN_INDEXES: [&str; 9] = [
    "ledger_links_request",
    "ledger_links_correlation",
    "ledger_links_causation",
    "ledger_links_task",
    "ledger_links_run",
    "ledger_links_lease",
    "ledger_links_frame",
    "ledger_links_action",
    "ledger_links_recognition",
];
const HISTORY_INDEXES: [&str; 6] = [
    "ledger_view_index_type_v1",
    "ledger_view_index_time_v1",
    "ledger_view_index_severity_v1",
    "ledger_view_index_source_v1",
    "ledger_view_index_module_v1",
    "ledger_links_instance",
];

fn instance(letter: char) -> InstanceId {
    serde_json::from_str(&format!("\"instance_{letter:0>32}\"")).expect("a fixture instance id")
}

/// One run's identifiers, and the links its records carry.
struct Run {
    instance: InstanceId,
    request: RequestId,
    correlation: CorrelationId,
    task: TaskId,
    run: RunId,
    issued_run: IssuedRunId,
    issued_correlation: IssuedCorrelationId,
    links: EventLinksDraft,
}

impl Run {
    fn mint(issuer: &IdentifierIssuer, instance: InstanceId) -> Self {
        let request = issuer.mint_request_id().expect("a request id");
        let correlation = issuer.mint_correlation_id().expect("a correlation id");
        let task = issuer.mint_task_id().expect("a task id");
        let run = issuer.mint_run_id().expect("a run id");
        let lease = issuer.mint_lease_id().expect("a lease id");
        Self {
            instance,
            request: *request.transport(),
            correlation: *correlation.transport(),
            task: *task.transport(),
            run: *run.transport(),
            issued_run: run,
            issued_correlation: correlation,
            links: EventLinksDraft::default()
                .with_instance_id(issuer.issue_registered_instance(instance))
                .with_request_id(request)
                .with_correlation_id(correlation)
                .with_task_id(task)
                .with_run_id(run)
                .with_lease_id(lease),
        }
    }
}

/// The per-run read forms of the root cause's DR-T1 for `run`, `frame` one of its frames.
fn per_run_forms(run: &Run, frame: FrameId) -> Vec<(&'static str, EventQuery)> {
    vec![
        (
            "the task diagnostic's",
            EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                request_id: Some(run.request),
                task_id: Some(run.task),
                run_id: Some(run.run),
                ..EventQuery::default()
            },
        ),
        (
            "the error frame's",
            EventQuery {
                event_type: Some(EventType::CaptureCompleted),
                run_id: Some(run.run),
                ..EventQuery::default()
            },
        ),
        (
            "one frame's artifact",
            EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                run_id: Some(run.run),
                frame_id: Some(frame),
                ..EventQuery::default()
            },
        ),
        (
            "the lease grant of linked_policy_run_events",
            EventQuery {
                event_type: Some(EventType::LeaseGranted),
                request_id: Some(run.request),
                correlation_id: Some(run.correlation),
                run_id: Some(run.run),
                ..EventQuery::default()
            },
        ),
        (
            "event types with a request",
            EventQuery {
                event_types: vec![
                    EventType::ArtifactVerified,
                    EventType::CaptureCompleted,
                    EventType::LeaseGranted,
                ],
                request_id: Some(run.request),
                ..EventQuery::default()
            },
        ),
        (
            "an artifact of one instance's run",
            EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                instance_id: Some(run.instance),
                run_id: Some(run.run),
                ..EventQuery::default()
            },
        ),
    ]
}

/// The records the artifact store gives while it stores one frame, in order.
#[derive(Default)]
struct Drafts(Vec<EventDraft>);

impl ArtifactEventSink for Drafts {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        self.0.push(draft);
        Ok(())
    }
}

/// The runs the test appends through the writer: each frame a capture and its frame stored by
/// the root's artifact store as the capture store stores one, each record captured in order.
struct Appender<'a> {
    ledger: &'a GlobalLedger,
    store: ArtifactStore,
    issuer: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
    records: u64,
}

impl<'a> Appender<'a> {
    fn new(ledger: &'a GlobalLedger, root: &Path) -> Self {
        let store = ArtifactStore::open(root).expect("the root's artifact store");
        let mut admission = GlobalLedgerSink::new(None);
        admission.capacity_root = Some(store.root().to_path_buf());
        store
            .install_capacity_admission(Arc::new(admission))
            .expect("the fixture's capacity owner");
        Self {
            ledger,
            store,
            issuer: IdentifierIssuer::new().expect("identifier issuer"),
            fingerprinter: Sha256SecretFingerprinter::new(SALT).expect("fingerprinter"),
            records: 0,
        }
    }

    /// The next record's time, or the inverted time.
    fn at(&mut self, inverted: bool) -> u64 {
        self.records += 1;
        if inverted {
            INVERTED_MS
        } else {
            APPENDED_FIRST_MS + RECORD_INTERVAL_MS * self.records
        }
    }

    fn capture(&self, draft: EventDraft) {
        let draft = draft
            .sanitize(&self.fingerprinter)
            .expect("a fixture record");
        self.ledger
            .append_deferred(draft)
            .expect("the record is captured");
    }

    /// A run of `instance` with its lease grant.
    fn run(&mut self, instance: InstanceId) -> Run {
        let run = Run::mint(&self.issuer, instance);
        let at = self.at(false);
        self.capture(EventDraft::new(
            self.issuer.mint_event_id().expect("an event id"),
            at,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
            ),
            run.links.clone(),
            LeasePayloadDraft::granted(
                EventAction::LeaseAcquire,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        ));
        run
    }

    /// One frame of `run`: its capture and its stored frame (`artifact.created`,
    /// `artifact.verified`), all three at the inverted time when `inverted`.
    fn frame(&mut self, run: &Run, inverted: bool) -> FrameId {
        let frame = self.issuer.mint_frame_id().expect("a frame id");
        let links = run.links.clone().with_frame_id(frame);
        let at = self.at(inverted);
        self.capture(EventDraft::new(
            self.issuer.mint_event_id().expect("an event id"),
            at,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Device,
                OriginModule::Capture,
                EventActor::Runtime,
            ),
            links.clone(),
            CapturePayloadDraft::completed_with_capture_acquire(
                EventAction::CaptureObserve,
                EffectDisposition::NotPerformed,
                FRAME_WIDTH,
                FRAME_HEIGHT,
                Some(18_000),
                AuditInput::new(),
            )
            .into(),
        ));
        let at = self.at(inverted);
        let material = format!("diag-run-frame-{}", self.records);
        let mut drafts = Drafts::default();
        self.store
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    material.as_bytes(),
                    ArtifactWriteContext::new(
                        ArtifactLinksDraft::default()
                            .with_run_id(run.issued_run)
                            .with_frame_id(frame)
                            .with_correlation_id(run.issued_correlation),
                        links,
                        at,
                    ),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CaptureStore,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut drafts,
            )
            .expect("the frame is stored");
        assert_eq!(drafts.0.len(), 2, "a stored frame's two records");
        for draft in drafts.0 {
            self.capture(draft);
        }
        *frame.transport()
    }

    /// Every captured record committed.
    fn commit(&self) {
        let summary = self
            .ledger
            .confirm_deferred(Duration::from_secs(600))
            .expect("the confirmation");
        assert!(
            summary.failed.is_empty() && summary.pending == 0,
            "every appended record commits: {:?}, {} pending",
            summary.failed,
            summary.pending
        );
    }
}

/// A formal root holding the synthetic history of `records` records, then (with `interleaved`)
/// the interleaved runs, then the short run at the head; its writer open.
struct Root {
    _dir: TempDir,
    database: Arc<RuntimeDatabase>,
    ledger: GlobalLedger,
    head: u64,
    /// The short run at the head and its first frame.
    short: (Run, FrameId),
    /// The long interleaved run and its first frame.
    long: Option<(Run, FrameId)>,
    /// The two runs interleaved with it (the first holds the inverted frame), each with its
    /// first frame.
    others: Vec<(Run, FrameId)>,
}

impl Root {
    fn new(records: u64, interleaved: bool) -> Self {
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
        let instances = INSTANCE_LETTERS
            .chars()
            .map(|letter| SyntheticInstance {
                alias: format!("fixture-instance-{letter}"),
                instance_id: instance(letter),
            })
            .collect();
        let mut spec = SyntheticLedgerSpec::new(records, instances);
        spec.mix.frames_per_run_min = FRAMES_PER_RUN;
        spec.mix.frames_per_run_max = FRAMES_PER_RUN;
        spec.mix.frame_artifact_every_frames = STORED_FRAME_EVERY;
        let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
        assert_eq!(generated.records, records, "{generated:?}");
        assert!(generated.stored_frames > 0, "{generated:?}");
        let ledger = LedgerMaintenance::acquire(
            dir.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner")
        .open_writer(Arc::clone(&database), OWNER.into())
        .expect("the writer opens");
        let mut appender = Appender::new(&ledger, dir.path());
        let mut long = None;
        let mut others = Vec::new();
        if interleaved {
            let long_run = appender.run(instance('a'));
            let other_runs = [appender.run(instance('b')), appender.run(instance('c'))];
            let mut long_frames = Vec::new();
            let mut other_frames = [Vec::new(), Vec::new()];
            for index in 0..LONG_FRAMES {
                long_frames.push(appender.frame(&long_run, false));
                let other = index % 2;
                let inverted = other == 0 && index / 2 == INVERTED_FRAME;
                other_frames[other].push(appender.frame(&other_runs[other], inverted));
            }
            long = Some((long_run, long_frames[0]));
            for (run, frames) in other_runs.into_iter().zip(other_frames) {
                others.push((run, frames[0]));
            }
        }
        let short_run = appender.run(instance('a'));
        let short_frames = (0..HEAD_FRAMES)
            .map(|_| appender.frame(&short_run, false))
            .collect::<Vec<_>>();
        appender.commit();
        drop(appender);
        let head = ledger.latest_sequence().expect("the head");
        Self {
            _dir: dir,
            database,
            ledger,
            head,
            short: (short_run, short_frames[0]),
            long,
            others,
        }
    }
}

/// The outer loop of a plan: its first search or scan.
fn outer_loop(plan: &[String]) -> Option<&str> {
    plan.iter()
        .map(String::as_str)
        .find(|step| step.starts_with("SEARCH ") || step.starts_with("SCAN "))
}

/// The SQLite work of the first page of `query` through `through` (the diagnostic's first read),
/// and the rows it returned.
fn first_page(connection: &Connection, query: &EventQuery, through: u64) -> (usize, i32) {
    let (sql, values) =
        history_statement(&HistorySelection::of(query), None, (0, through), PAGE_ROWS)
            .expect("the page's statement");
    let mut statement = connection.prepare(&sql).expect("prepare the page");
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| row.get::<_, i64>(0))
        .expect("the page")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read the page")
        .len();
    (rows, statement.get_status(StatementStatus::VmStep))
}

/// Each form's first page for the short run at the head of a root of `records` synthetic
/// records: its rows and its SQLite work.
fn head_run_pages(records: u64) -> Vec<(&'static str, usize, i32)> {
    let root = Root::new(records, false);
    let Root {
        database,
        ledger,
        head,
        short: (run, frame),
        ..
    } = root;
    ledger.close().expect("close the writer");
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("connection");
    per_run_forms(&run, frame)
        .into_iter()
        .map(|(form, query)| {
            let (rows, steps) = first_page(&connection, &query, head);
            (form, rows, steps)
        })
        .collect()
}

/// Every page of `query` through `through`, `rows` records a page, from sequence 0 until a page
/// is empty, as the task diagnostic pages: the records' sequences.
fn every_page(ledger: &GlobalLedger, query: &EventQuery, through: u64, rows: usize) -> Vec<u64> {
    let mut sequences = Vec::new();
    let mut after = 0;
    for _ in 0..MAX_PAGES {
        let page = ledger
            .query_page(query.clone(), after, through, rows)
            .expect("the page");
        let Some(last) = page.last() else {
            return sequences;
        };
        after = last.sequence();
        sequences.extend(page.iter().map(PersistedEvent::sequence));
    }
    panic!("{query:?} did not end within {MAX_PAGES} pages of {rows}");
}

#[test]
fn diag_run_t1_a_per_run_read_is_driven_by_its_run_link_index() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let run = Run::mint(&issuer, instance('a'));
    let frame = *issuer.mint_frame_id().expect("a frame id").transport();
    for (form, query) in per_run_forms(&run, frame) {
        let (sql, _) = history_statement(&HistorySelection::of(&query), None, (0, 1), PAGE_ROWS)
            .expect("the page's statement");
        let check = check_plan(&sql);
        let outer = outer_loop(&check.plan).unwrap_or_default();
        assert!(
            outer.starts_with("SEARCH l USING ")
                && PER_RUN_INDEXES
                    .iter()
                    .any(|index| outer.contains(&format!("INDEX {index} ("))),
            "{form}: the outer loop is not a per-run link index: {check:?}\n{sql}"
        );
        assert!(
            check
                .plan
                .iter()
                .all(|step| HISTORY_INDEXES.iter().all(|index| !step.contains(index))),
            "{form}: the plan uses a history-wide index: {check:?}\n{sql}"
        );
        assert!(check.violations.is_empty(), "{form}: {check:?}\n{sql}");
    }

    // Forms A and C of stop 1 have no per-run link: their statements are today's.
    let since = APPENDED_FIRST_MS;
    for (form, query) in [
        (
            "A: no instance, a window",
            EventQuery {
                from_timestamp_unix_ms: Some(since),
                to_timestamp_unix_ms: Some(since + RECORD_INTERVAL_MS),
                ..EventQuery::default()
            },
        ),
        (
            "C: one event type of one instance since a time",
            EventQuery {
                event_type: Some(EventType::TaskRequested),
                instance_id: Some(run.instance),
                from_timestamp_unix_ms: Some(since),
                ..EventQuery::default()
            },
        ),
    ] {
        let (sql, _) = history_statement(&HistorySelection::of(&query), None, (0, 1), PAGE_ROWS)
            .expect("the page's statement");
        assert!(
            !sql.contains('+'),
            "{form}: a statement without a per-run link is unchanged: {sql}"
        );
    }
}

#[test]
fn diag_run_t2_a_run_reads_work_does_not_grow_with_the_history() {
    let small = head_run_pages(SMALL_RECORDS);
    let large = head_run_pages(LARGE_RECORDS);
    let mut grown = Vec::new();
    for ((form, small_rows, small_steps), (_, large_rows, large_steps)) in small.iter().zip(&large)
    {
        assert_eq!(
            small_rows, large_rows,
            "{form}: the head run's page holds the same records on both roots"
        );
        if 2 * i64::from(*large_steps) > 3 * i64::from(*small_steps) {
            grown.push(format!(
                "{form}: {large_steps} VM steps on {LARGE_RECORDS} records, {small_steps} on \
                 {SMALL_RECORDS}"
            ));
        }
    }
    // The diagnostic's page holds the head run's stored frames.
    assert_eq!(small[0].1, HEAD_FRAMES, "{small:?}");
    assert!(
        grown.is_empty(),
        "the first page of a run at the head grew more than 1.5 times:\n{}",
        grown.join("\n")
    );
}

#[test]
fn diag_run_t3_paging_a_per_run_read_returns_the_selected_records_in_ledger_order() {
    let root = Root::new(SMALL_RECORDS, true);
    let every = root
        .ledger
        .query(EventQuery::default())
        .expect("every record");
    let (long, long_frame) = root.long.as_ref().expect("the long run");
    let [(inverted, inverted_frame), (other, other_frame)] = root.others.as_slice() else {
        panic!("two runs interleaved with the long run");
    };
    let (short, short_frame) = &root.short;
    let selected = |query: &EventQuery| {
        let selection = HistorySelection::of(query);
        every
            .iter()
            .filter(|event| selection.selects(*event))
            .collect::<Vec<_>>()
    };

    // The cases are what they claim to be.
    let diagnostic = |run: &Run| per_run_forms(run, *short_frame).remove(0).1;
    let long_records = selected(&diagnostic(long));
    assert!(
        long_records.len() > PAGE_ROWS,
        "the long run has more than a page of artifact.verified records: {}",
        long_records.len()
    );
    let (first, last) = (
        long_records.first().map(|event| event.sequence()),
        long_records.last().map(|event| event.sequence()),
    );
    assert!(
        [inverted, other].into_iter().all(|run| {
            selected(&diagnostic(run))
                .iter()
                .any(|event| Some(event.sequence()) > first && Some(event.sequence()) < last)
        }),
        "both other runs' records lie between the long run's"
    );
    assert!(
        selected(&diagnostic(inverted))
            .iter()
            .any(|event| event.timestamp_unix_ms() == INVERTED_MS),
        "a record of the inverted run is stamped before the whole history"
    );
    assert_eq!(selected(&diagnostic(short)).len(), HEAD_FRAMES);
    assert_eq!(
        every.last().map(PersistedEvent::sequence),
        Some(root.head),
        "the short run is at the head"
    );

    for (case, run, frame) in [
        ("the short run at the head", short, short_frame),
        ("the long interleaved run", long, long_frame),
        ("the run with the inverted record", inverted, inverted_frame),
        ("the other interleaved run", other, other_frame),
    ] {
        for (form, query) in per_run_forms(run, *frame) {
            let expected = selected(&query)
                .iter()
                .map(|event| event.sequence())
                .collect::<Vec<_>>();
            assert!(!expected.is_empty(), "{case}, {form}: the run has records");
            for rows in [PAGE_ROWS, LINKED_PAGE_ROWS] {
                assert_eq!(
                    every_page(&root.ledger, &query, root.head, rows),
                    expected,
                    "{case}, {form}: pages of {rows}"
                );
            }
        }
    }
    root.ledger.close().expect("close the writer");
}
