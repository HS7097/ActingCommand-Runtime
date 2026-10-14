// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #378 SHUT-FU (host gate 73): the install CommitShutdown is decided by the same idle
// test as a shutdown request, so it lets lifecycle work in flight on other threads end before it
// is decided, as a shutdown request does since train 11, and only when nothing else refuses it.
// The work in flight is a client's queue poll that the queue-operation test hook holds after its
// snapshot; the settle observer reports each time a commit starts to wait. Every wait returns as
// soon as its message arrives; `GATE_WAIT` only bounds a broken run.

use super::*;
use actingcommand_contract::{
    InstallTransitionAction, InstallTransitionPhase, InstallTransitionTicket, RuntimeShutdownTarget,
};

const GATE_WAIT: Duration = Duration::from_secs(120);
const ALIAS: &str = "node.a";
const DRAIN_TIMEOUT_MS: u64 = 120_000;
const POLL_LIMIT: usize = 100_000;

// What the test hears while the installer's commit is in flight.
enum Commit {
    Waiting,
    Answered(RuntimeReceipt),
}

// The installer's control connection, with its governance identity declared, as acsetup's.
fn installer(host: &RuntimeHost) -> TestClient {
    let mut installer = TestClient::connect(host);
    installer.set_receipt_read_timeout();
    installer.declare_governance_identity();
    installer
}

fn install_request(
    installer: &TestClient,
    target: RuntimeShutdownTarget,
    action: InstallTransitionAction,
) -> RuntimeRequest {
    installer.request(RuntimeOperation::InstallTransition { target, action })
}

// Begins the drain `transition_id`; its ticket and the phase the begin answered.
fn begin_drain(
    installer: &mut TestClient,
    target: RuntimeShutdownTarget,
    transition_id: &str,
) -> (InstallTransitionTicket, InstallTransitionPhase) {
    let begin = install_request(
        installer,
        target,
        InstallTransitionAction::BeginDrain {
            transition_id: transition_id.to_owned(),
            timeout_ms: DRAIN_TIMEOUT_MS,
        },
    );
    let receipt = installer.send(&begin);
    let Some(RuntimeResult::InstallTransition { status }) = receipt.result() else {
        panic!("begin drain: {}", receipt.outcome().code().as_str());
    };
    (status.ticket.clone(), status.phase)
}

// Begins the drain and queries it until it is drained, as the installer does before it commits.
fn drained(
    installer: &mut TestClient,
    target: RuntimeShutdownTarget,
    transition_id: &str,
) -> InstallTransitionTicket {
    let (ticket, mut phase) = begin_drain(installer, target, transition_id);
    for _ in 0..POLL_LIMIT {
        if phase == InstallTransitionPhase::Drained {
            return ticket;
        }
        assert_eq!(phase, InstallTransitionPhase::Draining, "{transition_id}");
        let query = install_request(
            installer,
            target,
            InstallTransitionAction::Query {
                transition_id: transition_id.to_owned(),
            },
        );
        let receipt = installer.send(&query);
        let Some(RuntimeResult::InstallTransition { status }) = receipt.result() else {
            panic!("query drain: {}", receipt.outcome().code().as_str());
        };
        phase = status.phase;
    }
    panic!("{transition_id}: the idle Runtime did not drain");
}

// A client's poll of a queue entry, held by the test hook after its snapshot: lifecycle work in
// flight on the poll's connection thread until the control resumes it.
fn hold_a_queue_poll(
    host: &RuntimeHost,
) -> (
    QueueOperationTestControl,
    thread::JoinHandle<RuntimeReceipt>,
) {
    let queued_request_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_request_id()
        .expect("request id")
        .transport();
    let control = host
        .pause_queue_operation_after_snapshot_for_test(
            QueueOperationTestKind::Poll,
            queued_request_id,
        )
        .expect("install the queue poll hook");
    let mut poller = TestClient::connect(host);
    poller.set_receipt_read_timeout();
    let poll = poller.request(RuntimeOperation::PollQueuedLease { queued_request_id });
    let polling = thread::spawn(move || poller.send(&poll));
    control.wait_until_paused();
    (control, polling)
}

// Reports each wait of a shutdown request or commit as `Commit::Waiting` on `events`.
fn observe_settle(host: &RuntimeHost, events: &mpsc::Sender<Commit>) {
    let waiting = events.clone();
    host.observe_shutdown_settle_for_test(Box::new(move || {
        // The test may have stopped listening; then the report has no reader.
        let _ = waiting.send(Commit::Waiting);
    }))
    .expect("observe the shutdown settle");
}

// An idle Runtime, drained right after ready, accepts the installer's commit-shutdown that meets
// another connection's lifecycle work in flight: the commit waits for that work instead of
// answering install_not_drained, and once it ends the shutdown is admitted for the drained
// owner.
#[test]
fn shutfu_an_idle_runtime_accepts_the_install_commit_shutdown_right_after_ready() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::from_entries(Vec::new())),
    )
    .expect("runtime host");
    let target = host.runtime_info().shutdown_target();
    let mut installer = installer(&host);
    let ticket = drained(&mut installer, target, "shutfu-idle");
    let (poll, polling) = hold_a_queue_poll(&host);
    let (events, heard) = mpsc::channel();
    observe_settle(&host, &events);

    let commit = install_request(
        &installer,
        target,
        InstallTransitionAction::CommitShutdown { ticket },
    );
    let committing = thread::spawn(move || {
        let receipt = installer.send(&commit);
        // The test may have stopped listening; then the answer has no reader.
        let _ = events.send(Commit::Answered(receipt));
    });
    match heard
        .recv_timeout(GATE_WAIT)
        .expect("the commit waits or answers")
    {
        Commit::Waiting => {}
        Commit::Answered(receipt) => panic!(
            "the commit answered while the poll was in flight: {:?} {}",
            receipt.state(),
            receipt.outcome().code().as_str()
        ),
    }
    poll.resume();
    let accepted = loop {
        match heard.recv_timeout(GATE_WAIT).expect("the commit answers") {
            Commit::Waiting => {}
            Commit::Answered(receipt) => break receipt,
        }
    };
    committing.join().expect("the installer's commit");
    polling.join().expect("the client's poll");

    assert_eq!(
        accepted.state(),
        RuntimeReceiptState::Admitted,
        "{}",
        accepted.outcome().code().as_str()
    );
    assert!(
        matches!(
            accepted.result(),
            Some(RuntimeResult::ShutdownAccepted { target: admitted }) if *admitted == target
        ),
        "{:?}",
        accepted.result()
    );
    host.close().expect("close the Runtime");
}

// A Runtime whose lease is still held refuses the installer's commit-shutdown at once with
// install_not_drained, although another connection's lifecycle work is in flight: the commit
// never waits for that work.
#[test]
fn shutfu_a_runtime_holding_a_lease_refuses_the_install_commit_shutdown_at_once() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let target = host.runtime_info().shutdown_target();
    let mut owner = TestClient::connect(&host);
    let (_, token) = owner.acquire(ALIAS);
    let mut installer = installer(&host);
    let (ticket, phase) = begin_drain(&mut installer, target, "shutfu-held");
    assert_eq!(
        phase,
        InstallTransitionPhase::Draining,
        "the held lease keeps the drain open"
    );
    let (poll, polling) = hold_a_queue_poll(&host);
    let (events, heard) = mpsc::channel();
    observe_settle(&host, &events);

    let commit = install_request(
        &installer,
        target,
        InstallTransitionAction::CommitShutdown { ticket },
    );
    let refused = installer.send(&commit);
    let waits = heard.try_iter().count();
    poll.resume();
    polling.join().expect("the client's poll");

    assert_eq!(refused.state(), RuntimeReceiptState::Denied);
    assert_eq!(refused.outcome().code().as_str(), "install_not_drained");
    assert_eq!(waits, 0, "the commit waited for the poll in flight");
    assert!(!host.is_shutdown_requested().expect("runtime state"));

    let release = owner.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(owner.send(&release).state(), RuntimeReceiptState::Completed);
    drop(owner);
    drop(installer);
    host.close().expect("close the Runtime");
}
