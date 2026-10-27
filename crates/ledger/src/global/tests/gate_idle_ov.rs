// SPDX-License-Identifier: AGPL-3.0-only

//! #422 IDLE-OV (v0.12.19; brief rd-1027/brief-idle-overview.md, investigation
//! rd-1027/q-idle-overview-budget.md §2). A Runtime view page read the Lab relations (every Lab
//! anchor of the whole history, LN-6) before its selection on every page, also on an empty page
//! and on a page without the Lab view. Those statements are outside the page's selection, so a
//! statement the read's bound interrupted there (the read was long, or its budget was already
//! spent waiting for the shared connection) refused the page with `ledger_read_budget_exceeded`.
//! The required states, the ledger's part:
//! - I-3: an empty page (the run listing's read of an instance without a record in the window)
//!   never reads the Lab relations, and is answered even when its budget is spent before it
//!   starts, on a history whose Lab relations' read is longer than one look interval of the bound;
//! - I-4: the Lab view still gets its relations: through a Lab anchor's request, its correlation
//!   and a run that names one of them; and a page of rows outside the Lab view still names the
//!   Lab view among the views of a related row;
//! - I-8 (review M1, fix round 2): a page outside the Lab view reads its Lab relations at its
//!   first row that names a request, a correlation or a run; when its budget runs out there, the
//!   page answers incomplete with the rows before that row, never a refusal, and the next page
//!   starts at that row.
//!
//! The ledger runs on a writer clock in milliseconds that moves a set step each time it is read
//! (1 ms, or two budgets to spend a read's budget at its first look); nothing sleeps. The
//! statements of each read are recorded on the writer's thread (`WriterStatementRecorder`).

use super::*;
use crate::global::{
    GlobalLedger, GlobalLedgerConfig, LedgerMonotonicClock, LedgerWriterCommandKind,
    Sha256SecretFingerprinter,
};
use crate::{LedgerWriterCommandStatements, WriterStatementRecorder};
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, EventAction, EventActor, EventDraft,
    EventId, EventLinksDraft, EventOrigin, EventPayloadDraft, EventQuery, EventSeverity,
    EventSource, EventType, IdentifierIssuer, LedgerPageLimit, LedgerView, OriginModule,
    ProjectionProfile, RuntimeEventQueryPage, RuntimeEventQueryPageRequest, SanitizedEventDraft,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;

/// The one bound of a wait in this file (the deferred appends' confirmation).
const GATE_WAIT: Duration = Duration::from_secs(30);
const AT_MS: u64 = 1_752_147_200_000;
/// The read budget (`ledger_observer.read_budget_ms`'s default).
const BUDGET_MS: u64 = 1_000;
/// `ledger_observer.read_check_vm_steps`'s default: the bound looks at the clock every this many
/// SQLite steps of a statement.
const CHECK_VM_STEPS: u64 = 10_000;
/// The Lab anchors of the history: enough that one statement of the Lab relations' read runs
/// past one look interval of the bound.
const LAB_ANCHORS: u64 = 2_000;
/// I-8: the writer clock's step during the page: a page reads the clock when it starts, before
/// its selection's second span and at each look of its bound inside a statement, so the look
/// before the second span sees three quarters of the budget spent and the next look, the first
/// inside the Lab relations' read, sees it spent.
const I8_STEP_MS: u64 = BUDGET_MS * 3 / 4;
const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;

fn salt() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"idle-ov-fixture-salt").expect("salt")
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

fn lab() -> EventOrigin {
    EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab)
}

fn received() -> EventPayloadDraft {
    CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into()
}

fn lab_request() -> EventPayloadDraft {
    ClientPayloadDraft::lab_request(EventAction::RuntimeTaskRun, AuditInput::new()).into()
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

/// The history's identities: instance B, which has no record, instance C and C's four records
/// in ledger order; and the request of a third Lab anchor, which no record of C names.
struct Corpus {
    instance_b: actingcommand_contract::InstanceId,
    instance_c: actingcommand_contract::InstanceId,
    by_request: EventId,
    by_correlation: EventId,
    by_run: EventId,
    unrelated: EventId,
    third_anchor_request: actingcommand_contract::IssuedRequestId,
}

/// Commits the history: `LAB_ANCHORS` Lab requests of instance A, each with its own request and
/// correlation; then on instance C a record under the first anchor's request in a run (the Lab
/// run), one under the second anchor's correlation, one in the Lab run without the request, and
/// one under a request no anchor names.
fn corpus(ledger: &GlobalLedger) -> Corpus {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let instance_a = ids.mint_instance_id().expect("instance");
    let instance_b = ids.mint_instance_id().expect("instance");
    let instance_c = ids.mint_instance_id().expect("instance");
    let mut anchors = Vec::new();
    for offset in 0..LAB_ANCHORS {
        let request = ids.mint_request_id().expect("request");
        let correlation = ids.mint_correlation_id().expect("correlation");
        anchors.push((request, correlation));
        ledger
            .append_deferred(draft(
                &ids,
                offset,
                lab(),
                EventLinksDraft::default()
                    .with_instance_id(instance_a)
                    .with_request_id(request)
                    .with_correlation_id(correlation),
                lab_request(),
            ))
            .expect("accepted");
    }
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("confirmation");
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    let lab_run = ids.mint_run_id().expect("run");
    let mut offset = LAB_ANCHORS;
    let mut append = |links: EventLinksDraft| {
        offset += 1;
        *ledger
            .append(draft(
                &ids,
                offset,
                cli(),
                links.with_instance_id(instance_c),
                received(),
            ))
            .expect("a corpus record")
            .event_id()
    };
    let by_request = append(
        EventLinksDraft::default()
            .with_request_id(anchors[0].0)
            .with_run_id(lab_run),
    );
    let by_correlation = append(EventLinksDraft::default().with_correlation_id(anchors[1].1));
    let by_run = append(EventLinksDraft::default().with_run_id(lab_run));
    let unrelated =
        append(EventLinksDraft::default().with_request_id(ids.mint_request_id().expect("request")));
    Corpus {
        instance_b: *instance_b.transport(),
        instance_c: *instance_c.transport(),
        by_request,
        by_correlation,
        by_run,
        unrelated,
        third_anchor_request: anchors[2].0,
    }
}

fn open(root: &TempDir, clock: &Clock, owner: &str) -> GlobalLedger {
    sqlite_contract::open(
        GlobalLedgerConfig::new(root.path(), owner)
            .with_read_budget(Duration::from_millis(BUDGET_MS))
            .with_clock(clock.clock()),
    )
    .expect("a SQLite ledger")
}

fn first_page(limit: u16) -> RuntimeEventQueryPageRequest {
    RuntimeEventQueryPageRequest::new(limit, None).expect("a first page")
}

/// The one view page among the read commands `recorder` recorded since it was last read (the
/// writer records a command after its reply; it has answered a later command, the head read,
/// once every earlier one is recorded).
fn page_read(
    ledger: &GlobalLedger,
    recorder: &WriterStatementRecorder,
) -> LedgerWriterCommandStatements {
    ledger.latest_sequence().expect("the head");
    let mut pages = recorder
        .take()
        .into_iter()
        .filter(|command| command.command == LedgerWriterCommandKind::ProjectViewPage)
        .collect::<Vec<_>>();
    assert_eq!(pages.len(), 1, "one view page was read: {pages:?}");
    pages.remove(0)
}

/// The SQLite work of each statement of the Lab relations' read (`read_lab_relations`: its two
/// anchor statements and its run statement) among `read`'s.
fn lab_relation_steps(read: &LedgerWriterCommandStatements) -> Vec<u64> {
    read.statements
        .iter()
        .filter(|statement| {
            statement
                .sql
                .contains("FROM ledger_events a JOIN ledger_links al ON al.sequence=a.sequence")
                || statement
                    .sql
                    .starts_with("SELECT r.run_id FROM ledger_links r")
        })
        .map(|statement| statement.vm_steps)
        .collect()
}

#[test]
fn idle_ov_i3_an_empty_page_never_reads_the_lab_relations_and_is_never_refused_for_them() {
    let root = TempDir::new().expect("state root");
    let clock = Clock::new();
    let ledger = open(&root, &clock, "idle-ov-i3");
    let corpus = corpus(&ledger);
    let recorder = WriterStatementRecorder::arm(root.path()).expect("the recorder");

    // The fixture: a page with rows reads the Lab relations, and on this history one of their
    // statements runs past one look interval of the bound, so a spent budget interrupts it.
    let page = ledger
        .project_view_page(
            EventQuery {
                instance_id: Some(corpus.instance_c),
                ..EventQuery::default()
            },
            PROFILE,
            first_page(10),
        )
        .expect("a page with rows");
    assert!(!page.events().is_empty(), "the page has rows");
    let steps = lab_relation_steps(&page_read(&ledger, &recorder));
    assert!(
        steps.iter().any(|steps| *steps > CHECK_VM_STEPS),
        "the Lab relations' statements on this history: {steps:?} steps"
    );

    // The run listing's read of an instance without a record in the window: an empty page,
    // complete, that reads none of the Lab relations.
    let empty = EventQuery {
        instance_id: Some(corpus.instance_b),
        event_type: Some(EventType::TaskRequested),
        from_timestamp_unix_ms: Some(AT_MS),
        ..EventQuery::default()
    };
    let page = ledger
        .project_view_page(empty.clone(), PROFILE, first_page(256))
        .expect("the empty page");
    assert!(
        page.events().is_empty() && !page.has_more(),
        "an empty, complete page"
    );
    let read = page_read(&ledger, &recorder);
    assert_eq!(
        lab_relation_steps(&read),
        Vec::<u64>::new(),
        "the empty page read the Lab relations: {read:?}"
    );

    // The same page with its budget spent before it starts (as when it waited for the shared
    // connection): answered, with no row, and still no Lab relations read.
    clock.step_ms(2 * BUDGET_MS);
    let spent = ledger.project_view_page(empty, PROFILE, first_page(256));
    clock.step_ms(1);
    let page = spent.expect("the empty page with its budget spent is answered, not refused");
    assert!(page.events().is_empty(), "no row");
    let read = page_read(&ledger, &recorder);
    assert_eq!(
        lab_relation_steps(&read),
        Vec::<u64>::new(),
        "the empty page read the Lab relations: {read:?}"
    );
    drop(recorder);
    ledger.close().expect("close");
}

#[test]
fn idle_ov_i4_the_lab_view_still_gets_its_relations() {
    let root = TempDir::new().expect("state root");
    let clock = Clock::new();
    let ledger = open(&root, &clock, "idle-ov-i4");
    let corpus = corpus(&ledger);
    let ids = |page: &actingcommand_contract::RuntimeEventQueryPage| {
        page.events()
            .iter()
            .map(|row| row.event_id)
            .collect::<Vec<_>>()
    };

    // The Lab view of instance C: the records related through the anchor's request, its
    // correlation and the run that names its request; not the unrelated one.
    let page = ledger
        .project_view_page(
            EventQuery {
                view: Some(LedgerView::Lab),
                instance_id: Some(corpus.instance_c),
                ..EventQuery::default()
            },
            PROFILE,
            first_page(10),
        )
        .expect("the Lab view page");
    assert_eq!(
        ids(&page),
        vec![corpus.by_request, corpus.by_correlation, corpus.by_run],
        "the Lab view of instance C"
    );
    assert!(
        page.events()
            .iter()
            .all(|row| row.views.contains(&LedgerView::Lab)),
        "each row names the Lab view"
    );

    // Instance C's rows without a view: the related rows name the Lab view, the other does not.
    let page = ledger
        .project_view_page(
            EventQuery {
                instance_id: Some(corpus.instance_c),
                ..EventQuery::default()
            },
            PROFILE,
            first_page(10),
        )
        .expect("the page without a view");
    let lab_rows = page
        .events()
        .iter()
        .filter(|row| row.views.contains(&LedgerView::Lab))
        .map(|row| row.event_id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids(&page),
        vec![
            corpus.by_request,
            corpus.by_correlation,
            corpus.by_run,
            corpus.unrelated
        ],
        "instance C's rows"
    );
    assert_eq!(
        lab_rows,
        vec![corpus.by_request, corpus.by_correlation, corpus.by_run],
        "the rows that name the Lab view"
    );
    ledger.close().expect("close");
}

#[test]
fn idle_ov_i8_a_page_whose_budget_ends_at_its_first_linked_row_answers_the_rows_before_it() {
    let root = TempDir::new().expect("state root");
    let clock = Clock::new();
    let ledger = open(&root, &clock, "idle-ov-i8");
    let corpus = corpus(&ledger);
    // Instance D, one record after another: two that name no request, correlation or run, one
    // under a Lab anchor's request (the page's first linked row) and one more without a link.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let instance_d = ids.mint_instance_id().expect("instance");
    let mut offset = 2 * LAB_ANCHORS;
    let mut append = |links: EventLinksDraft| {
        offset += 1;
        ledger
            .append(draft(
                &ids,
                offset,
                cli(),
                links.with_instance_id(instance_d),
                received(),
            ))
            .expect("a record of instance D")
    };
    let first = append(EventLinksDraft::default());
    let second = append(EventLinksDraft::default());
    let linked = append(EventLinksDraft::default().with_request_id(corpus.third_anchor_request));
    let last = append(EventLinksDraft::default());
    assert_eq!(
        [second.sequence(), linked.sequence(), last.sequence()],
        [
            first.sequence() + 1,
            first.sequence() + 2,
            first.sequence() + 3
        ],
        "instance D's records follow one another"
    );
    let query = EventQuery {
        instance_id: Some(*instance_d.transport()),
        from_sequence: Some(first.sequence()),
        ..EventQuery::default()
    };
    let rows = |page: &RuntimeEventQueryPage| {
        page.events()
            .iter()
            .map(|row| row.event_id)
            .collect::<Vec<_>>()
    };

    // The page's first span selects the first record; the look before its second span sees
    // the budget not yet spent; the second span selects the second record and reaches the
    // linked row, whose Lab relations' read the bound cuts off at its first look.
    clock.step_ms(I8_STEP_MS);
    let cut = ledger.project_view_page(query.clone(), PROFILE, first_page(10));
    clock.step_ms(1);
    let page = cut.expect("the page whose budget ended at its linked row is answered, not refused");
    assert_eq!(
        rows(&page),
        vec![*first.event_id(), *second.event_id()],
        "the rows before the linked row"
    );
    let scope = page.read_scope().expect("a Runtime page has its scope");
    assert!(
        !scope.read_complete && scope.limits.contains(&LedgerPageLimit::ReadBudget),
        "the budget ended the page: {scope:?}"
    );
    assert!(page.has_more(), "{scope:?}");
    let cursor = page.next_cursor().expect("a page that continues").clone();
    assert_eq!(
        cursor.after_sequence(),
        second.sequence(),
        "the page ends just before the linked row"
    );

    // The next page, its budget whole, starts at the linked row, which names the Lab view.
    let next = ledger
        .project_view_page(
            query,
            PROFILE,
            RuntimeEventQueryPageRequest::new(10, Some(cursor)).expect("the next page"),
        )
        .expect("the next page");
    assert_eq!(
        rows(&next),
        vec![*linked.event_id(), *last.event_id()],
        "the next page"
    );
    assert!(
        next.events()[0].views.contains(&LedgerView::Lab),
        "the linked row names the Lab view"
    );
    assert!(!next.has_more(), "the next page ends the read");
    ledger.close().expect("close");
}
