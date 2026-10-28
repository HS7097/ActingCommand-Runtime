// SPDX-License-Identifier: AGPL-3.0-only

// #422 QR-1 (brief rd-1028/brief-qr1-reads-off-writer.md; Alice 10-28: 「账本查询和清理用的是同一个
// 写入线程……至少得异步，最好多线程执行」): view pages are read by the Runtime writer's reader pool,
// off the writer thread, and the frame cleaner's trim reaches the writer in bounded chunks.
// - Q-1: while the writer is held inside an append's write transaction (`WriterStallGate`), a view
//   page is answered by the reader pool, with the bytes of the page the writer itself reads.
// - Q-2: a pass start's trim hands the writer chunks that look at no more objects than the chunk's
//   bound; between two chunks an append and a view page both complete.
// - Q-3: the read budget stops a reader-pool page while the writer is held, as it stopped the
//   writer's (incomplete at its budget, a cursor past its start), and a pool page not answered
//   within the reply limit is `writer_response_timeout`, not fatal, as before.
// - Q-4: a page never returns a record past the committed head while the writer appends: not the
//   record of a write transaction still open, and never past the head read after the page.
//
// Fix 1 (brief rd-1028/brief-qr1-fix1.md, review rd-1028/review-qr1.md):
// - F-1 (M-1): a page whose caller's deadline passed while it waited in the queue is answered
//   with the caller's `writer_response_timeout` and never read.
// - F-2 (M-1): a close with pages queued reads none of them; each is answered as refused by the
//   time the close returns, which waits for the page in progress only.
// - F-3 (L-6): a reader that ends is said once, not fatal, and the reader left answers the pages.
// - F-4 (L-6): once no reader is left, that is said too, and the writer reads the next page.
// - F-5 (L-4): the pool's pages are recorded in the writer's statistics as `ProjectViewPage`.
//
// The roots are formal roots with the synthetic ledger (LN-1) where a test reads history. Every
// bounded read runs on a fake writer clock that moves a fixed step each time it is read; F-1's
// deadline passes on a writer clock the test moves; nothing sleeps. Only Q-3's reply limit is a
// real wait, of one second, for a read the test holds.

use super::*;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::reader_pool::ReaderPool;
use crate::global::{
    GlobalLedger, GlobalLedgerError, GlobalLedgerResult, LedgerFailurePlan, LedgerMaintenance,
    LedgerMonotonicClock, LedgerOpenTiming, LedgerWritePolicy, LedgerWriterCommandKind,
    Sha256SecretFingerprinter,
};
use crate::{
    SyntheticInstance, SyntheticLedgerSpec, WriterReadHoldGate, WriterStallGate,
    WriterStatementRecorder, write_synthetic_ledger,
};
use actingcommand_contract::{
    ArtifactId, AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventQuery,
    EventSeverity, EventSource, FactValue, IdentifierIssuer, InstanceId, LedgerPageLimit,
    LedgerView, OriginModule, OwnerEpoch, ProjectionProfile, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft,
    SanitizedEventDraft,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes) and the writer's owner id: neutral
// fixture words.
const STATE_KEY: &[u8] = b"ledger-qr1-fixture-key";
const OWNER: &str = "ledger-qr1-fixture";
// Q-1 and Q-4 read a small history; Q-3 the history LN-WS T2 stops inside a statement.
const SMALL_RECORDS: u64 = 600;
const BUDGET_RECORDS: u64 = 3_000;
// The first record's time (2030-01-01) and the time between two records.
const FIRST_MS: u64 = 1_893_456_000_000;
const RECORD_INTERVAL_MS: u64 = 3_000;
// The instances of the history's runs, in turn.
const INSTANCE_TURNS: &str = "abcadbac";
// The writer's write policy and the open's deadline, as LN-WS's.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
const OPEN_TIMEOUT_SECONDS: u64 = 600;
// Every caller waits this long for one reply: no real wait of these tests reaches it, but Q-3's.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);
const Q3_REPLY_TIMEOUT: Duration = Duration::from_secs(1);
// LN-WS T2's bound: the clock is looked at every 100 VM steps, a history statement spans at most
// 1 024 sequences, the budget runs out after 30 looks at a clock that moves 1 ms a look.
const BOUND_CHECK_VM_STEPS: u32 = 100;
const BOUND_SCAN_ROWS: usize = 1_024;
const Q3_BUDGET: Duration = Duration::from_millis(30);
const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
// Q-2: the objects the writer holds, one in this many in the live listing, and the chunk's bound.
const Q2_OBJECTS: usize = 40;
const Q2_LIVE_EVERY: usize = 4;
const Q2_CHUNK: usize = 8;
// Q-4: the appends that run while the pages are read.
const Q4_APPENDS: i64 = 30;
// F-1: the reply limit of the caller whose page waits past it, in writer clock milliseconds.
const F1_LATE_TIMEOUT_MS: u64 = 1_000;
// F-2: the pages queued behind the page in progress when the pool closes; F-3 and F-5: the pages
// read one after another.
const F_QUEUED: usize = 3;
const F_PAGES: usize = 3;

/// A writer clock in milliseconds that moves `step_ms` each time it is read.
fn stepping_clock(step_ms: u64) -> LedgerMonotonicClock {
    let now = Arc::new(AtomicU64::new(0));
    Arc::new(move || now.fetch_add(step_ms, Ordering::SeqCst))
}

/// A writer clock in milliseconds that only the test moves, through the returned counter.
fn manual_clock() -> (Arc<AtomicU64>, LedgerMonotonicClock) {
    let now = Arc::new(AtomicU64::new(0));
    let read = Arc::clone(&now);
    (now, Arc::new(move || read.load(Ordering::SeqCst)))
}

/// The view pages `recorder` recorded since it was last read.
fn pages_read(recorder: &WriterStatementRecorder) -> usize {
    recorder
        .take()
        .iter()
        .filter(|command| command.command == LedgerWriterCommandKind::ProjectViewPage)
        .count()
}

/// The writer's command just before an append of `value`, as the append observed it.
fn writer_work_before_append(
    ledger: &GlobalLedger,
    ids: &IdentifierIssuer,
    value: i64,
) -> Option<LedgerWriterCommandKind> {
    let (appended, observation) = ledger.append_with_observation(runtime_fact(ids, value));
    appended.expect("the append commits");
    observation
        .expect("the append's observation")
        .previous_writer_work
        .command
}

/// A view page of every record.
fn read_page(ledger: &GlobalLedger) -> GlobalLedgerResult<RuntimeEventQueryPage> {
    ledger.project_view_page(EventQuery::default(), PROFILE, first_page(10))
}

/// An error's code and operation.
fn named(error: &GlobalLedgerError) -> (&'static str, &'static str) {
    (error.code(), error.operation())
}

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

/// A formal root, closed, holding `records` synthetic records (none when 0).
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
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
        if records > 0 {
            let mut spec = SyntheticLedgerSpec::new(records, instances());
            spec.first_timestamp_unix_ms = FIRST_MS;
            spec.record_interval_ms = RECORD_INTERVAL_MS;
            let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
            assert_eq!(generated.records, records, "{generated:?}");
        }
        Self { dir, database }
    }

    /// The Runtime's writer of the root (a failure plan: it has a reader pool), its reads bounded
    /// by `budget` on its clock when one is given, its callers waiting `reply_timeout`.
    fn open(
        &self,
        budget: Option<(Duration, LedgerMonotonicClock)>,
        reply_timeout: Duration,
    ) -> GlobalLedger {
        let (budget, clock) = budget.unzip();
        self.open_with(budget, clock, None, reply_timeout)
    }

    /// `open`, its clock and its reader pool's size given apart (fix 1).
    fn open_with(
        &self,
        budget: Option<Duration>,
        clock: Option<LedgerMonotonicClock>,
        readers: Option<usize>,
        reply_timeout: Duration,
    ) -> GlobalLedger {
        let state_root = self.dir.path().canonicalize().expect("state root");
        let mut plan = LedgerFailurePlan::new(
            LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
                .expect("write policy"),
            &state_root,
            None,
            epoch(),
            FIRST_MS,
        )
        .with_reply_timeout(reply_timeout)
        .with_read_check_vm_steps(BOUND_CHECK_VM_STEPS)
        .with_history_scan_rows(BOUND_SCAN_ROWS);
        if let Some(budget) = budget {
            plan = plan.with_read_budget(budget);
        }
        if let Some(clock) = clock {
            plan = plan.with_clock(clock);
        }
        if let Some(readers) = readers {
            plan = plan.with_reader_threads_for_test(readers);
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
}

fn first_page(limit: u16) -> RuntimeEventQueryPageRequest {
    RuntimeEventQueryPageRequest::new(limit, None).expect("a first page")
}

/// A synchronous append as the policy driver makes one: a runtime fact the writer records.
fn runtime_fact(ids: &IdentifierIssuer, value: i64) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        FIRST_MS + RECORD_INTERVAL_MS * (BUDGET_RECORDS + 1),
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
                key: "backend.fixture_key_qr1".to_owned(),
                value: FactValue::Integer(value),
                observed_at_unix_ms: FIRST_MS,
                source: OriginModule::Runtime,
                ttl_ms: None,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"qr1-fixture-salt").expect("salt"))
    .expect("a fixture draft")
}

/// A page's bytes, as a client receives them.
fn bytes(page: &RuntimeEventQueryPage) -> Vec<u8> {
    serde_json::to_vec(page).expect("a page serializes")
}

/// Every row of `page` is at or before its snapshot, and the snapshot at or before `head`.
fn assert_within(page: &RuntimeEventQueryPage, head: u64, read: &str) {
    let snapshot = page.snapshot_ledger_position();
    assert!(
        snapshot <= head,
        "{read}: the snapshot {snapshot} is past the head {head}"
    );
    for event in page.events() {
        assert!(
            event.sequence <= snapshot,
            "{read}: a row at {} is past the snapshot {snapshot}",
            event.sequence
        );
    }
    let scope = page.read_scope().expect("a Runtime page has its scope");
    assert!(
        scope.scanned_through_position <= snapshot,
        "{read}: {scope:?} reads past the snapshot {snapshot}"
    );
}

#[test]
fn qr1_q1_a_page_is_answered_by_the_reader_pool_while_the_writer_is_held() {
    let root = Root::new(SMALL_RECORDS);
    let ledger = root.open(None, REPLY_TIMEOUT);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let reads = [
        (
            "the changes view",
            EventQuery {
                view: Some(LedgerView::Changes),
                ..EventQuery::default()
            },
            50,
        ),
        ("every record", EventQuery::default(), 100),
        (
            "one instance",
            EventQuery {
                instance_id: Some(instance('a')),
                ..EventQuery::default()
            },
            40,
        ),
    ];
    // The pages the writer reads itself (the path before the pool), the first and its next.
    let expected = reads
        .iter()
        .map(|(read, query, limit)| {
            let first = ledger
                .project_view_page_on_writer_for_test(query.clone(), PROFILE, first_page(*limit))
                .unwrap_or_else(|error| panic!("{read}: the writer's page: {error:?}"));
            let next = first.next_cursor().cloned().map(|cursor| {
                let request =
                    RuntimeEventQueryPageRequest::new(*limit, Some(cursor)).expect("a next page");
                let page = ledger
                    .project_view_page_on_writer_for_test(query.clone(), PROFILE, request.clone())
                    .unwrap_or_else(|error| panic!("{read}: the writer's next page: {error:?}"));
                (request, page)
            });
            (first, next)
        })
        .collect::<Vec<_>>();
    let head = ledger.latest_sequence().expect("head");
    let stall = WriterStallGate::arm(root.dir.path()).expect("writer stall gate");
    let writer = &ledger;
    let appended = thread::scope(|scope| {
        let draft = runtime_fact(&ids, 1);
        let appending = scope.spawn(move || writer.append(draft));
        assert!(
            stall.wait_held_commit(BARRIER_WAIT),
            "the writer is held inside the append's write transaction"
        );
        for ((read, query, limit), (first, next)) in reads.iter().zip(&expected) {
            let page = writer
                .project_view_page(query.clone(), PROFILE, first_page(*limit))
                .unwrap_or_else(|error| panic!("{read}: the pool's page: {error:?}"));
            assert_eq!(page.snapshot_ledger_position(), head, "{read}");
            assert_eq!(
                bytes(&page),
                bytes(first),
                "{read}: the pool's page is the writer's, byte for byte"
            );
            if let Some((request, expected_next)) = next {
                let page = writer
                    .project_view_page(query.clone(), PROFILE, request.clone())
                    .unwrap_or_else(|error| panic!("{read}: the pool's next page: {error:?}"));
                assert_eq!(
                    bytes(&page),
                    bytes(expected_next),
                    "{read}: the pool's next page is the writer's, byte for byte"
                );
            }
        }
        stall.release();
        appending.join().expect("the append's thread")
    });
    let appended = appended.expect("the held append commits once released");
    assert_eq!(appended.sequence(), head + 1);
    ledger.close().expect("close");
}

#[test]
fn qr1_q2_a_pass_start_trim_reaches_the_writer_in_bounded_chunks_with_work_between_them() {
    let root = Root::new(0);
    let ledger = root.open(None, REPLY_TIMEOUT);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let objects = ledger
        .seed_frame_objects_for_test(Q2_OBJECTS)
        .expect("the writer holds the frame objects");
    assert_eq!(objects.len(), Q2_OBJECTS);
    let live = Arc::new(
        objects
            .iter()
            .step_by(Q2_LIVE_EVERY)
            .copied()
            .collect::<BTreeSet<ArtifactId>>(),
    );
    let mut cursor = None;
    let mut chunks = 0_usize;
    let mut looked = 0_usize;
    let mut dropped = 0_usize;
    let mut appended = 0_i64;
    loop {
        let chunk = ledger
            .retain_live_frames_chunk(&live, cursor, Q2_CHUNK)
            .expect("one chunk of the trim");
        chunks += 1;
        assert!(
            chunk.looked <= Q2_CHUNK,
            "a chunk looked at {} objects, more than its bound {Q2_CHUNK}",
            chunk.looked
        );
        looked += chunk.looked;
        dropped += chunk.dropped;
        let Some(next) = chunk.next else {
            break;
        };
        cursor = Some(next);
        // Between two chunks the writer takes an append and the pool a page, and both answer.
        appended += 1;
        let event = ledger
            .append(runtime_fact(&ids, appended))
            .expect("an append between two chunks commits");
        let page = ledger
            .project_view_page(
                EventQuery {
                    from_sequence: Some(event.sequence()),
                    ..EventQuery::default()
                },
                PROFILE,
                first_page(10),
            )
            .expect("a page between two chunks answers");
        assert_eq!(page.snapshot_ledger_position(), event.sequence());
        assert!(
            page.events()
                .iter()
                .any(|row| row.event_id == *event.event_id()),
            "the page reads the append committed just before it"
        );
    }
    assert!(
        chunks >= Q2_OBJECTS.div_ceil(Q2_CHUNK),
        "{chunks} chunks for {Q2_OBJECTS} objects of at most {Q2_CHUNK}"
    );
    assert!(appended > 0, "work ran between the chunks");
    assert_eq!(looked, Q2_OBJECTS, "every object was looked at once");
    assert_eq!(
        dropped,
        Q2_OBJECTS - live.len(),
        "the objects outside the listing were dropped"
    );
    // A second trim finds only the listing's objects.
    let again = ledger
        .retain_live_frames_chunk(&live, None, Q2_OBJECTS)
        .expect("a second trim");
    assert_eq!(
        (again.looked, again.dropped, again.next),
        (live.len(), 0, None)
    );
    ledger.close().expect("close");
}

#[test]
fn qr1_q3_the_read_budget_and_the_reply_limit_still_stop_a_reader_pool_read() {
    // The budget: a page that reads past its budget answers incomplete there, read by the pool
    // while the writer is held, as LN-WS T2's page on the writer.
    let root = Root::new(BUDGET_RECORDS);
    let ledger = root.open(Some((Q3_BUDGET, stepping_clock(1))), REPLY_TIMEOUT);
    let head = ledger.latest_sequence().expect("head");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    // No record names the fact key, which has no column: the page's selection looks at every
    // record until its budget stops it (#411 A51 T2's read).
    let query = EventQuery {
        view: Some(LedgerView::Changes),
        fact_key: Some("backend.fixture_key_qr1_absent".to_owned()),
        ..EventQuery::default()
    };
    let stall = WriterStallGate::arm(root.dir.path()).expect("writer stall gate");
    let writer = &ledger;
    let page = thread::scope(|scope| {
        let draft = runtime_fact(&ids, 1);
        let appending = scope.spawn(move || writer.append(draft));
        assert!(
            stall.wait_held_commit(BARRIER_WAIT),
            "the writer is held inside the append's write transaction"
        );
        let page = writer.project_view_page(query, PROFILE, first_page(100));
        stall.release();
        appending
            .join()
            .expect("the append's thread")
            .expect("the held append commits once released");
        page
    })
    .expect("a page stopped at its budget still answers");
    let scope = page.read_scope().expect("a Runtime page has its scope");
    assert!(
        !scope.read_complete && scope.limits.contains(&LedgerPageLimit::ReadBudget),
        "the budget stopped the pool's read: {scope:?}"
    );
    let cursor = page.next_cursor().expect("a page that continues");
    assert!(
        cursor.after_sequence() > 0 && cursor.after_sequence() < head,
        "the cursor {} is past the page's start and before the head {head}",
        cursor.after_sequence()
    );
    ledger.close().expect("close");

    // The reply limit: a pool page held at its start is not answered in time; its caller gets
    // `writer_response_timeout`, not fatal, and the next page answers once the hold is gone.
    let root = Root::new(0);
    let ledger = root.open(None, Q3_REPLY_TIMEOUT);
    let hold = WriterReadHoldGate::arm(root.dir.path()).expect("read hold");
    let error = ledger
        .project_view_page(EventQuery::default(), PROFILE, first_page(10))
        .expect_err("a page held at the pool is not answered in time");
    assert!(hold.wait_held(BARRIER_WAIT), "the pool's read was held");
    assert_eq!(
        error.code(),
        LedgerCode::WriterResponseTimeout.as_str(),
        "{error:?}"
    );
    assert!(!error.is_fatal(), "{error:?}");
    hold.release();
    ledger
        .project_view_page(EventQuery::default(), PROFILE, first_page(10))
        .expect("the next page answers");
    ledger.close().expect("close");
}

#[test]
fn qr1_q4_a_page_never_reads_past_the_committed_head_while_the_writer_appends() {
    let root = Root::new(SMALL_RECORDS);
    let ledger = root.open(None, REPLY_TIMEOUT);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let head = ledger.latest_sequence().expect("head");
    let tail = EventQuery {
        from_sequence: Some(head.saturating_sub(4)),
        ..EventQuery::default()
    };
    // A record written inside a write transaction that has not committed is not read.
    let stall = WriterStallGate::arm(root.dir.path()).expect("writer stall gate");
    let writer = &ledger;
    let appended = thread::scope(|scope| {
        let draft = runtime_fact(&ids, 1);
        let appending = scope.spawn(move || writer.append(draft));
        assert!(
            stall.wait_held_commit(BARRIER_WAIT),
            "the writer is held inside the append's write transaction"
        );
        let page = writer
            .project_view_page(tail.clone(), PROFILE, first_page(100))
            .expect("the pool's page");
        assert_eq!(page.snapshot_ledger_position(), head);
        assert_within(&page, head, "the held append");
        stall.release();
        appending.join().expect("the append's thread")
    })
    .expect("the held append commits once released");
    assert_eq!(appended.sequence(), head + 1);
    // Once it committed, the next page reads it.
    let page = ledger
        .project_view_page(tail.clone(), PROFILE, first_page(100))
        .expect("the pool's page");
    assert_eq!(page.snapshot_ledger_position(), head + 1);
    assert_eq!(
        page.events().last().map(|row| row.event_id),
        Some(*appended.event_id()),
        "the committed append is the page's last row"
    );
    // Pages read while the writer appends: each is at or before the head read after it.
    let drafts = (0..Q4_APPENDS)
        .map(|value| runtime_fact(&ids, 2 + value))
        .collect::<Vec<_>>();
    thread::scope(|scope| {
        let appending = scope.spawn(move || {
            for draft in drafts {
                writer
                    .append(draft)
                    .expect("an append while pages are read");
            }
        });
        for read in 0..Q4_APPENDS {
            let page = writer
                .project_view_page(tail.clone(), PROFILE, first_page(100))
                .expect("a page while the writer appends");
            let committed = writer.latest_sequence().expect("the head after the page");
            assert_within(&page, committed, &format!("page {read}"));
        }
        appending.join().expect("the appends' thread");
    });
    ledger.close().expect("close");
}

#[test]
fn qr1_f1_a_page_past_its_callers_deadline_is_answered_and_never_read() {
    let root = Root::new(0);
    let (now, clock) = manual_clock();
    let ledger = root.open_with(None, Some(clock), Some(1), REPLY_TIMEOUT);
    let pool = ledger.reader_pool.as_ref().expect("the reader pool");
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("statement recorder");
    let hold = WriterReadHoldGate::arm(root.dir.path()).expect("read hold");
    // The pool's one reader is held in a page; a second page waits in the queue behind it.
    let held = pool
        .submit(
            EventQuery::default(),
            PROFILE,
            first_page(10),
            REPLY_TIMEOUT,
        )
        .expect("the first page is queued");
    assert!(
        hold.wait_held(BARRIER_WAIT),
        "the reader is held in the first page"
    );
    let late = pool
        .submit(
            EventQuery::default(),
            PROFILE,
            first_page(10),
            Duration::from_millis(F1_LATE_TIMEOUT_MS),
        )
        .expect("the second page is queued");
    // The second page's caller stops waiting while its page is still queued.
    now.fetch_add(F1_LATE_TIMEOUT_MS, Ordering::SeqCst);
    hold.release();
    let (page, _) = ReaderPool::receive(&held, BARRIER_WAIT).expect("the held page is answered");
    page.expect("the held page is read");
    let (late, observation) =
        ReaderPool::receive(&late, BARRIER_WAIT).expect("the late page is answered");
    let error = late.expect_err("the late page is not read");
    assert_eq!(
        named(&error),
        (
            LedgerCode::WriterResponseTimeout.as_str(),
            LedgerLocation::ProjectLedgerViewPage.as_str()
        ),
        "the late page has its caller's timeout: {error:?}"
    );
    assert!(!error.is_fatal(), "{error:?}");
    assert!(observation.is_none(), "nothing was read for the late page");
    assert_eq!(pages_read(&recorder), 1, "the pool read the held page only");
    ledger.close().expect("close");
}

#[test]
fn qr1_f1_a_close_answers_the_queued_pages_as_refused_and_reads_none() {
    let root = Root::new(0);
    let mut ledger = root.open_with(None, None, Some(1), REPLY_TIMEOUT);
    let mut pool = ledger.reader_pool.take().expect("the reader pool");
    let recorder = WriterStatementRecorder::arm(root.dir.path()).expect("statement recorder");
    let hold = WriterReadHoldGate::arm(root.dir.path()).expect("read hold");
    // The pool's one reader is held in a page; more pages wait in the queue behind it.
    let in_progress = pool
        .submit(
            EventQuery::default(),
            PROFILE,
            first_page(10),
            REPLY_TIMEOUT,
        )
        .expect("the first page is queued");
    assert!(
        hold.wait_held(BARRIER_WAIT),
        "the reader is held in the first page"
    );
    let queued = (0..F_QUEUED)
        .map(|_| {
            pool.submit(
                EventQuery::default(),
                PROFILE,
                first_page(10),
                REPLY_TIMEOUT,
            )
            .expect("a page is queued")
        })
        .collect::<Vec<_>>();
    // The close's first step, then the page in progress goes on, then the close ends.
    pool.stop();
    hold.release();
    pool.close();
    let (_in_progress, _) = in_progress
        .try_recv()
        .expect("the page in progress was answered before the close returned");
    for (index, receiver) in queued.iter().enumerate() {
        let (reply, observation) = receiver
            .try_recv()
            .unwrap_or_else(|error| panic!("queued page {index} was not answered: {error:?}"));
        let error = reply.expect_err("a queued page is refused");
        assert_eq!(
            named(&error),
            (
                LedgerCode::WriterUnavailable.as_str(),
                LedgerLocation::ShutdownWriter.as_str()
            ),
            "queued page {index}: {error:?}"
        );
        assert!(!error.is_fatal(), "queued page {index}: {error:?}");
        assert!(observation.is_none(), "nothing was read for page {index}");
    }
    assert_eq!(
        pages_read(&recorder),
        1,
        "the close read none of the queued pages"
    );
    drop(pool);
    ledger.close().expect("close");
}

#[test]
fn qr1_f1_a_reader_that_ends_is_said_once_and_the_reader_left_answers_the_pages() {
    let root = Root::new(0);
    let ledger = root.open(None, REPLY_TIMEOUT);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    ledger
        .reader_pool
        .as_ref()
        .expect("the reader pool")
        .end_readers_for_test(1);
    let error = read_page(&ledger).expect_err("the page whose reader ended");
    assert_eq!(
        named(&error),
        (
            LedgerCode::WriterPanicked.as_str(),
            LedgerLocation::ProjectLedgerViewPage.as_str()
        ),
        "{error:?}"
    );
    assert!(!error.is_fatal(), "a reader's end is not fatal: {error:?}");
    let findings = ledger.take_reader_pool_findings();
    assert_eq!(
        findings.iter().map(named).collect::<Vec<_>>(),
        vec![(
            LedgerCode::WriterPanicked.as_str(),
            LedgerLocation::QueryLedgerView.as_str()
        )],
        "one warning for the reader's end: {findings:?}"
    );
    assert!(findings.iter().all(|finding| !finding.is_fatal()));
    assert!(ledger.reads_off_writer(), "a reader is left");
    for read in 0..F_PAGES {
        read_page(&ledger).unwrap_or_else(|error| panic!("page {read}: {error:?}"));
    }
    assert!(
        ledger.take_reader_pool_findings().is_empty(),
        "the reader's end is said once"
    );
    assert_ne!(
        writer_work_before_append(&ledger, &ids, 1),
        Some(LedgerWriterCommandKind::ProjectViewPage),
        "the reader left read the pages, not the writer"
    );
    ledger.close().expect("close");
}

#[test]
fn qr1_f1_once_no_reader_is_left_the_writer_reads_the_pages_with_a_warning() {
    let root = Root::new(0);
    let ledger = root.open(None, REPLY_TIMEOUT);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    ledger
        .reader_pool
        .as_ref()
        .expect("the reader pool")
        .end_readers_for_test(2);
    for reader in 0..2 {
        let error = read_page(&ledger).expect_err("the page whose reader ended");
        assert_eq!(
            named(&error),
            (
                LedgerCode::WriterPanicked.as_str(),
                LedgerLocation::ProjectLedgerViewPage.as_str()
            ),
            "reader {reader}: {error:?}"
        );
        assert!(!error.is_fatal(), "reader {reader}: {error:?}");
    }
    let findings = ledger.take_reader_pool_findings();
    assert_eq!(
        findings.iter().map(named).collect::<Vec<_>>(),
        vec![
            (
                LedgerCode::WriterPanicked.as_str(),
                LedgerLocation::QueryLedgerView.as_str()
            ),
            (
                LedgerCode::WriterPanicked.as_str(),
                LedgerLocation::ProjectLedgerViewPage.as_str()
            ),
        ],
        "the first end and the fallback to the writer are said: {findings:?}"
    );
    assert!(findings.iter().all(|finding| !finding.is_fatal()));
    assert!(!ledger.reads_off_writer(), "no reader is left");
    read_page(&ledger).expect("the writer answers the next page");
    assert_eq!(
        writer_work_before_append(&ledger, &ids, 1),
        Some(LedgerWriterCommandKind::ProjectViewPage),
        "the writer read the page"
    );
    assert!(ledger.take_reader_pool_findings().is_empty());
    ledger.close().expect("close");
}

#[test]
fn qr1_f1_the_pool_pages_are_recorded_in_the_writer_statistics() {
    let root = Root::new(0);
    let ledger = root.open(None, REPLY_TIMEOUT);
    // A new window opens; only the pool's pages run in it.
    ledger.take_writer_peak();
    for read in 0..F_PAGES {
        read_page(&ledger).unwrap_or_else(|error| panic!("page {read}: {error:?}"));
    }
    let peak = ledger.take_writer_peak();
    assert_eq!(
        peak.commands,
        u64::try_from(F_PAGES).expect("a page count"),
        "every pool page is recorded: {peak:?}"
    );
    assert_eq!(
        peak.longest.map(|(command, _)| command),
        Some(LedgerWriterCommandKind::ProjectViewPage),
        "the window's longest work is a pool page, with its time: {peak:?}"
    );
    ledger.close().expect("close");
}
