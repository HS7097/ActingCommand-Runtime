// SPDX-License-Identifier: AGPL-3.0-only

//! #422 W1-8 (brief rd-1028/brief-wave1.md W1-8; #422 6463703245, 6461256427 L-4's follow-up):
//! the published read point and the reader pool's page times, the host's part.
//!
//! - T7 (P30): `Status`, `MonitorStatus` and `RuntimeFactSnapshot` read the ledger head the
//!   writer last published, not the writer's: while the writer is held in a read of its own (the
//!   read hold gate, a 3 s reply limit), each answers `Completed`, all at the one published head;
//!   nothing latches and no stop is requested.
//! - T8 (P29): `PerformanceStatus` shows the ledger reader pool's pages while the Runtime runs
//!   (`ledger_read_pages`): the `QueryEvents` pages the pool read are counted, with their times in
//!   order, and the status keeps its shape rules.
//!
//! The writer is held by the ledger's read hold gate, which the test releases; nothing sleeps.

use super::*;
use actingcommand_ledger::WriterReadHoldGate;

/// T7: how long a caller waits for one writer reply; a read that waited for the held writer would
/// fail within the test.
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// The bound of the test's wait for the read hold; it only ends a broken run.
const GATE_WAIT: Duration = Duration::from_secs(30);
/// T8: the pages read.
const T8_PAGES: u64 = 3;

fn host(root: &TempDir) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_frame_retention_enabled(false)
            .with_ledger_reply_timeout(REPLY_TIMEOUT),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

fn read(host: &RuntimeHost, ids: &IdentifierIssuer, operation: RuntimeOperation) -> RuntimeReceipt {
    let receipt = host
        .process_request_for_test(
            &runtime_request(ids, operation),
            ConnectionId::new(1).expect("connection"),
        )
        .expect("a receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    receipt
}

#[test]
fn w1_8_t7_status_monitor_and_fact_snapshot_answer_at_the_published_head_while_the_writer_is_held()
{
    let root = TempDir::new().expect("tempdir");
    let host = host(&root);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let hold = WriterReadHoldGate::arm(root.path()).expect("read hold");
    let heads = thread::scope(|scope| {
        let host = &host;
        // The writer is held in a read of its head on the writer, a whole-history read's first;
        // that read's own answer is not this test's subject.
        let held = scope.spawn(move || host.query_persisted_events_for_test(EventQuery::default()));
        assert!(hold.wait_held(GATE_WAIT), "the writer is in a read");
        let status = read(host, &ids, RuntimeOperation::Status);
        let Some(RuntimeResult::Status { status }) = status.result() else {
            panic!("a status: {status:?}");
        };
        let monitor = read(host, &ids, RuntimeOperation::MonitorStatus);
        let Some(RuntimeResult::MonitorStatus { status: monitor }) = monitor.result() else {
            panic!("a monitor status: {monitor:?}");
        };
        let facts = read(host, &ids, RuntimeOperation::RuntimeFactSnapshot);
        let Some(RuntimeResult::RuntimeFactSnapshot { snapshot, .. }) = facts.result() else {
            panic!("a fact snapshot: {facts:?}");
        };
        let heads = [
            status
                .source()
                .expect("the status' source")
                .ledger_committed_head,
            monitor
                .source()
                .expect("the monitor status' source")
                .ledger_committed_head,
            snapshot.ledger_position,
        ];
        assert!(
            hold.wait_held(Duration::ZERO),
            "the writer was still held in its read"
        );
        hold.release();
        let _read = held.join().expect("the read's thread");
        heads
    });
    // No commit runs while the writer is held, so the three name the one published head.
    assert!(heads[0] > 0, "the start's records are committed: {heads:?}");
    assert_eq!(heads, [heads[0]; 3], "one published head: {heads:?}");
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown flag"),
        "no stop requested"
    );
    host.close().expect("close host");
}

#[test]
fn w1_8_t8_the_performance_status_shows_the_reader_pools_pages() {
    let root = TempDir::new().expect("tempdir");
    let host = host(&root);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let pages = |host: &RuntimeHost| {
        let receipt = read(host, &ids, RuntimeOperation::PerformanceStatus);
        let Some(RuntimeResult::PerformanceStatus { status }) = receipt.result() else {
            panic!("a performance status: {receipt:?}");
        };
        status.validate().expect("the status keeps its shape rules");
        status
            .ledger_read_pages
            .expect("the Runtime's ledger has a reader pool")
    };
    let before = pages(&host);
    for _ in 0..T8_PAGES {
        read(
            &host,
            &ids,
            RuntimeOperation::QueryEvents {
                query: EventQuery::default(),
                profile: ProjectionProfile::Forensic,
                page: RuntimeEventQueryPageRequest::new(10, None).expect("a first page"),
            },
        );
    }
    let after = pages(&host);
    // The Runtime's own reads may add pages of their own; the requests' pages are among them.
    assert!(
        after.pages >= before.pages + T8_PAGES,
        "each QueryEvents page is counted: {before:?} {after:?}"
    );
    assert!(
        after.last_ms <= after.longest_ms && after.longest_ms <= after.total_ms,
        "{after:?}"
    );
    host.close().expect("close host");
}
