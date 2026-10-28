// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-WS (brief rd-1026/brief-ln-ws.md; root cause rd-1026/writer-stall/root-cause.md §5;
//! the production stop of 2026-10-26 19:12:43): no read can make a write fail. An `ac_events`
//! read with a time window and no instance held the ledger's writer for 15 to 18 s; a policy
//! append waited out its reply limit behind it, `append_runtime_event` turned the timeout into a
//! fatal `ledger_failure` without its cause, and the Runtime stopped.
//!
//! The host's part:
//! - T6 (R5): each read of the incident's table, sent as `QueryEvents` while an append through
//!   `append_runtime_event` is in flight behind it: the 19:12:24 call (no instance, `changes`,
//!   the window, 100 rows), the 19:11:30 call (one instance, a 100 s window, 30 rows), the
//!   19:12:38 call (one instance, a 60 s window, 40 rows), a page of the run listing (one event
//!   type of one instance since the window's start, 10 rows) and a run evidence read (one run's
//!   records). The Runtime keeps running, the append commits, and each read answers a page,
//!   complete or incomplete at its budget. The host runs on a production-shaped synthetic ledger
//!   (LN-1 with LN-8d's frame shape, four instances in turn with the host's own in seven runs of
//!   twenty, three seconds between records, one inverted record), the shipped read limits, a
//!   writer clock that moves 1 ms each time it is read and a 600 s reply limit, so no real wait
//!   can matter;
//! - T7 (R4): an append the writer does not answer within its reply limit (the writer held in a
//!   read by the read hold gate, a 3 s reply limit) fails with `ledger_failure` whose cause names
//!   `writer_response_timeout`.

use super::lean_scale::{formal_root, scale_instance_id};
use super::*;
use actingcommand_contract::{LedgerPageLimit, LedgerView, outcome::Outcome};
use actingcommand_ledger::{
    LedgerCode, LedgerMonotonicClock, SyntheticInstance, SyntheticInversion, SyntheticLedgerSpec,
    WriterReadHoldGate, write_synthetic_ledger,
};
use actingcommand_runtime_database::RuntimeDatabase;

/// The history: 3 000 records, one frame in sixteen stored and live (LN-8d), three seconds
/// between records from the synthetic ledger's first time (2025-06-15).
const RECORDS: u64 = 3_000;
const LIVE_FRAME_EVERY: u64 = 16;
const FIRST_MS: u64 = 1_749_945_600_000;
const RECORD_INTERVAL_MS: u64 = 3_000;
/// The instances in turn, one letter per run: `a`, the host's own, runs seven runs of every
/// twenty.
const INSTANCE_TURNS: &str = "abcadbacdabcadbacdab";
/// T6: every caller waits this long for one writer reply, and a read's budget is the shipped
/// `ledger_observer.read_budget_ms`.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);
const READ_BUDGET: Duration = Duration::from_millis(1_000);
/// T7: the reply limit the held writer makes the append wait out.
const T7_REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// The bound of the test's waits for the read hold; it only ends a broken run.
const GATE_WAIT: Duration = Duration::from_secs(30);

/// A writer clock in milliseconds that moves 1 ms each time it is read.
fn stepping_clock() -> LedgerMonotonicClock {
    let now = Arc::new(AtomicU64::new(0));
    Arc::new(move || now.fetch_add(1, Ordering::SeqCst))
}

fn instance_of(letter: char) -> InstanceId {
    if letter == 'a' {
        return scale_instance_id();
    }
    serde_json::from_str(&format!("\"instance_{letter:0>32}\"")).expect("a fixture instance id")
}

/// The reads' window: from 90 % of the history to 95 %.
fn window() -> (u64, u64) {
    let at = |share: u64| FIRST_MS + RECORD_INTERVAL_MS * (RECORDS * share / 100);
    (at(90), at(95))
}

/// A closed formal root with the production-shaped history.
fn synthetic_root() -> TempDir {
    let root = TempDir::new().expect("the state root");
    formal_root(root.path());
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
    let instances = INSTANCE_TURNS
        .chars()
        .map(|letter| SyntheticInstance {
            alias: format!("fixture-instance-{letter}"),
            instance_id: instance_of(letter),
        })
        .collect();
    let mut spec = SyntheticLedgerSpec::new(RECORDS, instances);
    spec.first_timestamp_unix_ms = FIRST_MS;
    spec.record_interval_ms = RECORD_INTERVAL_MS;
    spec.mix.frame_artifact_every_frames = LIVE_FRAME_EVERY;
    spec.inversion = Some(SyntheticInversion {
        record: RECORDS / 2,
        timestamp_unix_ms: window().0 + RECORD_INTERVAL_MS / 2,
    });
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    assert_eq!(generated.records, RECORDS, "{generated:?}");
    drop(database);
    root
}

/// One approval record appended through `append_runtime_event`, as any synchronous append of the
/// Runtime's own is.
fn append_approval(host: &RuntimeHost, index: u64) -> RuntimeHostResult<()> {
    host.append_approval_event_for_test(
        EventSource::Adapter,
        EventActor::Agent,
        ApprovalDecisionRecord::new(
            format!("approval:ln-ws-fixture-{index}"),
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: format!("sha256:{}", "c".repeat(64)),
                catalog_version: 1,
            },
            actingcommand_contract::ApprovalReasonCode::new("agent_claimed_approval")
                .expect("approval reason"),
        )
        .expect("approval decision"),
    )
}

fn query_events(ids: &IdentifierIssuer, query: EventQuery, limit: u16) -> RuntimeRequest {
    runtime_request(
        ids,
        RuntimeOperation::QueryEvents {
            query,
            profile: ProjectionProfile::Forensic,
            page: RuntimeEventQueryPageRequest::new(limit, None).expect("a first page"),
        },
    )
}

#[test]
fn ln_ws_t6_the_incident_reads_answer_while_an_append_commits_and_the_runtime_runs() {
    let root = synthetic_root();
    let host = RuntimeHost::start(
        config(&root)
            .with_frame_retention_enabled(false)
            .with_ledger_reply_timeout(REPLY_TIMEOUT)
            .with_ledger_read_budget_for_test(READ_BUDGET, stepping_clock()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            scale_instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the Runtime host starts on the synthetic ledger");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let own = scale_instance_id();
    let (since, until) = window();
    let run = *host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::TaskRequested),
            instance_id: Some(own),
            ..EventQuery::default()
        })
        .expect("the instance's requested tasks")
        .last()
        .and_then(|event| event.links().run_id())
        .expect("a requested task with its run");
    let reads = [
        (
            "19:12:24, no instance",
            EventQuery {
                view: Some(LedgerView::Changes),
                from_timestamp_unix_ms: Some(since),
                to_timestamp_unix_ms: Some(until),
                ..EventQuery::default()
            },
            100,
        ),
        (
            "19:11:30, one instance",
            EventQuery {
                instance_id: Some(own),
                from_timestamp_unix_ms: Some(since + 30_000),
                to_timestamp_unix_ms: Some(since + 130_000),
                ..EventQuery::default()
            },
            30,
        ),
        (
            "19:12:38, one instance",
            EventQuery {
                instance_id: Some(own),
                from_timestamp_unix_ms: Some(since + 300_000),
                to_timestamp_unix_ms: Some(since + 360_000),
                ..EventQuery::default()
            },
            40,
        ),
        (
            "a run listing page",
            EventQuery {
                event_type: Some(EventType::TaskRequested),
                instance_id: Some(own),
                from_timestamp_unix_ms: Some(since),
                ..EventQuery::default()
            },
            10,
        ),
        (
            "a run evidence read",
            EventQuery {
                run_id: Some(run),
                ..EventQuery::default()
            },
            100,
        ),
    ];
    let appends = reads.len();
    for (index, (read, query, limit)) in reads.into_iter().enumerate() {
        let index = index as u64;
        let request = query_events(&ids, query, limit);
        let hold = WriterReadHoldGate::arm(root.path()).expect("read hold");
        let (receipt, appended) = thread::scope(|scope| {
            let request = &request;
            let host = &host;
            let answered = scope.spawn(move || {
                host.process_request_for_test(
                    request,
                    ConnectionId::new(100 + index).expect("connection"),
                )
            });
            assert!(hold.wait_held(GATE_WAIT), "{read}: the writer is in a read");
            let appending = scope.spawn(move || append_approval(host, index));
            hold.release();
            (
                answered.join().expect("the read's thread"),
                appending.join().expect("the append's thread"),
            )
        });
        appended.unwrap_or_else(|error| panic!("{read}: the append commits: {error:?}"));
        let receipt = receipt.unwrap_or_else(|error| panic!("{read}: a receipt: {error:?}"));
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{read}: {receipt:?}"
        );
        let Some(RuntimeResult::EventPage { page }) = receipt.result() else {
            panic!("{read}: an event page: {receipt:?}");
        };
        let scope = page.read_scope().expect("a Runtime page has its scope");
        assert!(
            scope.read_complete || scope.limits.contains(&LedgerPageLimit::ReadBudget),
            "{read}: the page is complete, or incomplete at its budget: {scope:?}"
        );
        assert!(
            host.fatal_error().expect("health").is_none(),
            "{read}: nothing latched"
        );
        assert!(
            !host.is_shutdown_requested().expect("shutdown flag"),
            "{read}: no stop requested"
        );
    }
    // Every append committed, and the Runtime still answers.
    let approvals = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            ..EventQuery::default()
        })
        .expect("the approvals");
    assert_eq!(approvals.len(), appends, "{approvals:?}");
    let status = host
        .process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::Status),
            ConnectionId::new(99).expect("connection"),
        )
        .expect("a status receipt");
    assert_eq!(status.state(), RuntimeReceiptState::Completed, "{status:?}");
    host.close().expect("close host");
}

#[test]
fn ln_ws_t7_an_append_not_answered_in_time_fails_naming_writer_response_timeout() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root)
            .with_frame_retention_enabled(false)
            .with_ledger_reply_timeout(T7_REPLY_TIMEOUT),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let hold = WriterReadHoldGate::arm(root.path()).expect("read hold");
    let appended = thread::scope(|scope| {
        let host = &host;
        // The writer is held in a read of the ledger's head on the writer, a whole-history read's
        // first (#422 W1-8, P30: a status no longer reads its head on the writer); the append is
        // captured behind it. That read's own answer is not this test's subject.
        let held = scope.spawn(move || host.query_persisted_events_for_test(EventQuery::default()));
        assert!(hold.wait_held(GATE_WAIT), "the writer is in a read");
        let appended = append_approval(host, 0);
        hold.release();
        let _read = held.join().expect("the read's thread");
        appended
    });
    let error = appended.expect_err("the append's reply did not come in time");
    assert_eq!(error.code(), "ledger_failure", "{error:?}");
    let outcome: Outcome = error.outcome();
    assert!(
        outcome.causes().iter().any(
            |link| link.outcome().code().as_str() == LedgerCode::WriterResponseTimeout.as_str()
        ),
        "the failure names the ledger's code: {outcome:?}"
    );
    host.close().expect("close host");
}
