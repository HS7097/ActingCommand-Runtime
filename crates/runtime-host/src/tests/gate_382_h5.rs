// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 H-5 (test plan v2.2 §3.6): host wave B.
//
// G18 (inventory-host key 3; req-basis-ha H-5 G18): the start's process probe of a previous
// owner whose last journal record still declares resources in use, injected through
// `ScriptedProcessProbe` (seam I6). contracts/actingd-unlock-owner.md "When startup releases the
// owner itself" (Workflow #315 B2c-2): dead (no running process with the pid, or a running one
// created more than 2 s after the recorded start) releases the owner, records one
// `runtime.lifecycle_observed` `prior_epoch_owner_released_by_exit` with the pid, the start and
// the last disposition, and gives the daemon its `owner_resources_released_by_exit` line; alive
// (created at most 2 s after the start) keeps the `owner_resource_unconfirmed` refusal ending in
// `: pid <pid> alive`; a probe that cannot decide keeps it, ending in
// `: probe unknown: <reason>`, and is never read as dead.
//
// G4 (inventory-host key 10; req-basis-ha H-5 G4): the host side of #376. While an install drain
// is under way an event subscription is a refusable request like any other (m376 §5): it is
// Denied `RuntimeUnavailable`, not fatal, and the Runtime stays healthy; `is_lifecycle_draining`
// is true across Draining and Drained and false once the drain is aborted (INSTALL.md:166-172);
// after the Abort the same cursor subscribes again and reads on from where it stood (m376 M5).
// The `drain_timeout` row is not written: the drain deadline runs on the monotonic wall clock,
// and its clock seam (H-1-rest I2) is not built yet (coordinator ruling 7 of wave 12).
//
// G11 (no inventory key; host key 79; req-basis-ha H-5 G11, coordinator 10-09 on
// input-receipt-wait.md): the ledger writer stalls past a client's I/O budget, with the stall
// gate (LED-I3 = HOST-I4) holding the commit. The typed client gets
// `runtime_receipt_header_failed`, latches it and sends nothing more; the Runtime writes one
// terminal for the request; a new connection that sends the same request (same id, same payload)
// gets the receipt of that one terminal, with no second effect.
//
// G15 (inventory-host key 35; req-basis-ha H-5 G15 as corrected by the coordinator 10-09 15:3x):
// the start's runtime-fact recovery replays the newest complete `runtime.fact_snapshot` set and
// then reads only the runtime-fact events after it (Workflow #308 5d-1; Workflow #375). Seams
// (I5): `RuntimeHost::append_runtime_fact_snapshot_for_test` seals a set now, and
// `RuntimeHost::take_last_runtime_fact_replay_for_test` tells where the recovery read from and
// how many events it read. No wall-clock bound is asserted.

use super::*;
use crate::owner::ScriptedProcessProbe;
use actingcommand_contract::{
    InstallTransitionAction, InstallTransitionPhase, OWNER_JOURNAL_SCHEMA,
    OwnerResourceDisposition, RuntimeLifecyclePhase, RuntimePayload, RuntimeSubscriptionRequest,
    SubscriptionCursor,
};
use actingcommand_host_metrics::{HostMetricsCode, ProcessProbe};
use actingcommand_ledger::WriterStallGate;
use actingcommand_ledger::owner_journal::RuntimeOwnerRecord;

/// The previous owner's pid and the start its record names.
const OWNER_PID: u32 = 4_242;
const OWNER_STARTED_AT_UNIX_MS: u64 = 1_700_000_000_000;
/// The contract's pid-reuse tolerance: a process created later than this after the recorded
/// start is another incarnation of the pid.
const PID_REUSE_TOLERANCE_MS: u64 = 2_000;
/// Bounds a broken run's waits only; every wait returns as soon as its condition holds.
const GATE_WAIT: Duration = Duration::from_secs(120);

/// A state root whose owner journal ends in the previous owner's active record with resources
/// in use, as an owner that died mid-run leaves it.
fn root_with_owner_in_use() -> TempDir {
    let root = TempDir::new().expect("tempdir");
    let record = RuntimeOwnerRecord {
        schema_version: OWNER_JOURNAL_SCHEMA.to_owned(),
        revision: 1,
        owner_epoch: *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_owner_epoch()
            .expect("owner epoch")
            .transport(),
        pid: OWNER_PID,
        started_at_unix_ms: OWNER_STARTED_AT_UNIX_MS,
        active: true,
        active_instances: Vec::new(),
        closed_at_unix_ms: None,
        resource_disposition: Some(OwnerResourceDisposition::InUse),
        closed_by: None,
    };
    let mut line = serde_json::to_vec(&record).expect("owner record JSON");
    line.push(b'\n');
    fs::write(root.path().join("owner.lock"), line).expect("write the owner journal");
    root
}

/// Starts a Runtime on `root` with `answer` as the process probe's; the pids it asked about.
fn start_with_probe(
    root: &TempDir,
    answer: ProcessProbe,
) -> (RuntimeHostResult<RuntimeHost>, Vec<u32>) {
    let probe = ScriptedProcessProbe::install(answer);
    let started = RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    );
    let probed = probe.probed();
    drop(probe);
    (started, probed)
}

#[test]
fn gate_a_previous_owner_alive_or_undecided_keeps_the_refusal_with_the_probes_finding() {
    for (case, answer, state, suffix, cause) in [
        (
            "created at the recorded start",
            ProcessProbe::Running {
                created_at_unix_ms: OWNER_STARTED_AT_UNIX_MS,
            },
            "running",
            format!(": pid {OWNER_PID} alive"),
            None,
        ),
        (
            "created exactly the tolerance after the recorded start",
            ProcessProbe::Running {
                created_at_unix_ms: OWNER_STARTED_AT_UNIX_MS + PID_REUSE_TOLERANCE_MS,
            },
            "running",
            format!(": pid {OWNER_PID} alive"),
            None,
        ),
        (
            "a probe the operating system refused",
            ProcessProbe::Unknown(HostMetricsCode::ProcessQueryRefused.code()),
            "unknown",
            ": probe unknown: process_query_refused".to_owned(),
            Some("process_query_refused"),
        ),
    ] {
        let root = root_with_owner_in_use();
        let (started, probed) = start_with_probe(&root, answer);
        let Err(refused) = started else {
            panic!("{case}: the start takes the owner over");
        };
        assert_eq!(probed, [OWNER_PID], "{case}");
        assert_eq!(refused.code(), "owner_resource_unconfirmed", "{case}");
        let message = refused.to_string();
        assert!(message.ends_with(&suffix), "{case}: {message}");
        let outcome = refused.outcome();
        assert_eq!(
            outcome.value("owner_pid"),
            Some(&serde_json::json!(OWNER_PID)),
            "{case}: {outcome}"
        );
        assert_eq!(
            outcome.value("owner_probe_state"),
            Some(&serde_json::json!(state)),
            "{case}: {outcome}"
        );
        assert_eq!(
            outcome
                .causes()
                .first()
                .map(|link| link.outcome().code().as_str()),
            cause,
            "{case}: {outcome}"
        );
        // The refusal comes before the Runtime opens its ledger.
        assert!(
            !root.path().join(RUNTIME_STATE_DATABASE_FILE).exists(),
            "{case}"
        );
    }
}

#[test]
fn gate_a_previous_owner_gone_is_released_with_one_fact_and_the_daemon_line() {
    for (case, answer) in [
        ("no running process has the pid", ProcessProbe::NotRunning),
        (
            "the running process was created after the tolerance",
            ProcessProbe::Running {
                created_at_unix_ms: OWNER_STARTED_AT_UNIX_MS + PID_REUSE_TOLERANCE_MS + 1,
            },
        ),
    ] {
        let root = root_with_owner_in_use();
        let (started, probed) = start_with_probe(&root, answer);
        let host = started.unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(probed, [OWNER_PID], "{case}");
        let released = PriorOwnerReleasedByExit {
            pid: OWNER_PID,
            started_at_unix_ms: OWNER_STARTED_AT_UNIX_MS,
            last_disposition: OwnerResourceDisposition::InUse,
        };
        assert_eq!(host.owner_released_by_exit(), Some(released), "{case}");
        // actingd prints this after its `actingd ` prefix.
        assert_eq!(
            released.to_string(),
            format!(
                "owner_resources_released_by_exit pid={OWNER_PID} \
                 started_at_unix_ms={OWNER_STARTED_AT_UNIX_MS} last_disposition=in_use"
            ),
            "{case}"
        );
        let events = host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("ledger events");
        let facts = events
            .iter()
            .filter(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => matches!(
                    value.phase(),
                    RuntimeLifecyclePhase::PriorEpochOwnerReleasedByExit { .. }
                ),
                _ => false,
            })
            .collect::<Vec<_>>();
        let [fact] = facts.as_slice() else {
            panic!("{case}: one release fact, not {}", facts.len());
        };
        let EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) = fact.payload() else {
            panic!("{case}: a lifecycle observation");
        };
        assert_eq!(value.phase(), released.phase(), "{case}");
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close host");
    }
}

/// One page of every event from `cursor`, at most `max_events`, as the #376 policy monitor
/// subscribes.
fn subscribe(
    client: &mut TestClient,
    cursor: SubscriptionCursor,
    max_events: u16,
) -> RuntimeReceipt {
    let request = client.request(RuntimeOperation::SubscribeEvents {
        request: RuntimeSubscriptionRequest::new(
            EventQuery::default(),
            ProjectionProfile::Forensic,
            cursor,
            100,
            max_events,
        )
        .expect("subscription request"),
    });
    client.send(&request)
}

/// The install transition's status in `receipt`.
fn transition_status(receipt: &RuntimeReceipt) -> &actingcommand_contract::InstallTransitionStatus {
    let Some(RuntimeResult::InstallTransition { status }) = receipt.result() else {
        panic!("an install transition status: {receipt:?}");
    };
    status
}

#[test]
fn gate_a_subscription_during_an_install_drain_is_refused_and_resumes_from_its_cursor_after_abort()
{
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let mut monitor = TestClient::connect(&host);
    let mut installer = TestClient::connect(&host);
    assert!(!host.is_lifecycle_draining().expect("draining state"));

    // Before the drain the monitor reads its first event and keeps the cursor after it.
    let before = subscribe(&mut monitor, SubscriptionCursor::default(), 1);
    let Some(RuntimeResult::EventBatch { batch }) = before.result() else {
        panic!("an event batch before the drain: {before:?}");
    };
    let [first] = batch.events() else {
        panic!("one event before the drain: {:?}", batch.events());
    };
    let first_sequence = first.sequence;
    let cursor = batch.next_cursor();

    installer.declare_governance_identity();
    let begin = installer.governance_request(RuntimeOperation::InstallTransition {
        target: host.runtime_info().shutdown_target(),
        action: InstallTransitionAction::BeginDrain {
            transition_id: "h5-g4-drain".to_owned(),
            timeout_ms: 600_000,
        },
    });
    let begun = installer.send(&begin);
    let status = transition_status(&begun);
    assert!(
        matches!(
            status.phase,
            InstallTransitionPhase::Draining | InstallTransitionPhase::Drained
        ),
        "{begun:?}"
    );
    let ticket = status.ticket.clone();
    assert!(host.is_lifecycle_draining().expect("draining state"));

    // During the drain the subscription is a refused request, not a fatal one.
    let refused = subscribe(&mut monitor, cursor, 1);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    let projection = refused.error_projection().expect("refusal projection");
    assert_eq!(projection.code, RuntimeErrorCode::RuntimeUnavailable);
    assert!(!projection.fatal, "{refused:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert!(host.is_lifecycle_draining().expect("draining state"));

    let abort = installer.governance_request(RuntimeOperation::InstallTransition {
        target: host.runtime_info().shutdown_target(),
        action: InstallTransitionAction::Abort { ticket },
    });
    let aborted = installer.send(&abort);
    assert_eq!(
        transition_status(&aborted).phase,
        InstallTransitionPhase::Aborted,
        "{aborted:?}"
    );
    assert!(!host.is_lifecycle_draining().expect("draining state"));

    // After the Abort the same cursor subscribes again and reads on after the first event.
    let resumed = subscribe(&mut monitor, cursor, 1);
    let Some(RuntimeResult::EventBatch { batch }) = resumed.result() else {
        panic!("an event batch after the abort: {resumed:?}");
    };
    let [next] = batch.events() else {
        panic!("the next event after the abort: {:?}", batch.events());
    };
    assert!(next.sequence > first_sequence, "{next:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(monitor);
    drop(installer);
    host.close().expect("close host");
}

/// A client action, the request whose one effect is its own record.
fn client_action(target: &str) -> ClientActionRecord {
    ClientActionRecord::new("settings", target, ClientActionKind::Button, None, None)
        .expect("client action")
}

/// Records one more client action on a new connection and returns its terminal's sequence. The
/// ledger has one writer, which commits in order: once this receipt is back, every commit held
/// before it has finished.
fn write_barrier(host: &RuntimeHost) -> u64 {
    let mut barrier = TestClient::connect(host);
    let request = barrier.request(RuntimeOperation::RecordClientAction {
        action: client_action("barrier"),
    });
    let receipt = barrier.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    receipt.terminal().expect("the barrier's terminal").sequence
}

/// The ledger's `client.action` records, in order.
fn client_actions(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::ClientAction),
        ..EventQuery::default()
    })
    .expect("ledger events")
}

#[test]
fn gate_a_stalled_writer_latches_the_typed_client_and_its_request_has_one_terminal() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let client = actingcommand_runtime_client::RuntimeClient::connect(
        actingcommand_runtime_client::RuntimeClientConfig::new(
            root.path(),
            EventActor::Cli,
            EventSource::Cli,
        )
        .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("connect the typed client");
    let gate = WriterStallGate::arm(root.path()).expect("arm the writer stall gate");

    let Err(stalled) = client.record_client_action(client_action("first")) else {
        panic!("the receipt arrived while the writer was stalled");
    };
    assert!(
        gate.wait_held_commit(GATE_WAIT),
        "no commit reached the gate"
    );
    assert_eq!(
        stalled.code().as_str(),
        "runtime_receipt_header_failed",
        "{stalled}"
    );
    // Latched: the next call fails with the same error and sends nothing.
    let Err(latched) = client.record_client_action(client_action("second")) else {
        panic!("a latched client sent another request");
    };
    assert_eq!(latched.code(), stalled.code(), "{latched}");
    gate.release();
    drop(client);

    // The Runtime wrote one record of the first request and none of the second: besides the
    // barrier's own record, one client action.
    let barrier = write_barrier(&host);
    let actions = client_actions(&host);
    let [action, last] = actions.as_slice() else {
        panic!(
            "the stalled action and the barrier, not {} records",
            actions.len()
        );
    };
    assert_eq!(last.sequence(), barrier);
    assert!(action.sequence() < barrier);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn gate_the_same_request_on_a_new_connection_after_a_stall_gets_the_one_terminals_receipt() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let mut first = TestClient::connect(&host);
    let request = first.request(RuntimeOperation::RecordClientAction {
        action: client_action("stalled"),
    });
    let gate = WriterStallGate::arm(root.path()).expect("arm the writer stall gate");

    // The test client's own I/O budget runs out while the commit is held.
    assert!(
        first.send_result(&request).is_err(),
        "the receipt arrived while the writer was stalled"
    );
    assert!(
        gate.wait_held_commit(GATE_WAIT),
        "no commit reached the gate"
    );
    gate.release();
    drop(first);

    let barrier = write_barrier(&host);

    let mut second = TestClient::connect(&host);
    let replayed = second.send(&request);
    assert_eq!(
        replayed.state(),
        RuntimeReceiptState::Completed,
        "{replayed:?}"
    );
    // The stalled request's one record, before the barrier's; the replay wrote nothing.
    let actions = client_actions(&host);
    let [action, last] = actions.as_slice() else {
        panic!(
            "the stalled action and the barrier, not {} records",
            actions.len()
        );
    };
    assert_eq!(last.sequence(), barrier);
    let terminal = replayed.terminal().expect("the receipt's terminal");
    assert_eq!(
        (terminal.sequence, terminal.event_id),
        (action.sequence(), *action.event_id())
    );
    assert_events_in_order_with_one_terminal(
        &event_types_for_request(
            &host,
            &IdentifierIssuer::new().expect("identifier issuer"),
            ConnectionId::new(279).expect("query connection"),
            request.request_id(),
        ),
        &[EventType::ClientAction],
        &[EventType::ClientAction, EventType::CommandRejected],
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(second);
    host.close().expect("close host");
}

#[test]
fn gate_runtime_fact_recovery_reads_only_after_the_newest_complete_snapshot_set() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    // The start recorded its runtime facts (its configuration among them); seal them.
    assert!(
        host.append_runtime_fact_snapshot_for_test()
            .expect("seal a runtime-fact snapshot set"),
        "the start recorded no runtime fact"
    );
    host.close().expect("close host");

    let ledger = super::instance_workers::closed_ledger_events(root.path());
    let newest_set = ledger
        .iter()
        .filter(|event| event.event_type() == EventType::RuntimeFactSnapshot)
        .map(PersistedEvent::sequence)
        .max()
        .expect("a sealed snapshot set");
    let runtime_fact =
        |event: &&PersistedEvent| event.origin().module() == OriginModule::RuntimeFacts;
    let records_before = ledger
        .iter()
        .filter(runtime_fact)
        .filter(|event| {
            event.sequence() < newest_set
                && matches!(
                    event.event_type(),
                    EventType::RuntimeFactRecorded | EventType::RuntimeFactInvalidated
                )
        })
        .count();
    assert!(records_before > 0, "no runtime-fact record before the set");
    let after_set = ledger
        .iter()
        .filter(runtime_fact)
        .filter(|event| event.sequence() > newest_set)
        .count();

    let _ = RuntimeHost::take_last_runtime_fact_replay_for_test();
    let restarted = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    assert_eq!(
        RuntimeHost::take_last_runtime_fact_replay_for_test(),
        Some((newest_set + 1, after_set)),
        "read only after the newest set ({records_before} records before it)"
    );
    assert!(restarted.fatal_error().expect("runtime health").is_none());
    restarted.close().expect("close the restarted host");
}
