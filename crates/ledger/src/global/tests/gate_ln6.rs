// SPDX-License-Identifier: AGPL-3.0-only

//! LN-6 (#422, LEAN-L §3.3 and §5 LN-6; brief rd-1026/brief-ln6.md): the Runtime's view pages and
//! artifact resolution read the store. The writer keeps no verified prefix of the history: a view
//! page selects its rows through the indexes and its stored view's condition (the Lab view through
//! its relations) and reads each row on its own; an artifact's event is one authenticated point
//! read. The view tests run on a SQLite store whose history pages hold two rows
//! (`history_page_rows`), so every page spans several statements:
//!
//! - T1: each of the six views, page by page, returns the rows its stored view
//!   (`ledger_view_*_v1`) selects at the page's snapshot, each row naming the views whose stored
//!   view selects it, and each page is the in-memory projection's of the same records; again after
//!   the writer is opened anew, when the records are only on disk;
//! - T2: the Lab view relates a record through the request or the correlation a Lab anchor names,
//!   directly or through the record's run, from the anchor's position on;
//! - T3: `resolve_artifact` answers an artifact without an eviction proof and one with its intent
//!   and outcome, and refuses a position the ledger does not hold;
//! - T4: a view page that its work budget stops answers incomplete, and its cursors read the
//!   whole selection.
//!
//! Every record time is a fixed offset; T4's budget runs on a clock that moves a fixed step each
//! time it is read. `GATE_WAIT` only bounds a broken run.

use super::missing_material::{Capture, capture, formal_root, material_store};
use super::*;
use crate::codes::LedgerCode;
use crate::global::projection::{EventIndexes, PageSelection};
use crate::global::{
    ArtifactEvictionAdmission, GlobalLedger, GlobalLedgerConfig, LedgerArtifactSelection,
    LedgerMonotonicClock, Sha256SecretFingerprinter,
};
use actingcommand_artifact_store::try_artifact_delete_guard;
use actingcommand_contract::{
    AuditInput, CapturePayloadDraft, ClientPayloadDraft, CommandPayloadDraft, CorrelationId,
    DiagnosticCode, EffectDisposition, EventAction, EventActor, EventDraft, EventId,
    EventLinksDraft, EventOrigin, EventPayloadDraft, EventQuery, EventSeverity, EventSource,
    FactValue, IdentifierIssuer, LedgerEventPosition, LedgerMaterialReadState, LedgerPageLimit,
    LedgerReadScope, LedgerReadSource, LedgerView, MonitorPayloadDraft, OriginModule,
    ProjectedEvent, ProjectionProfile, RequestId, RunId, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft,
    SanitizedEventDraft, TerminalEvent,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;

/// Test plan v2.2 §2.1 guard 3: the one bound of a wait in this file.
const GATE_WAIT: Duration = Duration::from_secs(30);
const AT_MS: u64 = 1_752_147_200_000;
/// The rows of one history page in these tests.
const ROWS: usize = 2;
/// The events of one view page in these tests.
const PAGE: u16 = 2;
/// The fact key T4's record names.
const FACT_KEY: &str = "backend.ready";

fn salt() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"ln6-fixture-salt").expect("salt")
}

fn draft(
    ids: &IdentifierIssuer,
    offset: u64,
    origin: EventOrigin,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS + offset,
        EventSeverity::Info,
        origin,
        links,
        payload,
    )
    .sanitize(&salt())
    .expect("a fixture draft")
}

fn cli() -> EventOrigin {
    EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User)
}

fn runtime() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

fn lab() -> EventOrigin {
    EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab)
}

fn received() -> EventPayloadDraft {
    CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into()
}

fn rejected() -> EventPayloadDraft {
    CommandPayloadDraft::rejected(
        EventAction::RuntimeStart,
        DiagnosticCode::CommandRejected,
        EffectDisposition::NotPerformed,
        AuditInput::new(),
    )
    .into()
}

fn runtime_fact(key: &str) -> EventPayloadDraft {
    RuntimePayloadDraft::fact_recorded(
        RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: key.to_owned(),
            value: FactValue::Integer(1),
            observed_at_unix_ms: AT_MS,
            source: OriginModule::Runtime,
            ttl_ms: None,
        },
        AuditInput::new(),
    )
    .into()
}

/// The views' corpus: the identities its records carry and the committed records.
struct Corpus {
    database: Arc<RuntimeDatabase>,
    lab_request: RequestId,
    lab_correlation: CorrelationId,
    lab_run: RunId,
    /// The committed records, in sequence order.
    events: Vec<PersistedEvent>,
    /// The Lab anchor's position among `events`.
    anchor: usize,
}

fn open(root: &TempDir, database: &Arc<RuntimeDatabase>, owner: &str) -> GlobalLedger {
    GlobalLedger::open_sqlite_candidate(
        GlobalLedgerConfig::new(root.path(), owner).with_history_page_rows(ROWS),
        Arc::clone(database),
    )
    .expect("a SQLite ledger")
}

/// Commits the corpus: a command of instance A; a rejected command (a warning) in a run no Lab
/// anchor names; a monitor probe (health) in the Lab run before the run names the Lab request; a
/// capture request (observation) in the Lab run under the Lab request; a command under the Lab
/// correlation; the Lab anchor (a Lab request naming that request and correlation); a command in
/// the Lab run without the request; a command under the Lab request; a second rejected command in
/// the other run; a command linked to nothing Lab.
fn corpus(root: &TempDir) -> (GlobalLedger, Corpus) {
    let database = sqlite_contract::database(root.path());
    let ledger = open(root, &database, "ln6-views");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let instance_a = ids.mint_instance_id().expect("instance");
    let instance_b = ids.mint_instance_id().expect("instance");
    let lab_request = ids.mint_request_id().expect("request");
    let lab_correlation = ids.mint_correlation_id().expect("correlation");
    let lab_run = ids.mint_run_id().expect("run");
    let other_run = ids.mint_run_id().expect("run");
    let drafts = [
        draft(
            &ids,
            0,
            cli(),
            EventLinksDraft::default()
                .with_instance_id(instance_a)
                .with_request_id(ids.mint_request_id().expect("request")),
            received(),
        ),
        draft(
            &ids,
            1,
            runtime(),
            EventLinksDraft::default()
                .with_instance_id(instance_b)
                .with_run_id(other_run),
            rejected(),
        ),
        draft(
            &ids,
            2,
            runtime(),
            EventLinksDraft::default().with_run_id(lab_run),
            MonitorPayloadDraft::requested(AuditInput::new()).into(),
        ),
        draft(
            &ids,
            3,
            runtime(),
            EventLinksDraft::default()
                .with_instance_id(instance_a)
                .with_request_id(lab_request)
                .with_run_id(lab_run)
                .with_frame_id(ids.mint_frame_id().expect("frame")),
            CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()).into(),
        ),
        draft(
            &ids,
            4,
            cli(),
            EventLinksDraft::default()
                .with_instance_id(instance_b)
                .with_correlation_id(lab_correlation),
            received(),
        ),
        draft(
            &ids,
            5,
            lab(),
            EventLinksDraft::default()
                .with_request_id(lab_request)
                .with_correlation_id(lab_correlation),
            ClientPayloadDraft::lab_request(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        ),
        draft(
            &ids,
            6,
            cli(),
            EventLinksDraft::default()
                .with_instance_id(instance_a)
                .with_run_id(lab_run),
            received(),
        ),
        draft(
            &ids,
            7,
            cli(),
            EventLinksDraft::default()
                .with_instance_id(instance_a)
                .with_request_id(lab_request),
            received(),
        ),
        draft(
            &ids,
            8,
            runtime(),
            EventLinksDraft::default()
                .with_instance_id(instance_b)
                .with_run_id(other_run),
            rejected(),
        ),
        draft(
            &ids,
            9,
            cli(),
            EventLinksDraft::default().with_instance_id(instance_b),
            received(),
        ),
    ];
    let events = drafts
        .into_iter()
        .map(|draft| ledger.append(draft).expect("a corpus record"))
        .collect::<Vec<_>>();
    (
        ledger,
        Corpus {
            database,
            lab_request: *lab_request.transport(),
            lab_correlation: *lab_correlation.transport(),
            lab_run: *lab_run.transport(),
            events,
            anchor: 5,
        },
    )
}

fn text(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .expect("a value")
        .as_str()
        .expect("a string value")
        .to_owned()
}

/// The event ids the stored view of `view` selects at the position of `at`: its rows at or
/// before that position whose `available_from_sequence` is at or before it, in ledger order.
fn stored_view(database: &RuntimeDatabase, view: LedgerView, at: &PersistedEvent) -> Vec<EventId> {
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("a connection");
    let mut statement = connection
        .prepare(&format!(
            "SELECT event_id FROM ledger_view_{}_v1 WHERE sequence<=(SELECT sequence FROM ledger_events WHERE event_id=?1) AND available_from_sequence<=(SELECT sequence FROM ledger_events WHERE event_id=?1) ORDER BY sequence",
            text(view)
        ))
        .expect("the stored view");
    statement
        .query_map([text(at.event_id())], |row| row.get::<_, String>(0))
        .expect("the stored view's rows")
        .map(|id| {
            serde_json::from_value(serde_json::Value::String(id.expect("an event id")))
                .expect("a stored event id")
        })
        .collect()
}

/// The first page request of a read at `snapshot`.
fn first_request(snapshot: u64) -> RuntimeEventQueryPageRequest {
    RuntimeEventQueryPageRequest::new(PAGE, None)
        .and_then(|request| request.at_snapshot(snapshot))
        .expect("a first page request")
}

/// The in-memory projection of the same records at the head: the views' definitions.
fn reference_page(
    ledger: &GlobalLedger,
    query: &EventQuery,
    request: &RuntimeEventQueryPageRequest,
) -> RuntimeEventQueryPage {
    let events = ledger.query(EventQuery::default()).expect("every record");
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

/// Every row of `query`'s pages at `snapshot`, each page checked against the in-memory page of
/// the same request.
fn read_pages(ledger: &GlobalLedger, query: &EventQuery, snapshot: u64) -> Vec<ProjectedEvent> {
    let mut rows = Vec::new();
    let mut request = first_request(snapshot);
    loop {
        let page = ledger
            .project_view_page(query.clone(), ProjectionProfile::Concise, request.clone())
            .expect("a view page");
        assert_eq!(
            page,
            reference_page(ledger, query, &request),
            "the in-memory page of {query:?} at {snapshot}"
        );
        rows.extend(page.events().iter().cloned());
        match page.next_cursor() {
            Some(cursor) => {
                request = RuntimeEventQueryPageRequest::new(PAGE, Some(cursor.clone()))
                    .expect("a next page request");
            }
            None => return rows,
        }
    }
}

/// T1's assertions at the positions of `corpus.events[at]`, for every view.
fn assert_views(ledger: &GlobalLedger, corpus: &Corpus, when: &str) {
    for at in [corpus.anchor - 1, corpus.anchor, corpus.events.len() - 1] {
        let snapshot = &corpus.events[at];
        let stored = LedgerView::ALL
            .into_iter()
            .map(|view| (view, stored_view(&corpus.database, view, snapshot)))
            .collect::<Vec<_>>();
        for (view, selected) in &stored {
            let query = EventQuery {
                view: Some(*view),
                ..EventQuery::default()
            };
            let rows = read_pages(ledger, &query, snapshot.sequence());
            assert_eq!(
                rows.iter().map(|row| row.event_id).collect::<Vec<_>>(),
                *selected,
                "the {view:?} view at {at}, {when}"
            );
            for row in &rows {
                let memberships = stored
                    .iter()
                    .filter(|(_, selected)| selected.contains(&row.event_id))
                    .map(|(view, _)| *view)
                    .collect::<Vec<_>>();
                assert_eq!(
                    row.views, memberships,
                    "the views of record {} at {at}, {when}",
                    row.sequence
                );
            }
        }
    }
}

// T1: each view returns what its stored view selects, page by page; again after the writer is
// opened anew.
#[test]
fn ln6_t1_each_view_returns_what_its_stored_view_selects() {
    let root = TempDir::new().expect("state root");
    let (ledger, corpus) = corpus(&root);
    // Every view selects something at the head, so no comparison is between two empty lists.
    let head = corpus.events.last().expect("the head record");
    for view in LedgerView::ALL {
        assert!(
            !stored_view(&corpus.database, view, head).is_empty(),
            "the corpus has records of the {view:?} view"
        );
    }
    assert_views(&ledger, &corpus, "on the writer that appended them");
    ledger.close().expect("close the writer");
    let reopened = open(&root, &corpus.database, "ln6-views-reopened");
    assert_views(&reopened, &corpus, "on a writer opened anew");
    reopened.close().expect("close the reopened writer");
}

// T2: the Lab view relates a record through the anchor's request or correlation, directly or
// through its run, from the anchor's position on.
#[test]
fn ln6_t2_the_lab_view_relates_records_through_the_anchors_links_from_its_position() {
    let root = TempDir::new().expect("state root");
    let (ledger, corpus) = corpus(&root);
    let events = &corpus.events;
    let query = EventQuery {
        view: Some(LedgerView::Lab),
        ..EventQuery::default()
    };
    let lab_ids = |snapshot: usize| {
        read_pages(&ledger, &query, events[snapshot].sequence())
            .into_iter()
            .map(|row| row.event_id)
            .collect::<Vec<_>>()
    };
    let ids = |positions: &[usize]| {
        positions
            .iter()
            .map(|position| *events[*position].event_id())
            .collect::<Vec<_>>()
    };
    // Before the anchor nothing relates to Lab: the request, the correlation and the run are
    // not yet named by an anchor.
    assert!(
        lab_ids(corpus.anchor - 1).is_empty(),
        "nothing before the anchor"
    );
    // At the anchor: the run's earlier record (through the run, which names the request), the
    // record under the request, the record under the correlation, and the anchor itself.
    assert_eq!(lab_ids(corpus.anchor), ids(&[2, 3, 4, 5]), "at the anchor");
    // At the head also the run's later record and the request's later record; the other run's
    // records and the unlinked record never relate.
    assert_eq!(
        lab_ids(events.len() - 1),
        ids(&[2, 3, 4, 5, 6, 7]),
        "at the head"
    );
    // The relation's links are the corpus's.
    assert_eq!(events[3].links().request_id(), Some(&corpus.lab_request));
    assert_eq!(
        events[4].links().correlation_id(),
        Some(&corpus.lab_correlation)
    );
    assert_eq!(events[6].links().run_id(), Some(&corpus.lab_run));
    // The Lab view of one instance: the relation and the instance condition together.
    let instance = *events[3].links().instance_id().expect("instance A");
    let of_instance = read_pages(
        &ledger,
        &EventQuery {
            view: Some(LedgerView::Lab),
            instance_id: Some(instance),
            ..EventQuery::default()
        },
        events[events.len() - 1].sequence(),
    );
    assert_eq!(
        of_instance
            .iter()
            .map(|row| row.event_id)
            .collect::<Vec<_>>(),
        ids(&[3, 6, 7]),
        "the Lab view of instance A"
    );
    ledger.close().expect("close the writer");
}

fn selection(capture: &Capture, snapshot: u64) -> LedgerArtifactSelection {
    LedgerArtifactSelection {
        event: LedgerEventPosition {
            event_id: capture.verified.event_id,
            sequence: capture.verified.sequence,
        },
        artifact_id: capture.reference.artifact_id,
        snapshot_position: snapshot,
        sha256: capture.reference.sha256.clone(),
        byte_count: capture.reference.byte_count,
        run_id: None,
        frame_id: None,
        request_id: None,
        correlation_id: None,
    }
}

/// A deadline `GATE_WAIT` ahead. A gate file reads no clock itself (test plan v2.2 §2.1 guard 3);
/// as in the crate's gate fixture, the deadline is the one `MaintenanceLimits` computes.
fn deadline() -> Instant {
    MaintenanceLimits {
        timeout_seconds: GATE_WAIT.as_secs(),
        ..MaintenanceLimits::default()
    }
    .deadline()
    .expect("a deadline GATE_WAIT ahead")
}

// T3: an artifact without an eviction proof and one with its intent and outcome; a position the
// ledger does not hold is refused.
#[test]
fn ln6_t3_resolve_artifact_answers_with_and_without_proofs() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let kept = capture(&ledger, &artifacts, b"frame kept without a proof");
    let evicted = capture(&ledger, &artifacts, b"frame evicted with its proof");
    let guard = try_artifact_delete_guard(artifacts.root(), &evicted.reference)
        .expect("delete guard")
        .expect("material guard is free");
    let permit = match ledger.admit_artifact_eviction(guard).expect("admission") {
        ArtifactEvictionAdmission::Committed(permit) => permit,
        ArtifactEvictionAdmission::Deferred => panic!("the closed capture is admitted"),
    };
    let outcome = ledger
        .finish_artifact_eviction(permit)
        .expect("recorded deletion");
    let head = ledger.latest_sequence().expect("head");

    let resolved = ledger
        .resolve_artifact(selection(&kept, head), deadline())
        .expect("the kept artifact");
    assert_eq!(resolved.event().sequence, kept.verified.sequence);
    assert_eq!(resolved.reference().artifact_id, kept.reference.artifact_id);
    assert_eq!(resolved.reference().sha256, kept.reference.sha256);
    assert_eq!(resolved.availability_through(), head);
    assert!(resolved.eviction().is_none(), "no eviction proof");

    let resolved = ledger
        .resolve_artifact(selection(&evicted, head), deadline())
        .expect("the evicted artifact");
    assert_eq!(
        resolved.reference().artifact_id,
        evicted.reference.artifact_id
    );
    let eviction = resolved.eviction().expect("its eviction proof");
    assert_eq!(eviction.artifact_id, evicted.reference.artifact_id);
    assert!(
        eviction.intent.sequence < outcome.sequence(),
        "{eviction:?}"
    );
    assert_eq!(
        eviction.terminal_event,
        Some(TerminalEvent {
            sequence: outcome.sequence(),
            event_id: *outcome.event_id(),
        })
    );
    assert!(eviction.disposition.is_some(), "{eviction:?}");

    // Another record at the selected position, and a snapshot beyond the head.
    let mut other = selection(&kept, head);
    other.event.event_id = *outcome.event_id();
    let error = ledger
        .resolve_artifact(other, deadline())
        .expect_err("not the record at that position");
    assert_eq!(
        (error.code(), error.is_fatal()),
        (LedgerCode::ArtifactEventNotFound.as_str(), false)
    );
    let error = ledger
        .resolve_artifact(selection(&kept, head + 1), deadline())
        .expect_err("a snapshot beyond the head");
    assert_eq!(
        (error.code(), error.is_fatal()),
        (LedgerCode::ArtifactSnapshotPositionInvalid.as_str(), false)
    );
    ledger.close().expect("close");
}

/// A test clock in milliseconds that moves `step_ms` each time it is read.
fn stepping_clock(step_ms: u64) -> LedgerMonotonicClock {
    let now = Arc::new(AtomicU64::new(0));
    Arc::new(move || now.fetch_add(step_ms, Ordering::SeqCst))
}

// T4: a view page its work budget stops answers incomplete; its cursors read the whole selection.
#[test]
fn ln6_t4_a_view_page_over_its_budget_answers_incomplete_and_its_cursors_read_it_whole() {
    const EVENTS: u64 = 1_000;
    let root = TempDir::new().expect("state root");
    // Every look at the clock moves it 400 ms: a read stops at its third look at the clock,
    // after three times `READ_BUDGET_CHECK_ROWS` rows.
    let ledger = sqlite_contract::open(
        GlobalLedgerConfig::new(root.path(), "ln6-budget")
            .with_read_budget(Duration::from_millis(1_000))
            .with_clock(stepping_clock(400))
            .with_history_page_rows(ROWS),
    )
    .expect("a SQLite ledger");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for offset in 0..EVENTS {
        ledger
            .append_deferred(draft(
                &ids,
                offset,
                cli(),
                EventLinksDraft::default(),
                received(),
            ))
            .expect("accepted");
    }
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("confirmation");
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    let fact = ledger
        .append(draft(
            &ids,
            EVENTS,
            runtime(),
            EventLinksDraft::default(),
            runtime_fact(FACT_KEY),
        ))
        .expect("the fact record");
    let head = ledger.latest_sequence().expect("head");
    // The fact key has no column: every record of the Changes view is a candidate.
    let query = EventQuery {
        view: Some(LedgerView::Changes),
        fact_key: Some(FACT_KEY.to_owned()),
        ..EventQuery::default()
    };
    let mut request = RuntimeEventQueryPageRequest::new(PAGE, None).expect("a first page");
    let mut rows = Vec::new();
    let mut stopped = 0;
    let mut scanned = 0;
    loop {
        let page = ledger
            .project_view_page(query.clone(), ProjectionProfile::Concise, request)
            .expect("a page within the budget");
        assert_eq!(page.snapshot_ledger_position(), head);
        let scope = page.read_scope().expect("a Runtime page has its scope");
        assert!(scope.scanned_through_position > scanned, "{scope:?}");
        scanned = scope.scanned_through_position;
        if scope.limits.contains(&LedgerPageLimit::ReadBudget) {
            stopped += 1;
            assert!(!scope.read_complete, "{scope:?}");
            assert!(scope.scanned_through_position < head, "{scope:?}");
            assert!(page.has_more(), "a stopped page continues");
        }
        rows.extend(page.events().iter().map(|row| row.event_id));
        match page.next_cursor() {
            Some(cursor) => {
                assert!(page.has_more());
                request = RuntimeEventQueryPageRequest::new(PAGE, Some(cursor.clone()))
                    .expect("a next page");
            }
            None => break,
        }
    }
    assert!(stopped >= 1, "the budget stopped at least the first page");
    assert_eq!(rows, vec![*fact.event_id()], "the whole selection");
    ledger
        .append(draft(
            &ids,
            EVENTS + 1,
            cli(),
            EventLinksDraft::default(),
            received(),
        ))
        .expect("the writer goes on after the stopped pages");
    ledger.close().expect("close");
}
