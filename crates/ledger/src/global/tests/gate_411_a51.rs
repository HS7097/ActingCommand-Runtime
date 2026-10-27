// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A51 (0.12.9; the 10-21 incident: one `view: lab` read of one instance's window ran as
//! one SQLite statement on the writer thread for minutes, nothing committed, the reply timeouts
//! latched fatal and the close waited for the writer without a bound). Requirements of the slice
//! (brief A51, frozen by the coordinator 10-21 19:3x), the ledger's part:
//!
//! - T1 (R1): a Runtime read of the Lab view selects its rows from the writer's per-link-group
//!   facts, never with a per-row correlated sub-select: the page is the in-memory projection's
//!   (`EventIndexes::project_view_page`) at every snapshot, and the selection's work stays within
//!   200 per candidate or anchor;
//! - T2 (R2): a read that its work budget stops answers an incomplete page (`read_complete`
//!   false, the `read_budget` limit, a `scanned_through_position` below the head) whose cursor
//!   continues from that position, and an append that waited behind it commits before the next
//!   page is read;
//! - T4 (R4): a close whose writer is held in a read waits at most its bound and then ends with
//!   `ledger_close_timeout`, naming the read the writer is in.
//!
//! The budget and the close bound are driven by a test clock that moves a fixed step each time
//! it is read; the writer is held in a read by the read hold gate, which the test releases.
//! `BARRIER_WAIT` only bounds a broken run. Existing tests missed the incident: their Lab-view
//! fixtures hold a handful of events and none bounds a view's work against its anchors, no
//! writer read had a budget, and every close test closed an idle writer.

use super::*;
use crate::WriterReadHoldGate;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::projection::{EventIndexes, PageSelection};
use crate::global::{
    GlobalLedger, GlobalLedgerConfig, GlobalLedgerError, LedgerMonotonicClock,
    LedgerProjectViewCount, PersistedEventRef, Sha256SecretFingerprinter,
};
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, EventAction, EventActor, EventDraft,
    EventId, EventLinksDraft, EventOrigin, EventPayloadDraft, EventQuery, EventSeverity,
    EventSource, IdentifierIssuer, LedgerMaterialReadState, LedgerPageLimit, LedgerReadScope,
    LedgerReadSource, LedgerView, OriginModule, ProjectionProfile, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, SanitizedEventDraft,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;

/// The window of the instance the Lab page is read for: its first millisecond.
const WINDOW_START_MS: u64 = 1_792_569_600_000;
/// T1's fixture: Lab anchors, each with this many linked events on another instance.
const ANCHORS: usize = 40;
const LINKED_PER_ANCHOR: usize = 30;
/// T1's fixture: the scheduled run in the instance's window (one request, correlation and run
/// no anchor names) and the instance's three Lab-related events.
const RUN_EVENTS: usize = 300;
const LAB_RELATED: usize = 3;
/// T1: the selection's work per candidate or anchor that a Lab page may take at most.
const WORK_PER_ROW: u64 = 200;
/// The Lab page's limit, as `ac_events` asks.
const PAGE_LIMIT: u16 = 40;
/// The bounded waits' duration (test plan v2.2 §2.1 guard 3); it only bounds a broken run.
const GATE_WAIT: Duration = Duration::from_secs(30);

/// A test clock in milliseconds that moves `step_ms` each time it is read.
fn stepping_clock(step_ms: u64) -> LedgerMonotonicClock {
    let now = Arc::new(AtomicU64::new(0));
    Arc::new(move || now.fetch_add(step_ms, Ordering::SeqCst))
}

fn sanitized(
    ids: &IdentifierIssuer,
    at_unix_ms: u64,
    origin: EventOrigin,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        at_unix_ms,
        EventSeverity::Info,
        origin,
        links,
        payload,
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"a51-fixture-salt").expect("salt"))
    .expect("a fixture draft")
}

/// An ordinary record: a command the CLI sent.
fn command(ids: &IdentifierIssuer, at_unix_ms: u64, links: EventLinksDraft) -> SanitizedEventDraft {
    sanitized(
        ids,
        at_unix_ms,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        links,
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
}

/// A Lab anchor: a request the Lab sent.
fn lab_anchor(
    ids: &IdentifierIssuer,
    at_unix_ms: u64,
    links: EventLinksDraft,
) -> SanitizedEventDraft {
    sanitized(
        ids,
        at_unix_ms,
        EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
        links,
        ClientPayloadDraft::lab_request(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
    )
}

/// Commits `drafts` in order through the capture queue and returns their committed references.
fn commit(ledger: &GlobalLedger, drafts: Vec<SanitizedEventDraft>) -> Vec<PersistedEventRef> {
    for draft in drafts {
        ledger.append_deferred(draft).expect("accepted");
    }
    let summary = ledger.confirm_deferred(BARRIER_WAIT).expect("confirmation");
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    assert_eq!(summary.pending, 0, "every record committed");
    summary.committed
}

fn sequence_of(committed: &[PersistedEventRef], event_id: &EventId) -> u64 {
    committed
        .iter()
        .find(|reference| reference.event_id() == event_id)
        .expect("a committed fixture record")
        .sequence()
}

/// T1's ledger. 39 Lab anchors, each with 30 linked events (same request, correlation and run)
/// on another instance; then the instance's window: a scheduled run of 300 events with the
/// instance's three Lab-related events among them (one names anchor 0's request, one anchor 1's
/// correlation, one runs in anchor 39's run); then anchor 39 with its 30 linked events. The
/// third related event belongs to the Lab view only from anchor 39 on.
struct LabFixture {
    _root: TempDir,
    ledger: GlobalLedger,
    query: EventQuery,
    /// The three related events in sequence order.
    related: Vec<EventId>,
    /// Anchor 39's sequence.
    late_anchor: u64,
}

fn lab_fixture() -> LabFixture {
    let root = TempDir::new().expect("state root");
    let ledger = sqlite_contract::open(GlobalLedgerConfig::new(root.path(), "a51-lab-view"))
        .expect("a SQLite ledger");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let other = ids.mint_instance_id().expect("other instance");
    let instance = ids.mint_instance_id().expect("the read's instance");
    // Each anchor's request, correlation and run, which its linked events share.
    let groups = (0..ANCHORS)
        .map(|_| {
            (
                ids.mint_request_id().expect("anchor request"),
                ids.mint_correlation_id().expect("anchor correlation"),
                ids.mint_run_id().expect("anchor run"),
            )
        })
        .collect::<Vec<_>>();
    let anchor_group = |at_unix_ms: u64, index: usize| {
        let (request, correlation, run) = groups[index];
        let links = EventLinksDraft::default()
            .with_instance_id(other)
            .with_request_id(request)
            .with_correlation_id(correlation)
            .with_run_id(run);
        let mut drafts = vec![lab_anchor(&ids, at_unix_ms, links.clone())];
        drafts.extend((0..LINKED_PER_ANCHOR).map(|_| command(&ids, at_unix_ms, links.clone())));
        drafts
    };
    let mut drafts = Vec::new();
    for index in 0..ANCHORS - 1 {
        drafts.extend(anchor_group(WINDOW_START_MS - 60_000 + index as u64, index));
    }
    let run_links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(ids.mint_request_id().expect("run request"))
        .with_correlation_id(ids.mint_correlation_id().expect("run correlation"))
        .with_run_id(ids.mint_run_id().expect("run"));
    let related_drafts = vec![
        // Anchor 0's request.
        command(
            &ids,
            WINDOW_START_MS + 100,
            EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(groups[0].0),
        ),
        // Anchor 1's correlation.
        command(
            &ids,
            WINDOW_START_MS + 200,
            EventLinksDraft::default()
                .with_instance_id(instance)
                .with_correlation_id(groups[1].1),
        ),
        // Anchor 39's run, under a request no anchor names.
        command(
            &ids,
            WINDOW_START_MS + 299,
            EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(ids.mint_request_id().expect("unanchored request"))
                .with_run_id(groups[ANCHORS - 1].2),
        ),
    ];
    let related = related_drafts
        .iter()
        .map(|draft| *draft.event_id())
        .collect::<Vec<_>>();
    let mut related_drafts = related_drafts.into_iter();
    for offset in 0..RUN_EVENTS as u64 {
        drafts.push(command(&ids, WINDOW_START_MS + offset, run_links.clone()));
        if offset == 100 || offset == 200 || offset == 299 {
            drafts.push(related_drafts.next().expect("a related event"));
        }
    }
    let late_anchor_id = {
        let late = anchor_group(WINDOW_START_MS + 1_000, ANCHORS - 1);
        let id = *late[0].event_id();
        drafts.extend(late);
        id
    };
    let committed = commit(&ledger, drafts);
    assert_eq!(
        committed.len(),
        ANCHORS * (1 + LINKED_PER_ANCHOR) + RUN_EVENTS + LAB_RELATED
    );
    let late_anchor = sequence_of(&committed, &late_anchor_id);
    let query = EventQuery {
        view: Some(LedgerView::Lab),
        instance_id: Some(*instance.transport()),
        from_timestamp_unix_ms: Some(WINDOW_START_MS),
        ..EventQuery::default()
    };
    LabFixture {
        _root: root,
        ledger,
        query,
        related,
        late_anchor,
    }
}

/// The in-memory projection of the same events at `snapshot`: the Lab view's definition.
fn reference_page(
    ledger: &GlobalLedger,
    query: &EventQuery,
    request: &RuntimeEventQueryPageRequest,
) -> RuntimeEventQueryPage {
    let events = ledger.query(EventQuery::default()).expect("every event");
    let head = events.last().map_or(0, PersistedEvent::sequence);
    EventIndexes::from_events(&events)
        .project_view_page(
            &events,
            query,
            ProjectionProfile::Concise,
            request,
            LedgerReadScope {
                source: LedgerReadSource::Runtime,
                material_read: LedgerMaterialReadState::NotRequested,
                scanned_through_position: head,
                read_complete: true,
                limits: Vec::new(),
            },
            PageSelection::from(head),
        )
        .expect("the in-memory page")
}

#[test]
fn a51_t1_a_lab_page_is_the_in_memory_projection_and_its_work_is_bounded_by_its_candidates() {
    let fixture = lab_fixture();
    let candidates = (RUN_EVENTS + LAB_RELATED) as u64;
    let bound = WORK_PER_ROW * (candidates + ANCHORS as u64);
    let head = fixture.ledger.latest_sequence().expect("head");
    // Before anchor 39 the third related event is not yet in the Lab view; at the head it is.
    for (snapshot, expected) in [
        (fixture.late_anchor - 1, &fixture.related[..2]),
        (head, &fixture.related[..]),
    ] {
        let request = RuntimeEventQueryPageRequest::new(PAGE_LIMIT, None)
            .and_then(|request| request.at_snapshot(snapshot))
            .expect("a page request");
        let (page, observation) = fixture
            .ledger
            .project_view_page_with_observation(
                fixture.query.clone(),
                ProjectionProfile::Concise,
                request.clone(),
            )
            .expect("the Lab page");
        assert_eq!(
            page.events()
                .iter()
                .map(|event| event.event_id)
                .collect::<Vec<_>>(),
            expected,
            "the Lab-related events of the instance's window at {snapshot}"
        );
        assert_eq!(
            page,
            reference_page(&fixture.ledger, &fixture.query, &request),
            "the in-memory projection at {snapshot}"
        );
        let work = match observation
            .expect("a SQLite read is observed")
            .selection_work
        {
            LedgerProjectViewCount::Observed(work) => work,
            other => panic!("the selection's work is unobserved: {other:?}"),
        };
        assert!(
            work <= bound,
            "the selection took {work} steps, more than {bound} for {candidates} candidates and \
             {ANCHORS} anchors"
        );
    }
    fixture.ledger.close().expect("close");
}

#[test]
fn a51_t2_a_read_stopped_by_its_budget_answers_an_incomplete_page_that_continues() {
    const EVENTS: u64 = 2_000;
    let root = TempDir::new().expect("state root");
    // Every look at the clock moves it 400 ms: the read stops at its third check, after three
    // times `READ_BUDGET_CHECK_ROWS` rows.
    let ledger = sqlite_contract::open(
        GlobalLedgerConfig::new(root.path(), "a51-read-budget")
            .with_read_budget(Duration::from_millis(1_000))
            .with_clock(stepping_clock(400)),
    )
    .expect("a SQLite ledger");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    commit(
        &ledger,
        (0..EVENTS)
            .map(|offset| command(&ids, WINDOW_START_MS + offset, EventLinksDraft::default()))
            .collect(),
    );
    let head = ledger.latest_sequence().expect("head");
    // No record names the fact key, which has no column (LN-6: a view's condition selects
    // through the store's indexes): the read has to look at every record.
    let query = EventQuery {
        view: Some(LedgerView::Changes),
        fact_key: Some("backend.ready".to_owned()),
        ..EventQuery::default()
    };
    let request = RuntimeEventQueryPageRequest::new(PAGE_LIMIT, None).expect("first page");
    let hold = WriterReadHoldGate::arm(root.path()).expect("read hold");
    let first = std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            ledger.project_view_page(query.clone(), ProjectionProfile::Concise, request.clone())
        });
        assert!(
            hold.wait_held(BARRIER_WAIT),
            "the page read reached the writer"
        );
        // An append while the writer is in the read waits behind it.
        ledger
            .append_deferred(command(
                &ids,
                WINDOW_START_MS + EVENTS,
                EventLinksDraft::default(),
            ))
            .expect("accepted");
        hold.release();
        reader.join().expect("reader")
    })
    .expect("the first page");
    let scope = first.read_scope().expect("a Runtime page has its scope");
    assert!(
        !scope.read_complete,
        "the budget stopped the read: {first:?}"
    );
    assert!(
        scope.limits.contains(&LedgerPageLimit::ReadBudget),
        "{scope:?}"
    );
    assert!(scope.scanned_through_position < head, "{scope:?}");
    assert!(first.events().is_empty(), "no record names the fact key");
    assert!(first.has_more());
    let cursor = first.next_cursor().expect("a page that continues").clone();
    assert_eq!(cursor.after_sequence(), scope.scanned_through_position);
    // The append that waited behind the read committed before any later read.
    let appended = ledger.confirm_deferred(BARRIER_WAIT).expect("confirmation");
    assert_eq!(appended.committed.len(), 1, "{appended:?}");
    assert_eq!(appended.committed[0].sequence(), head + 1);
    // The next page goes on from where the first one stopped, at the same snapshot.
    let second = ledger
        .project_view_page(
            query,
            ProjectionProfile::Concise,
            RuntimeEventQueryPageRequest::new(PAGE_LIMIT, Some(cursor)).expect("next page"),
        )
        .expect("the second page");
    assert_eq!(
        second.snapshot_ledger_position(),
        first.snapshot_ledger_position()
    );
    assert!(
        second.read_scope().expect("scope").scanned_through_position
            > scope.scanned_through_position,
        "{second:?}"
    );
    ledger.close().expect("close");
}

#[test]
fn a51_t4_a_close_whose_writer_is_held_in_a_read_ends_at_its_bound_naming_the_read() {
    const CLOSE_BOUND: Duration = Duration::from_millis(5_000);
    let root = TempDir::new().expect("state root");
    // Every look at the clock moves it one second: the close's bound passes after five of its
    // looks, whatever the wall clock does.
    let ledger = sqlite_contract::open(
        GlobalLedgerConfig::new(root.path(), "a51-close-bound")
            .with_close_timeout(CLOSE_BOUND)
            .with_clock(stepping_clock(1_000)),
    )
    .expect("a SQLite ledger");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    commit(
        &ledger,
        vec![command(&ids, WINDOW_START_MS, EventLinksDraft::default())],
    );
    let hold = WriterReadHoldGate::arm(root.path()).expect("read hold");
    let held = ledger
        .query_unawaited_for_test(EventQuery::default())
        .expect("a read queued");
    assert!(hold.wait_held(BARRIER_WAIT), "the writer is in the read");
    let (closed_sender, closed) = mpsc::channel();
    thread::spawn(move || {
        let _ = closed_sender.send(ledger.close());
    });
    let first = closed.recv_timeout(GATE_WAIT);
    hold.release();
    let result = match first {
        Ok(result) => result,
        Err(_) => closed
            .recv_timeout(GATE_WAIT)
            .expect("the close returns once the read ends"),
    };
    let error = result.err();
    assert_eq!(
        error.as_ref().map(GlobalLedgerError::code),
        Some(LedgerCode::LedgerCloseTimeout.as_str()),
        "the close ends at its bound"
    );
    let error = error.expect("the close timed out");
    assert!(error.is_fatal(), "the writer is left behind");
    let outcome = error.outcome_values().expect("the close's values");
    assert_eq!(
        outcome.value("stage"),
        Some(&json!(LedgerLocation::QueryEvents.as_str())),
        "the read the writer is in"
    );
    assert_eq!(outcome.value("timeout_ms"), Some(&json!(5_000)));
    assert!(
        outcome
            .value("elapsed_ms")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|elapsed| elapsed >= 5_000),
        "{outcome:?}"
    );
    drop(held);
}
