// SPDX-License-Identifier: AGPL-3.0-only

//! LN-5 (#422, LEAN-L §3.3, §3.4 and §5 LN-5; brief rd-1024/brief-ln5.md): the ledger's history
//! is on disk. The writer keeps no list and no index of its records; its reads and the checks
//! its appends make read the store. Each test runs on a SQLite store whose history pages hold
//! two rows (`history_page_rows`), so every read spans several pages:
//!
//! - T1: each `EventQuery` condition selects exactly the records the query contract
//!   (`contracts/global-ledger-query.md`) names, through `query`, page by page through
//!   `query_page` and, for the whole history, through a subscription's replay; again after the
//!   writer is opened anew, when the records are only on disk;
//! - T2: a history read that its work budget stops is refused with
//!   `ledger_read_budget_exceeded`, not fatally; the same history read page by page, each page
//!   within the budget, is the whole history;
//! - T3: a duplicate event id is refused, against a record committed before the writer was
//!   opened anew and against a record staged in the same batch;
//! - T4: a task fact's run rules see the run's records staged in the same batch: a refused
//!   effect after its step's completed effect is admitted, one without it is refused;
//! - T5: the scheduled-settlement continuation reads its chain from the store: after the writer
//!   is opened anew it settles the run once and a second call returns the same completion; a
//!   chain without its lease grant is refused and the writer goes on.
//!
//! Every record time is a fixed offset; T2's budget runs on a clock that moves a fixed step each
//! time it is read. `BARRIER_WAIT` only bounds a broken run.

use super::*;
use crate::WriterReadHoldGate;
use crate::codes::LedgerCode;
use crate::global::{
    GlobalLedger, GlobalLedgerConfig, LedgerMonotonicClock, Sha256SecretFingerprinter,
};
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, DiagnosticCode, EffectDisposition,
    EventAction, EventActor, EventDraft, EventId, EventLinksDraft, EventOrigin, EventPayloadDraft,
    EventQuery, EventSeverity, EventSource, EventType, FactValue, IdentifierIssuer, IssuedEventId,
    LedgerView, OriginModule, RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft,
    SanitizedEventDraft, SubscriptionCursor, TaskPayloadDraft, TaskSemanticFact,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;

const AT_MS: u64 = 1_752_147_200_000;
/// The rows of one history page in these tests.
const ROWS: usize = 2;
/// The fact key T1's runtime fact names.
const FACT_KEY: &str = "backend.ready";

fn open(root: &TempDir, owner: &str) -> GlobalLedger {
    sqlite_contract::open(GlobalLedgerConfig::new(root.path(), owner).with_history_page_rows(ROWS))
        .expect("a SQLite ledger")
}

fn salt() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"ln5-fixture-salt").expect("salt")
}

fn draft_with_id(
    event_id: IssuedEventId,
    offset: u64,
    origin: EventOrigin,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) -> SanitizedEventDraft {
    EventDraft::new(
        event_id,
        AT_MS + offset,
        EventSeverity::Info,
        origin,
        links,
        payload,
    )
    .sanitize(&salt())
    .expect("a fixture draft")
}

fn draft(
    ids: &IdentifierIssuer,
    offset: u64,
    origin: EventOrigin,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) -> SanitizedEventDraft {
    draft_with_id(
        ids.mint_event_id().expect("event id"),
        offset,
        origin,
        links,
        payload,
    )
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

fn lab_request() -> EventPayloadDraft {
    ClientPayloadDraft::lab_request(EventAction::RuntimeTaskRun, AuditInput::new()).into()
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

/// A test clock in milliseconds that moves `step_ms` each time it is read.
fn stepping_clock(step_ms: u64) -> LedgerMonotonicClock {
    let now = Arc::new(AtomicU64::new(0));
    Arc::new(move || now.fetch_add(step_ms, Ordering::SeqCst))
}

/// Every event `query` selects after `after` through `through`, read in pages of `ROWS`.
fn paged(ledger: &GlobalLedger, query: &EventQuery, through: u64) -> Vec<PersistedEvent> {
    let mut events = Vec::new();
    let mut after = 0;
    loop {
        let page = ledger
            .query_page(query.clone(), after, through, ROWS)
            .expect("a history page");
        let full = page.len() == ROWS;
        if let Some(last) = page.last() {
            after = last.sequence();
        }
        events.extend(page);
        if !full {
            return events;
        }
    }
}

/// Holds the writer in a read while `capture` captures records, so that the flush after the
/// read commits them as one batch; returns what `capture` returned.
fn in_one_batch<T>(root: &TempDir, ledger: &GlobalLedger, capture: impl FnOnce() -> T) -> T {
    let hold = WriterReadHoldGate::arm(root.path()).expect("read hold");
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| ledger.latest_sequence());
        assert!(hold.wait_held(BARRIER_WAIT), "the read reached the writer");
        let captured = capture();
        hold.release();
        reader.join().expect("reader").expect("the held read");
        captured
    })
}

/// T1's corpus: the identities its records carry.
struct Corpus {
    instance_a: actingcommand_contract::InstanceId,
    instance_b: actingcommand_contract::InstanceId,
    lab_request: actingcommand_contract::RequestId,
    lab_correlation: actingcommand_contract::CorrelationId,
    lab_run: actingcommand_contract::RunId,
    /// The record that carries every link, with a value no other record has.
    linked: EventLinksDraft,
    /// The committed records, in sequence order.
    events: Vec<PersistedEvent>,
    /// The Lab anchor's position among `events`.
    anchor: usize,
}

/// Commits T1's corpus: a CLI command of instance A; a rejected command (an error with its
/// diagnostic code) that carries every link; a runtime fact; a record of instance A in the Lab
/// run under the Lab request; one of instance B under the Lab correlation; the Lab anchor (a Lab
/// request naming that request and correlation); one of instance A in the Lab run without the
/// request; one of instance A under the Lab request; one of instance B linked to nothing Lab.
fn corpus(ledger: &GlobalLedger) -> Corpus {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let instance_a = ids.mint_instance_id().expect("instance");
    let instance_b = ids.mint_instance_id().expect("instance");
    let lab_request = ids.mint_request_id().expect("request");
    let lab_correlation = ids.mint_correlation_id().expect("correlation");
    let lab_run = ids.mint_run_id().expect("run");
    let linked = EventLinksDraft::default()
        .with_instance_id(instance_b)
        .with_request_id(ids.mint_request_id().expect("request"))
        .with_correlation_id(ids.mint_correlation_id().expect("correlation"))
        .with_causation_id(ids.mint_causation_id().expect("causation"))
        .with_task_id(ids.mint_task_id().expect("task"))
        .with_run_id(ids.mint_run_id().expect("run"))
        .with_lease_id(ids.mint_lease_id().expect("lease"))
        .with_frame_id(ids.mint_frame_id().expect("frame"))
        .with_action_id(ids.mint_action_id().expect("action"))
        .with_recognition_id(ids.mint_recognition_id().expect("recognition"));
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
        draft(&ids, 1, runtime(), linked.clone(), rejected()),
        draft(
            &ids,
            2,
            runtime(),
            EventLinksDraft::default(),
            runtime_fact(FACT_KEY),
        ),
        draft(
            &ids,
            3,
            cli(),
            EventLinksDraft::default()
                .with_instance_id(instance_a)
                .with_request_id(lab_request)
                .with_run_id(lab_run),
            received(),
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
            lab_request(),
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
            cli(),
            EventLinksDraft::default().with_instance_id(instance_b),
            received(),
        ),
    ];
    let events = drafts
        .into_iter()
        .map(|draft| ledger.append(draft).expect("a corpus record"))
        .collect::<Vec<_>>();
    Corpus {
        instance_a: *instance_a.transport(),
        instance_b: *instance_b.transport(),
        lab_request: *lab_request.transport(),
        lab_correlation: *lab_correlation.transport(),
        lab_run: *lab_run.transport(),
        linked,
        events,
        anchor: 5,
    }
}

/// Each condition and the records the query contract says it selects among `events`.
fn conditions(corpus: &Corpus) -> Vec<(&'static str, EventQuery, Vec<PersistedEvent>)> {
    let events = &corpus.events;
    let keep = |selected: &dyn Fn(&PersistedEvent) -> bool| {
        events
            .iter()
            .filter(|event| selected(event))
            .cloned()
            .collect::<Vec<_>>()
    };
    let at = |index: usize| events[index].clone();
    let linked = &corpus.linked;
    let (first, last) = (events[2].sequence(), events[6].sequence());
    let (from_time, to_time) = (events[3].timestamp_unix_ms(), events[7].timestamp_unix_ms());
    vec![
        (
            "inclusive sequence bounds",
            EventQuery {
                from_sequence: Some(first),
                to_sequence: Some(last),
                ..EventQuery::default()
            },
            keep(&|event| (first..=last).contains(&event.sequence())),
        ),
        (
            "an event type",
            EventQuery {
                event_type: Some(EventType::CommandRejected),
                ..EventQuery::default()
            },
            keep(&|event| event.event_type() == EventType::CommandRejected),
        ),
        (
            "a set of event types",
            EventQuery {
                event_types: vec![events[2].event_type(), events[corpus.anchor].event_type()],
                ..EventQuery::default()
            },
            keep(&|event| {
                event.event_type() == events[2].event_type()
                    || event.event_type() == events[corpus.anchor].event_type()
            }),
        ),
        (
            "excluded event types",
            EventQuery {
                exclude_event_types: vec![EventType::CommandReceived],
                ..EventQuery::default()
            },
            keep(&|event| event.event_type() != EventType::CommandReceived),
        ),
        (
            "an inclusive minimum severity",
            EventQuery {
                minimum_severity: Some(EventSeverity::Warning),
                ..EventQuery::default()
            },
            keep(&|event| event.severity() >= EventSeverity::Warning),
        ),
        (
            "an inclusive maximum severity",
            EventQuery {
                maximum_severity: Some(EventSeverity::Info),
                ..EventQuery::default()
            },
            keep(&|event| event.severity() <= EventSeverity::Info),
        ),
        (
            "a time window, its end exclusive",
            EventQuery {
                from_timestamp_unix_ms: Some(from_time),
                to_timestamp_unix_ms: Some(to_time),
                ..EventQuery::default()
            },
            keep(&|event| {
                event.timestamp_unix_ms() >= from_time && event.timestamp_unix_ms() < to_time
            }),
        ),
        (
            "a source",
            EventQuery {
                source: Some(EventSource::Lab),
                ..EventQuery::default()
            },
            keep(&|event| event.origin().source() == EventSource::Lab),
        ),
        (
            "an origin module",
            EventQuery {
                origin_module: Some(OriginModule::Runtime),
                ..EventQuery::default()
            },
            keep(&|event| event.origin().module() == OriginModule::Runtime),
        ),
        (
            "a diagnostic code",
            EventQuery {
                diagnostic_code: Some(DiagnosticCode::CommandRejected),
                ..EventQuery::default()
            },
            keep(&|event| {
                event.payload().diagnostic_code() == Some(DiagnosticCode::CommandRejected)
            }),
        ),
        (
            "a fact key",
            EventQuery {
                fact_key: Some(FACT_KEY.to_owned()),
                ..EventQuery::default()
            },
            vec![at(2)],
        ),
        (
            "an instance",
            EventQuery {
                instance_id: Some(corpus.instance_a),
                ..EventQuery::default()
            },
            keep(&|event| event.links().instance_id() == Some(&corpus.instance_a)),
        ),
        (
            "a set of instances",
            EventQuery {
                instance_ids: vec![corpus.instance_a, corpus.instance_b],
                ..EventQuery::default()
            },
            keep(&|event| {
                event.links().instance_id().is_some_and(|instance| {
                    [corpus.instance_a, corpus.instance_b].contains(instance)
                })
            }),
        ),
        (
            "a request",
            EventQuery {
                request_id: Some(corpus.lab_request),
                ..EventQuery::default()
            },
            keep(&|event| event.links().request_id() == Some(&corpus.lab_request)),
        ),
        (
            "a correlation",
            EventQuery {
                correlation_id: Some(corpus.lab_correlation),
                ..EventQuery::default()
            },
            keep(&|event| event.links().correlation_id() == Some(&corpus.lab_correlation)),
        ),
        (
            "a run",
            EventQuery {
                run_id: Some(corpus.lab_run),
                ..EventQuery::default()
            },
            keep(&|event| event.links().run_id() == Some(&corpus.lab_run)),
        ),
        (
            "a causation",
            EventQuery {
                causation_id: linked.causation_id().copied(),
                ..EventQuery::default()
            },
            vec![at(1)],
        ),
        (
            "a task",
            EventQuery {
                task_id: linked.task_id().copied(),
                ..EventQuery::default()
            },
            vec![at(1)],
        ),
        (
            "a lease",
            EventQuery {
                lease_id: linked.lease_id().copied(),
                ..EventQuery::default()
            },
            vec![at(1)],
        ),
        (
            "a frame",
            EventQuery {
                frame_id: linked.frame_id().copied(),
                ..EventQuery::default()
            },
            vec![at(1)],
        ),
        (
            "an action",
            EventQuery {
                action_id: linked.action_id().copied(),
                ..EventQuery::default()
            },
            vec![at(1)],
        ),
        (
            "a recognition",
            EventQuery {
                recognition_id: linked.recognition_id().copied(),
                ..EventQuery::default()
            },
            vec![at(1)],
        ),
        (
            "the errors view",
            EventQuery {
                view: Some(LedgerView::Errors),
                ..EventQuery::default()
            },
            keep(&|event| event.severity() >= EventSeverity::Warning),
        ),
        // The Lab view: the anchor, the records under its request or its correlation, and the
        // records of a run one of whose records names its request.
        (
            "the Lab view",
            EventQuery {
                view: Some(LedgerView::Lab),
                ..EventQuery::default()
            },
            vec![at(3), at(4), at(5), at(6), at(7)],
        ),
        (
            "the Lab view of one instance",
            EventQuery {
                view: Some(LedgerView::Lab),
                instance_id: Some(corpus.instance_a),
                ..EventQuery::default()
            },
            vec![at(3), at(6), at(7)],
        ),
    ]
}

// T1: each condition selects what the query contract names, by every read path and after the
// writer is opened anew.
#[test]
fn ln5_t1_each_query_condition_selects_what_the_contract_names() {
    let root = TempDir::new().expect("state root");
    let ledger = open(&root, "ln5-conditions");
    let corpus = corpus(&ledger);
    let head = corpus.events.last().expect("a record").sequence();
    let check = |ledger: &GlobalLedger, when: &str| {
        assert_eq!(ledger.latest_sequence().expect("head"), head, "{when}");
        for (label, query, expected) in conditions(&corpus) {
            assert_eq!(
                ledger.query(query.clone()).expect("a query"),
                expected,
                "{label}, {when}: query"
            );
            assert_eq!(
                paged(ledger, &query, head),
                expected,
                "{label}, {when}: pages"
            );
        }
        // A page read at a position before the anchor relates nothing to Lab yet.
        let lab = EventQuery {
            view: Some(LedgerView::Lab),
            ..EventQuery::default()
        };
        let before_anchor = corpus.events[corpus.anchor - 1].sequence();
        assert!(
            paged(ledger, &lab, before_anchor).is_empty(),
            "the Lab view before its anchor, {when}"
        );
        assert_eq!(
            paged(ledger, &lab, corpus.events[6].sequence()),
            corpus.events[3..=6].to_vec(),
            "the Lab view at the run's later record, {when}"
        );
        // A subscription replays the whole history from the store.
        let mut subscription = ledger
            .subscribe(SubscriptionCursor { after_sequence: 0 })
            .expect("a subscription");
        let replayed = corpus
            .events
            .iter()
            .map(|_| {
                subscription
                    .recv_timeout(BARRIER_WAIT)
                    .expect("a replayed record")
            })
            .collect::<Vec<_>>();
        assert_eq!(replayed, corpus.events, "the replay, {when}");
    };
    check(&ledger, "while the writer that wrote them runs");
    ledger.close().expect("close");
    let reopened = open(&root, "ln5-conditions-reopened");
    check(&reopened, "after the writer is opened anew");
    reopened.close().expect("close the reopened writer");
}

// T2: a history read over its budget is refused, not fatally; page by page the same history is
// read whole.
#[test]
fn ln5_t2_a_history_read_over_its_budget_is_refused_and_its_pages_read_it_whole() {
    const EVENTS: u64 = 1_000;
    let root = TempDir::new().expect("state root");
    // Every look at the clock moves it 400 ms: a read stops at its third look at the clock,
    // after three times `READ_BUDGET_CHECK_ROWS` records; a page of 256 records looks once.
    let ledger = sqlite_contract::open(
        GlobalLedgerConfig::new(root.path(), "ln5-budget")
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
    let summary = ledger.confirm_deferred(BARRIER_WAIT).expect("confirmation");
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    assert_eq!(summary.committed.len() as u64, EVENTS);
    let head = ledger.latest_sequence().expect("head");

    let error = ledger
        .query(EventQuery::default())
        .expect_err("the whole history is over one read's budget");
    assert_eq!(
        (error.code(), error.is_fatal()),
        (LedgerCode::LedgerReadBudgetExceeded.as_str(), false)
    );

    let mut read = Vec::new();
    let mut after = 0;
    while after < head {
        let page = ledger
            .query_page(EventQuery::default(), after, head, 256)
            .expect("a page within the budget");
        after = page.last().expect("a page of the history").sequence();
        read.extend(page);
    }
    assert_eq!(
        read.iter()
            .map(|event| *event.event_id())
            .collect::<Vec<_>>(),
        summary
            .committed
            .iter()
            .map(|reference| *reference.event_id())
            .collect::<Vec<EventId>>()
    );
    ledger
        .append(draft(
            &ids,
            EVENTS,
            cli(),
            EventLinksDraft::default(),
            received(),
        ))
        .expect("the writer goes on after the refused read");
    ledger.close().expect("close");
}

// T3: a duplicate event id is refused against the store and against the staged batch.
#[test]
fn ln5_t3_a_duplicate_event_id_is_refused_against_the_store_and_the_batch() {
    let root = TempDir::new().expect("state root");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let committed_id = ids.mint_event_id().expect("event id");
    let ledger = open(&root, "ln5-duplicates");
    let first = ledger
        .append(draft_with_id(
            committed_id,
            0,
            cli(),
            EventLinksDraft::default(),
            received(),
        ))
        .expect("the first record");
    ledger.close().expect("close");

    let ledger = open(&root, "ln5-duplicates-reopened");
    let error = ledger
        .append(draft_with_id(
            committed_id,
            1,
            cli(),
            EventLinksDraft::default(),
            received(),
        ))
        .expect_err("an id the store holds");
    assert_eq!(
        (error.code(), error.is_fatal()),
        (LedgerCode::DuplicateEventId.as_str(), false)
    );

    let staged_id = ids.mint_event_id().expect("event id");
    in_one_batch(&root, &ledger, || {
        for offset in [2, 3] {
            ledger
                .append_deferred(draft_with_id(
                    staged_id,
                    offset,
                    cli(),
                    EventLinksDraft::default(),
                    received(),
                ))
                .expect("accepted");
        }
    });
    let confirmed = ledger.confirm_deferred(BARRIER_WAIT).expect("confirmation");
    assert_eq!(confirmed.committed.len(), 1, "{confirmed:?}");
    assert_eq!(confirmed.committed[0].event_id(), staged_id.transport());
    assert_eq!(
        confirmed
            .failed
            .iter()
            .map(|error| (error.code(), error.is_fatal()))
            .collect::<Vec<_>>(),
        [(LedgerCode::DuplicateEventId.as_str(), false)]
    );
    assert_eq!(
        ledger
            .query(EventQuery::default())
            .expect("history")
            .iter()
            .map(|event| *event.event_id())
            .collect::<Vec<_>>(),
        [*first.event_id(), *staged_id.transport()]
    );
    ledger.close().expect("close the reopened writer");
}

fn task_fact(
    ids: &IdentifierIssuer,
    links: &EventLinksDraft,
    offset: u64,
    fact: TaskSemanticFact,
) -> SanitizedEventDraft {
    draft(
        ids,
        offset,
        runtime(),
        links.clone(),
        TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
    )
}

// T4: a run's rules see the run's records staged in the same batch.
#[test]
fn ln5_t4_run_rules_see_the_records_staged_in_the_same_batch() {
    const OPERATION: &str = "confirm_choice";
    let root = TempDir::new().expect("state root");
    let ledger = open(&root, "ln5-run-rules");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = ids.mint_run_id().expect("run");
    let links = EventLinksDraft::default()
        .with_instance_id(ids.mint_instance_id().expect("instance"))
        .with_request_id(ids.mint_request_id().expect("request"))
        .with_correlation_id(ids.mint_correlation_id().expect("correlation"))
        .with_task_id(ids.mint_task_id().expect("task"))
        .with_run_id(run);
    let completed = |step_index| TaskSemanticFact::EffectCompleted {
        step_index,
        operation_label: OPERATION.to_owned(),
    };
    let refused = |step_index| TaskSemanticFact::EffectRefused {
        step_index,
        operation_label: OPERATION.to_owned(),
        target_id: "marker/refused".to_owned(),
    };
    let admitted = [
        task_fact(&ids, &links, 0, completed(2)),
        task_fact(&ids, &links, 1, refused(2)),
    ];
    let expected = admitted
        .iter()
        .map(|draft| *draft.event_id())
        .collect::<Vec<_>>();
    let out_of_order = task_fact(&ids, &links, 2, refused(3));
    in_one_batch(&root, &ledger, || {
        for draft in admitted.into_iter().chain([out_of_order]) {
            ledger.append_deferred(draft).expect("accepted");
        }
    });
    let confirmed = ledger.confirm_deferred(BARRIER_WAIT).expect("confirmation");
    assert_eq!(
        confirmed
            .committed
            .iter()
            .map(|reference| *reference.event_id())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        confirmed
            .failed
            .iter()
            .map(|error| error.code())
            .collect::<Vec<_>>(),
        [
            actingcommand_contract::codes::SanitizationCode::InvalidTaskEffect
                .code()
                .as_str()
        ]
    );
    assert_eq!(
        ledger
            .query(EventQuery {
                run_id: Some(*run.transport()),
                ..EventQuery::default()
            })
            .expect("the run's records")
            .iter()
            .map(|event| *event.event_id())
            .collect::<Vec<_>>(),
        expected
    );
    ledger.close().expect("close");
}

// T5: the settlement continuation reads its chain from the store.
#[test]
fn ln5_t5_the_settlement_continuation_reads_its_chain_from_the_store() {
    let root = TempDir::new().expect("state root");
    let drafts = scheduled_recovery_drafts();
    let ledger = open(&root, "ln5-settlement");
    let (_, execution) = append_scheduled_recovery_prefix(&ledger, &drafts);
    ledger.close().expect("close");

    let reopened = open(&root, "ln5-settlement-reopened");
    let before = reopened.latest_sequence().expect("head");
    let completion =
        reconcile_scheduled_settlement(&reopened, &drafts.execution).expect("the settlement");
    assert_eq!(completion.event_type(), EventType::PolicyDispatchCompleted);
    assert_eq!(completion.sequence(), before + 1);
    assert_eq!(completion.links().run_id(), execution.links().run_id());
    assert_eq!(completion.links().lease_id(), execution.links().lease_id());
    assert_eq!(
        reconcile_scheduled_settlement(&reopened, &drafts.execution).expect("the settlement again"),
        completion,
        "a second call returns the same completion"
    );
    assert_eq!(
        reopened.latest_sequence().expect("head"),
        before + 1,
        "the second call appends nothing"
    );
    reopened.close().expect("close the reopened writer");

    let missing_root = TempDir::new().expect("state root");
    let missing = open(&missing_root, "ln5-settlement-missing-lease");
    let drafts = scheduled_recovery_drafts();
    missing.append(drafts.intent).expect("intent");
    missing.append(drafts.admission).expect("admission");
    missing.append(drafts.execution.clone()).expect("execution");
    missing.close().expect("close");
    let reopened = open(&missing_root, "ln5-settlement-missing-lease-reopened");
    expect_recovered_execution_error(
        reopened,
        drafts.execution,
        "scheduled_execution_recovery_lease_not_unique",
    );
}
