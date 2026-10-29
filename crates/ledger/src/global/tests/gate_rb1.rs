// SPDX-License-Identifier: AGPL-3.0-only

//! #422 RB-1 (0.12.28; production v0.12.26 after the machine restart of 2026-10-29 05:23;
//! investigation rd-1029/inv-reboot/rb.md, coordinator work order rd-1029/brief-rb1.md). A page
//! whose selection reached a row with a request, correlation or run link read the Lab relations
//! of the whole history (`read_lab_relations`); with a cold cache that read outlasted the page's
//! budget, the page stopped before that row and the next page was refused, so the run listing
//! always failed. The reader pool now builds its Lab relations once in the background and its
//! pages read only the records after the build's anchor. The required states:
//! - T1: with and without the index, the pages are item for item the same, also pinned to old
//!   snapshots: before the build's anchor (the index filtered by sequence), at it, and after it
//!   (the index extended); runs related through a request, and through a correlation only,
//!   before the build and after it.
//! - T2: with the index built, a page that reaches a linked row reads no full history, measured
//!   in the rows its Lab relation statements return, and answers that row although its budget is
//!   spent at every look; without the index, the same page stops before the row and the next one
//!   is refused, as in production.
//! - T3: a build that fails (its deadline passed) is said once as a warning,
//!   `ledger_read_budget_exceeded` of `build_lab_relations`, not fatal, and the pages read the
//!   whole history's relations as before.
//! - T4: a Lab anchor after the build that names a request whose run's records are older than the
//!   build relates those records from its own sequence on, and not before.
//!
//! The roots are formal roots read by the Runtime's writer and its reader pool of one reader.
//! The read budget, when there is one, runs on a writer clock that moves a set step each time it
//! is read; the build waits on its end, never on a sleep.

use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::{
    GlobalLedger, GlobalLedgerError, LedgerFailurePlan, LedgerMaintenance, LedgerMonotonicClock,
    LedgerOpenTiming, LedgerWritePolicy, LedgerWriterCommandKind, Sha256SecretFingerprinter,
};
use crate::{LedgerWriterCommandStatements, PersistedEvent, WriterStatementRecorder};
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, EventAction, EventActor, EventDraft,
    EventId, EventLinksDraft, EventOrigin, EventPayloadDraft, EventQuery, EventSeverity,
    EventSource, IdentifierIssuer, IssuedCorrelationId, IssuedInstanceId, IssuedRequestId,
    IssuedRunId, LedgerView, OriginModule, OwnerEpoch, ProjectionProfile, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, SanitizedEventDraft,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes) and the writer's owner id: neutral
// fixture words.
const STATE_KEY: &[u8] = b"ledger-rb1-fixture-key";
const OWNER: &str = "ledger-rb1-fixture";
const AT_MS: u64 = 1_752_147_200_000;
// The writer's write policy and the open's deadline, as QR-2's.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
const OPEN_TIMEOUT_SECONDS: u64 = 600;
// Every caller waits this long for one reply; no read of these tests comes near it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);
// The one bound of a wait in this file: the deferred appends' confirmation and the build's end.
const GATE_WAIT: Duration = Duration::from_secs(120);
// The build's deadline when it builds (the open's, as `ledger_observer.open_timeout_ms`'s
// default).
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);
// T2: the read budget (`ledger_observer.read_budget_ms`'s default) and the bound's look interval
// (`ledger_observer.read_check_vm_steps`'s default).
const BUDGET_MS: u64 = 1_000;
const CHECK_VM_STEPS: u64 = 10_000;
// T2: the Lab anchors of the history: enough that a statement of the whole history's Lab
// relations runs past one look interval of the bound (as IDLE-OV's corpus).
const MANY_ANCHORS: usize = 2_000;
// T1, T3 and T4: a few Lab anchors.
const FEW_ANCHORS: usize = 4;
const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
const PAGE_LIMIT: u16 = 64;
// The statements of the whole history's Lab relations (`read_lab_relations`): its two anchor
// statements and its run statement.
const FULL_ANCHOR_STATEMENT: &str =
    "FROM ledger_events a JOIN ledger_links al ON al.sequence=a.sequence";
const FULL_RUN_STATEMENT: &str = "SELECT r.run_id FROM ledger_links r";
const FULL_STATEMENTS: usize = 3;
// The statements of the index (`sqlite::lab_relations`): its anchor statement and its two run
// statements.
const INDEX_ANCHOR_STATEMENT: &str =
    "FROM ledger_events x JOIN ledger_links xl ON xl.sequence=x.sequence";
const INDEX_RUN_STATEMENT: &str = "SELECT k.run_id,k.request_id,k.correlation_id";

fn salt() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"rb1-fixture-salt").expect("salt")
}

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

/// A writer clock in milliseconds that moves `step` each time it is read; the test sets the
/// step.
struct Clock {
    now: Arc<AtomicU64>,
    step: Arc<AtomicU64>,
}

impl Clock {
    fn new() -> Self {
        Self {
            now: Arc::new(AtomicU64::new(0)),
            step: Arc::new(AtomicU64::new(1)),
        }
    }

    fn clock(&self) -> LedgerMonotonicClock {
        let now = Arc::clone(&self.now);
        let step = Arc::clone(&self.step);
        Arc::new(move || now.fetch_add(step.load(Ordering::SeqCst), Ordering::SeqCst))
    }

    fn step_ms(&self, step: u64) {
        self.step.store(step, Ordering::SeqCst);
    }
}

/// How the reader pool of an open has its Lab relations.
#[derive(Clone, Copy)]
enum LabIndex {
    /// Built at the start, within `BUILD_TIMEOUT`.
    Built,
    /// None: the pages read the whole history's relations.
    Skipped,
    /// A build whose deadline has passed when it starts.
    Failing,
}

/// A formal root.
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
}

impl Root {
    fn new() -> Self {
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
        Self { dir, database }
    }

    /// The Runtime's writer of the root (a failure plan: it has a reader pool) with one reader,
    /// so that every page is read by the same reader; its pool's Lab relations as `index` says,
    /// and its reads bounded by the read budget on `clock` when one is given.
    fn open(&self, index: LabIndex, clock: Option<&Clock>) -> GlobalLedger {
        let state_root = self.dir.path().canonicalize().expect("state root");
        let mut plan = LedgerFailurePlan::new(
            LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
                .expect("write policy"),
            &state_root,
            None,
            epoch(),
            AT_MS,
        )
        .with_reply_timeout(REPLY_TIMEOUT)
        .with_reader_threads_for_test(1)
        .with_lab_relations_timeout_for_test(match index {
            LabIndex::Built => Some(BUILD_TIMEOUT),
            LabIndex::Skipped => None,
            LabIndex::Failing => Some(Duration::ZERO),
        });
        if let Some(clock) = clock {
            plan = plan
                .with_read_budget(Duration::from_millis(BUDGET_MS))
                .with_clock(clock.clock());
        }
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

    /// The Runtime's writer whose pool built its Lab relations, and the build's anchor (the head
    /// the writer had published when the build started), before any page or record.
    fn open_built(&self, clock: Option<&Clock>) -> (GlobalLedger, u64) {
        let ledger = self.open(LabIndex::Built, clock);
        let built = ledger.lab_relations_settled_for_test(GATE_WAIT);
        let Some(Some(anchor)) = built else {
            panic!("the pool built its Lab relations: {built:?}");
        };
        let head = ledger.latest_sequence().expect("the head");
        assert!(
            0 < anchor && anchor <= head,
            "the build's anchor {anchor} is a committed point (head {head})"
        );
        (ledger, anchor)
    }
}

/// The history's writer: its identities and the time of its next record.
struct History {
    ids: IdentifierIssuer,
    instance_a: IssuedInstanceId,
    instance_c: IssuedInstanceId,
    offset: u64,
}

impl History {
    fn new() -> Self {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let instance_a = ids.mint_instance_id().expect("instance");
        let instance_c = ids.mint_instance_id().expect("instance");
        Self {
            ids,
            instance_a,
            instance_c,
            offset: 0,
        }
    }

    fn request(&self) -> IssuedRequestId {
        self.ids.mint_request_id().expect("request")
    }

    fn correlation(&self) -> IssuedCorrelationId {
        self.ids.mint_correlation_id().expect("correlation")
    }

    fn run(&self) -> IssuedRunId {
        self.ids.mint_run_id().expect("run")
    }

    fn draft(
        &mut self,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> SanitizedEventDraft {
        self.offset += 1;
        EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            AT_MS + self.offset,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&salt())
        .expect("a fixture draft")
    }

    /// A Lab anchor (a Lab request) of instance A with `links`.
    fn anchor_draft(&mut self, links: EventLinksDraft) -> SanitizedEventDraft {
        self.draft(
            EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
            links.with_instance_id(self.instance_a),
            ClientPayloadDraft::lab_request(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        )
    }

    fn anchor(&mut self, ledger: &GlobalLedger, links: EventLinksDraft) -> PersistedEvent {
        let draft = self.anchor_draft(links);
        ledger.append(draft).expect("a Lab anchor")
    }

    /// `count` Lab anchors, each with a request and a correlation of its own, committed in one
    /// confirmation; their requests and correlations in order.
    fn anchors(
        &mut self,
        ledger: &GlobalLedger,
        count: usize,
    ) -> Vec<(IssuedRequestId, IssuedCorrelationId)> {
        let mut anchors = Vec::new();
        for _ in 0..count {
            let (request, correlation) = (self.request(), self.correlation());
            anchors.push((request, correlation));
            let draft = self.anchor_draft(
                EventLinksDraft::default()
                    .with_request_id(request)
                    .with_correlation_id(correlation),
            );
            ledger.append_deferred(draft).expect("accepted");
        }
        let summary = ledger.confirm_deferred(GATE_WAIT).expect("confirmation");
        assert!(summary.failed.is_empty(), "{:?}", summary.failed);
        anchors
    }

    /// A record of instance C (a command a person gave) with `links`.
    fn record(&mut self, ledger: &GlobalLedger, links: EventLinksDraft) -> PersistedEvent {
        let draft = self.draft(
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
            links.with_instance_id(self.instance_c),
            CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        );
        ledger.append(draft).expect("a record of instance C")
    }

    fn instance_c(&self) -> actingcommand_contract::InstanceId {
        *self.instance_c.transport()
    }
}

fn first_page() -> RuntimeEventQueryPageRequest {
    RuntimeEventQueryPageRequest::new(PAGE_LIMIT, None).expect("a first page")
}

fn at(snapshot: u64) -> RuntimeEventQueryPageRequest {
    first_page()
        .at_snapshot(snapshot)
        .expect("a page at the snapshot")
}

/// A page's bytes, as a client receives them.
fn bytes(page: &RuntimeEventQueryPage) -> Vec<u8> {
    serde_json::to_vec(page).expect("a page serializes")
}

/// The reader pool's page of `query` at `snapshot`.
fn pool_page(ledger: &GlobalLedger, query: &EventQuery, snapshot: u64) -> RuntimeEventQueryPage {
    ledger
        .project_view_page(query.clone(), PROFILE, at(snapshot))
        .unwrap_or_else(|error| panic!("the pool's page at {snapshot}: {error:?}"))
}

/// The writer's own page of `query` at `snapshot`: it reads the whole history's Lab relations.
fn writer_page(ledger: &GlobalLedger, query: &EventQuery, snapshot: u64) -> RuntimeEventQueryPage {
    ledger
        .project_view_page_on_writer_for_test(query.clone(), PROFILE, at(snapshot))
        .unwrap_or_else(|error| panic!("the writer's page at {snapshot}: {error:?}"))
}

/// The rows of `page` that name the Lab view among their views.
fn lab_rows(page: &RuntimeEventQueryPage) -> Vec<EventId> {
    page.events()
        .iter()
        .filter(|row| row.views.contains(&LedgerView::Lab))
        .map(|row| row.event_id)
        .collect()
}

fn rows(page: &RuntimeEventQueryPage) -> Vec<EventId> {
    page.events().iter().map(|row| row.event_id).collect()
}

/// The one view page the reader pool read since `recorder` was last read (a pool page is
/// recorded before it is answered).
fn pool_page_read(recorder: &WriterStatementRecorder) -> LedgerWriterCommandStatements {
    let mut pages = recorder
        .take()
        .into_iter()
        .filter(|command| command.command == LedgerWriterCommandKind::ProjectViewPage)
        .collect::<Vec<_>>();
    assert_eq!(pages.len(), 1, "one view page was read: {pages:?}");
    pages.remove(0)
}

/// The statements of the whole history's Lab relations among `read`'s.
fn full_statements(read: &LedgerWriterCommandStatements) -> Vec<&crate::LedgerStatementRun> {
    read.statements
        .iter()
        .filter(|statement| {
            statement.sql.contains(FULL_ANCHOR_STATEMENT)
                || statement.sql.starts_with(FULL_RUN_STATEMENT)
        })
        .collect()
}

/// The statements of the index among `read`'s.
fn index_statements(read: &LedgerWriterCommandStatements) -> Vec<&crate::LedgerStatementRun> {
    read.statements
        .iter()
        .filter(|statement| {
            statement.sql.contains(INDEX_ANCHOR_STATEMENT)
                || statement.sql.starts_with(INDEX_RUN_STATEMENT)
        })
        .collect()
}

fn named(error: &GlobalLedgerError) -> (&'static str, &'static str, bool) {
    (error.code(), error.operation(), error.is_fatal())
}

/// T1's and T4's history before the build, on instance C unless said:
/// - `FEW_ANCHORS` Lab anchors of instance A; a record under the first one's request in run R1,
///   one under the second one's correlation, one in R1 alone, one under the third one's
///   correlation in run R5 without a request and one in R5 alone (R5 is related through that
///   correlation only), one under a request no anchor names;
/// - a record under request R2 in run R2 and one in R2 alone, then a Lab anchor that names R2
///   (it relates both from its own sequence on);
/// - a record under request R3 in run R3 and one in R3 alone; no anchor names R3 yet.
struct Before {
    by_request: EventId,
    /// The third Lab anchor's correlation, which R5's first record names without a request.
    correlation: IssuedCorrelationId,
    /// The record in R5 alone.
    by_correlation: EventId,
    late_request: PersistedEvent,
    late_run: PersistedEvent,
    late_anchor: PersistedEvent,
    old_request: PersistedEvent,
    old_run: PersistedEvent,
    r1: IssuedRunId,
    r3_request: IssuedRequestId,
    r3: IssuedRunId,
}

fn history_before(root: &Root, history: &mut History) -> Before {
    let ledger = root.open(LabIndex::Skipped, None);
    let anchors = history.anchors(&ledger, FEW_ANCHORS);
    let r1 = history.run();
    let by_request = *history
        .record(
            &ledger,
            EventLinksDraft::default()
                .with_request_id(anchors[0].0)
                .with_run_id(r1),
        )
        .event_id();
    history.record(
        &ledger,
        EventLinksDraft::default().with_correlation_id(anchors[1].1),
    );
    history.record(&ledger, EventLinksDraft::default().with_run_id(r1));
    let (correlation, r5) = (anchors[2].1, history.run());
    history.record(
        &ledger,
        EventLinksDraft::default()
            .with_correlation_id(correlation)
            .with_run_id(r5),
    );
    let by_correlation = *history
        .record(&ledger, EventLinksDraft::default().with_run_id(r5))
        .event_id();
    let unrelated = history.request();
    history.record(
        &ledger,
        EventLinksDraft::default().with_request_id(unrelated),
    );
    let (r2_request, r2) = (history.request(), history.run());
    let late_request = history.record(
        &ledger,
        EventLinksDraft::default()
            .with_request_id(r2_request)
            .with_run_id(r2),
    );
    let late_run = history.record(&ledger, EventLinksDraft::default().with_run_id(r2));
    let late_anchor = history.anchor(
        &ledger,
        EventLinksDraft::default()
            .with_request_id(r2_request)
            .with_correlation_id(history.correlation()),
    );
    let (r3_request, r3) = (history.request(), history.run());
    let old_request = history.record(
        &ledger,
        EventLinksDraft::default()
            .with_request_id(r3_request)
            .with_run_id(r3),
    );
    let old_run = history.record(&ledger, EventLinksDraft::default().with_run_id(r3));
    ledger.close().expect("close");
    Before {
        by_request,
        correlation,
        by_correlation,
        late_request,
        late_run,
        late_anchor,
        old_request,
        old_run,
        r1,
        r3_request,
        r3,
    }
}

#[test]
fn rb1_t1_pages_with_and_without_the_index_are_the_same_also_at_old_snapshots() {
    let root = Root::new();
    let mut history = History::new();
    let before = history_before(&root, &mut history);
    let (ledger, anchor) = root.open_built(None);
    // After the build: a record without a link (so the snapshot before the next anchor is past
    // the build's); a record under the third Lab anchor's correlation in a new run R6 without a
    // request and one in R6 alone (R6 is related through that correlation only); a Lab anchor
    // that names the old request R3, a record in R3; a new Lab anchor with its own request R4 and
    // correlation C4, a record under R4 in run R4, one under C4, one in R4 alone; a record in the
    // Lab run R1 and one under a request no anchor names.
    history.record(&ledger, EventLinksDraft::default());
    let r6 = history.run();
    history.record(
        &ledger,
        EventLinksDraft::default()
            .with_correlation_id(before.correlation)
            .with_run_id(r6),
    );
    let by_correlation = *history
        .record(&ledger, EventLinksDraft::default().with_run_id(r6))
        .event_id();
    let new_anchor = history.anchor(
        &ledger,
        EventLinksDraft::default()
            .with_request_id(before.r3_request)
            .with_correlation_id(history.correlation()),
    );
    history.record(&ledger, EventLinksDraft::default().with_run_id(before.r3));
    let (r4_request, r4_correlation, r4) =
        (history.request(), history.correlation(), history.run());
    let r4_anchor = history.anchor(
        &ledger,
        EventLinksDraft::default()
            .with_request_id(r4_request)
            .with_correlation_id(r4_correlation),
    );
    history.record(
        &ledger,
        EventLinksDraft::default()
            .with_request_id(r4_request)
            .with_run_id(r4),
    );
    history.record(
        &ledger,
        EventLinksDraft::default().with_correlation_id(r4_correlation),
    );
    history.record(&ledger, EventLinksDraft::default().with_run_id(r4));
    history.record(&ledger, EventLinksDraft::default().with_run_id(before.r1));
    let unrelated = history.request();
    history.record(
        &ledger,
        EventLinksDraft::default().with_request_id(unrelated),
    );
    let head = ledger.latest_sequence().expect("the head");

    // The snapshots: before R2's anchor and at it (both before the build's anchor: the index
    // filtered), at the build's anchor, after it before and at R3's anchor, at R4's anchor and at
    // the head (the index extended); in turn up, then down (the extended index filtered).
    let snapshots = [
        before.late_run.sequence(),
        before.late_anchor.sequence(),
        anchor,
        new_anchor.sequence() - 1,
        new_anchor.sequence(),
        r4_anchor.sequence(),
        head,
    ];
    assert!(
        snapshots[1] < anchor && anchor < snapshots[3],
        "the snapshots lie on both sides of the build's anchor {anchor}: {snapshots:?}"
    );
    let queries = [
        EventQuery {
            instance_id: Some(history.instance_c()),
            ..EventQuery::default()
        },
        EventQuery {
            view: Some(LedgerView::Lab),
            instance_id: Some(history.instance_c()),
            ..EventQuery::default()
        },
        EventQuery {
            view: Some(LedgerView::Lab),
            ..EventQuery::default()
        },
    ];
    let mut with_index = Vec::new();
    for snapshot in snapshots.iter().chain(snapshots.iter().rev()) {
        for (number, query) in queries.iter().enumerate() {
            let page = pool_page(&ledger, query, *snapshot);
            assert_eq!(
                bytes(&page),
                bytes(&writer_page(&ledger, query, *snapshot)),
                "query {number} at {snapshot}: the pool's page with the index is the writer's"
            );
            with_index.push((number, *snapshot, bytes(&page)));
        }
    }
    // The relations follow each snapshot: R2's records are Lab-related from R2's anchor on, R3's
    // from the new anchor on, though both were committed before the build.
    let instance_c = &queries[0];
    let related = |snapshot: u64| lab_rows(&pool_page(&ledger, instance_c, snapshot));
    let late = [*before.late_request.event_id(), *before.late_run.event_id()];
    let old = [*before.old_request.event_id(), *before.old_run.event_id()];
    assert!(
        late.iter()
            .all(|row| !related(before.late_run.sequence()).contains(row)),
        "R2's records before R2's anchor"
    );
    assert!(
        late.iter()
            .all(|row| related(before.late_anchor.sequence()).contains(row)),
        "R2's records at R2's anchor"
    );
    assert!(
        old.iter()
            .all(|row| !related(new_anchor.sequence() - 1).contains(row)),
        "R3's records before the new anchor"
    );
    assert!(
        old.iter().all(|row| related(head).contains(row))
            && related(head).contains(&before.by_request),
        "R3's records and the first anchor's at the head"
    );
    assert!(
        [before.by_correlation, by_correlation]
            .iter()
            .all(|row| related(head).contains(row)),
        "R5's and R6's records in their run alone, related through a correlation only, at the head"
    );
    ledger.close().expect("close");

    // Without the index (the pool builds none), the pool's pages are the same, item for item.
    let ledger = root.open(LabIndex::Skipped, None);
    for (number, snapshot, expected) in &with_index {
        assert_eq!(
            &bytes(&pool_page(&ledger, &queries[*number], *snapshot)),
            expected,
            "query {number} at {snapshot}: the pool's page without the index"
        );
    }
    ledger.close().expect("close");
}

#[test]
fn rb1_t2_a_linked_page_with_the_index_reads_no_full_history_and_answers_within_its_budget() {
    let root = Root::new();
    let mut history = History::new();
    let ledger = root.open(LabIndex::Skipped, None);
    let anchors = history.anchors(&ledger, MANY_ANCHORS);
    ledger.close().expect("close");

    // With the index built at the start, a record under a Lab anchor's request after it (the
    // page's linked row), and one after that.
    let clock = Clock::new();
    let (ledger, anchor) = root.open_built(Some(&clock));
    let linked = history.record(
        &ledger,
        EventLinksDraft::default().with_request_id(anchors[2].0),
    );
    history.record(&ledger, EventLinksDraft::default());
    let snapshot = ledger.latest_sequence().expect("the head");
    let query = EventQuery {
        instance_id: Some(history.instance_c()),
        from_sequence: Some(linked.sequence()),
        ..EventQuery::default()
    };
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("the recorder");

    // From now on every look at the clock finds the budget spent: the page's first statement
    // runs (it is exempt), its Lab relations' statements run under the bound.
    clock.step_ms(2 * BUDGET_MS);
    let page = ledger.project_view_page(query.clone(), PROFILE, at(snapshot));
    clock.step_ms(1);
    let page = page.expect("the linked page with the index is answered");
    assert_eq!(
        rows(&page).first(),
        Some(linked.event_id()),
        "the page answers its linked row"
    );
    assert!(
        page.events()[0].views.contains(&LedgerView::Lab),
        "the linked row names the Lab view"
    );
    let read = pool_page_read(&recorder);
    assert!(
        full_statements(&read).is_empty(),
        "no statement of the whole history's Lab relations: {read:?}"
    );
    let index = index_statements(&read);
    assert!(
        !index.is_empty(),
        "the page read the records after the index: {read:?}"
    );
    let index_rows = index.iter().map(|statement| statement.rows).sum::<u64>();
    assert!(
        index_rows <= snapshot - anchor,
        "the index's statements returned {index_rows} rows, at most one a record after the \
         build's anchor {anchor} (snapshot {snapshot})"
    );
    assert!(
        index
            .iter()
            .all(|statement| statement.vm_steps < CHECK_VM_STEPS),
        "each statement of the index stays within one look interval: {index:?}"
    );
    drop(recorder);
    ledger.close().expect("close");

    // Without the index, the same pages, as in production: the first stops before the linked
    // row, the next is refused.
    let ledger = root.open(LabIndex::Skipped, Some(&clock));
    clock.step_ms(2 * BUDGET_MS);
    let first = ledger.project_view_page(query.clone(), PROFILE, at(snapshot));
    let cursor = first
        .as_ref()
        .ok()
        .and_then(|page| page.next_cursor().cloned());
    let next = cursor.clone().map(|cursor| {
        ledger.project_view_page(
            query.clone(),
            PROFILE,
            RuntimeEventQueryPageRequest::new(PAGE_LIMIT, Some(cursor)).expect("the next page"),
        )
    });
    clock.step_ms(1);
    let first = first.expect("the first page without the index is answered");
    assert!(
        rows(&first).is_empty() && first.has_more(),
        "it ends before the linked row: {:?}",
        rows(&first)
    );
    assert_eq!(
        cursor.map(|cursor| cursor.after_sequence()),
        Some(linked.sequence() - 1),
        "its cursor is just before the linked row"
    );
    let error = next
        .expect("a next page")
        .expect_err("the next page without the index is refused");
    assert_eq!(
        named(&error),
        (
            LedgerCode::LedgerReadBudgetExceeded.as_str(),
            LedgerLocation::ProjectLedgerViewPage.as_str(),
            false
        ),
        "{error:?}"
    );

    // Its budget whole, the page without the index reads the whole history's Lab relations:
    // thousands of rows, and a statement past one look interval.
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("the recorder");
    let page = pool_page(&ledger, &query, snapshot);
    assert_eq!(rows(&page).first(), Some(linked.event_id()));
    let read = pool_page_read(&recorder);
    let full = full_statements(&read);
    assert_eq!(full.len(), FULL_STATEMENTS, "{read:?}");
    let full_rows = full.iter().map(|statement| statement.rows).sum::<u64>();
    assert!(
        full_rows >= MANY_ANCHORS as u64,
        "the whole history's Lab relations returned {full_rows} rows"
    );
    assert!(
        full.iter()
            .any(|statement| statement.vm_steps > CHECK_VM_STEPS),
        "a statement of the whole history's Lab relations runs past one look interval: {full:?}"
    );
    drop(recorder);
    ledger.close().expect("close");
}

#[test]
fn rb1_t3_a_failed_build_is_a_warning_and_the_pages_read_as_before() {
    let root = Root::new();
    let mut history = History::new();
    let before = history_before(&root, &mut history);
    let ledger = root.open(LabIndex::Failing, None);
    assert_eq!(
        ledger.lab_relations_settled_for_test(GATE_WAIT),
        Some(None),
        "the build ended without the Lab relations"
    );
    let findings = ledger.take_reader_pool_findings();
    assert_eq!(
        findings.iter().map(named).collect::<Vec<_>>(),
        vec![(
            LedgerCode::LedgerReadBudgetExceeded.as_str(),
            LedgerLocation::BuildLabRelations.as_str(),
            false
        )],
        "{findings:?}"
    );
    assert!(
        ledger.take_reader_pool_findings().is_empty(),
        "the failed build is said once"
    );
    assert!(ledger.reads_off_writer(), "the pool still reads the pages");

    // A page that needs its Lab relations reads the whole history's, as before, and is the
    // writer's page.
    let head = ledger.latest_sequence().expect("the head");
    let query = EventQuery {
        instance_id: Some(history.instance_c()),
        ..EventQuery::default()
    };
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("the recorder");
    let page = pool_page(&ledger, &query, head);
    let read = pool_page_read(&recorder);
    assert_eq!(full_statements(&read).len(), FULL_STATEMENTS, "{read:?}");
    assert!(index_statements(&read).is_empty(), "{read:?}");
    drop(recorder);
    assert!(
        lab_rows(&page).contains(&before.by_request),
        "the first anchor's run names the Lab view"
    );
    assert_eq!(bytes(&page), bytes(&writer_page(&ledger, &query, head)));
    ledger.close().expect("close");
}

#[test]
fn rb1_t4_a_new_anchor_that_names_an_old_request_relates_its_records_from_its_own_sequence() {
    let root = Root::new();
    let mut history = History::new();
    let before = history_before(&root, &mut history);
    let (ledger, anchor) = root.open_built(None);
    assert!(
        before.old_run.sequence() <= anchor,
        "R3's records are older than the build's anchor"
    );
    // After the build: a record without a link, then a Lab anchor that names the old request R3,
    // then a record in R3.
    let filler = history.record(&ledger, EventLinksDraft::default());
    let new_anchor = history.anchor(
        &ledger,
        EventLinksDraft::default()
            .with_request_id(before.r3_request)
            .with_correlation_id(history.correlation()),
    );
    let later = history.record(&ledger, EventLinksDraft::default().with_run_id(before.r3));
    let query = EventQuery {
        instance_id: Some(history.instance_c()),
        ..EventQuery::default()
    };
    let lab_query = EventQuery {
        view: Some(LedgerView::Lab),
        instance_id: Some(history.instance_c()),
        ..EventQuery::default()
    };
    let old = [*before.old_request.event_id(), *before.old_run.event_id()];

    // The first page past the build reads only the records after it; the new anchor relates
    // R3's request and run from its own sequence on, the old records included.
    let at_anchor = pool_page(&ledger, &query, new_anchor.sequence());
    assert!(
        old.iter().all(|row| lab_rows(&at_anchor).contains(row)),
        "R3's old records at the new anchor: {:?}",
        lab_rows(&at_anchor)
    );
    let head = pool_page(&ledger, &query, later.sequence());
    assert!(
        old.iter()
            .chain([later.event_id()])
            .all(|row| lab_rows(&head).contains(row)),
        "R3's records at the head"
    );
    // Before the new anchor (the index extended past it, filtered), R3's records are not
    // related.
    let before_anchor = pool_page(&ledger, &query, filler.sequence());
    assert!(
        old.iter().all(
            |row| rows(&before_anchor).contains(row) && !lab_rows(&before_anchor).contains(row)
        ),
        "R3's records before the new anchor"
    );
    // The Lab view of instance C at the head: the first anchor's records and R2's and R3's.
    let lab_view = pool_page(&ledger, &lab_query, later.sequence());
    assert!(
        old.iter()
            .chain([later.event_id(), &before.by_request])
            .all(|row| rows(&lab_view).contains(row)),
        "the Lab view at the head: {:?}",
        rows(&lab_view)
    );
    // Each is the writer's page at its snapshot, which reads the whole history's relations.
    for (page, query, snapshot) in [
        (&at_anchor, &query, new_anchor.sequence()),
        (&head, &query, later.sequence()),
        (&before_anchor, &query, filler.sequence()),
        (&lab_view, &lab_query, later.sequence()),
    ] {
        assert_eq!(
            bytes(page),
            bytes(&writer_page(&ledger, query, snapshot)),
            "at {snapshot}: the pool's page is the writer's"
        );
    }
    ledger.close().expect("close");
}
