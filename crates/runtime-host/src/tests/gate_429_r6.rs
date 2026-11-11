// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-6, first slice (register §1.302; contracts/runtime-interface.md, "Workbench
// request kinds", the row `StopInstanceRun`; coordinator work order rd-1111/brief-r6.md, option C):
// a client's direct run is stopped and the Runtime resets the device itself, with the run's own
// lease; a host run and a policy run are refused with their origin. Through the host, on a fake
// clock and a fake device:
// - a stopped direct run ends `contained_task_cancelled` (ClientRequested); the Runtime commits one
//   `InputReset` on the run's lease before that lease's release; a later stop answers settled with
//   `reset: done` and no cause; the routine summary counts an interruption and no failure; the
//   instance is neither fenced nor busy afterwards, and the Runtime is not halted;
// - the requesting client disconnects right after its stop is sent: the reset still happens;
// - a host run and a policy run are Denied `scheduled_contained_task_not_client_cancellable` with
//   their `run_origin` and request, recorded, and nothing asked them to stop; a run the instance
//   neither runs nor stopped is Denied `contained_task_cancellation_identity_missing`;
// - `wait_ms` elapses while the run is held at its checkpoint: `settled: false` and no `reset`,
//   and the stop stays asked (the run ends cancelled);
// - the reset fails: `reset: failed` with the fence as the receipt's cause, that instance fenced,
//   the other instance untouched, the Runtime not halted;
// - a run that ended on its own after the stop reached it (past its last check) needed no reset:
//   `reset: not_needed`, nothing reset, as the client's own cancel-and-reset answers `not_needed`
//   for a run that did not end cancelled by its client (since #402 HS-4 every touch backend
//   performs a reset, so no backend answers it).
// Every alias, id and package is neutral.

use super::gate_369s5::direct_run_package;
use super::*;
use crate::host::ContainedTaskCheckpointPoint;
use actingcommand_contract::{
    ContainedTaskCancellationReason, EventAction, InstanceRunReset, InstanceRunStop, RequestId,
    RoutineSummaryGranularity,
};
use serde_json::json;
use std::net::SocketAddr;

const ALIAS: &str = "r6.instance.a";
const OTHER: &str = "r6.instance.b";
/// 2026-10-26 07:01 UTC.
const START_UNIX_MS: u64 = 1_792_998_060_000;
/// How long a test waits for something the host does on another thread.
const WAIT: Duration = Duration::from_secs(30);

/// A host on a fake clock with the instances `entries`.
fn start_host(root: &TempDir, entries: Vec<(&str, InstanceId, Arc<FakeState>)>) -> RuntimeHost {
    let clock: Arc<dyn RuntimeClock> = Arc::new(ManualRuntimeClock::new(START_UNIX_MS, 0));
    let entries = entries
        .into_iter()
        .map(|(alias, instance, state)| (alias.to_owned(), instance, state));
    let host = RuntimeHost::start(
        config(root).with_runtime_clock(clock),
        Arc::new(FakeProvider::from_entries(entries)),
    )
    .expect("runtime host");
    host.routine_summary_set_local_offset_for_test(0)
        .expect("the host's zone");
    host
}

/// `StopInstanceRun` of `run` on `alias`, waiting up to `wait_ms`, on a new connection.
fn stop(address: SocketAddr, alias: &str, run: RequestId, wait_ms: u64) -> RuntimeReceipt {
    let mut client = TestClient::connect_address(address);
    client.set_receipt_read_timeout();
    let request = client.request(RuntimeOperation::StopInstanceRun {
        instance_alias: alias.to_owned(),
        request_id: run,
        wait_ms,
    });
    client.send(&request)
}

/// The stop a completed receipt answers.
fn stopped(receipt: &RuntimeReceipt) -> InstanceRunStop {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        "request_stop_instance_run_completed"
    );
    let Some(RuntimeResult::InstanceRunStopped { stopped }) = receipt.result() else {
        panic!("a stop: {receipt:?}");
    };
    *stopped
}

/// A direct run of the neutral package on `alias` whose `at_checkpoint` runs on the run's own
/// thread at `point`, given the run's request: the run's request and its receipt.
fn run_with<F>(
    host: &RuntimeHost,
    alias: &str,
    instance: InstanceId,
    package: &str,
    point: ContainedTaskCheckpointPoint,
    at_checkpoint: F,
) -> (RequestId, RuntimeReceipt)
where
    F: FnOnce(RequestId) + Send + 'static,
{
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            alias,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    );
    let run = request.request_id();
    let checkpoint = host
        .run_at_request_checkpoint_for_test(point, run, instance, move |_| at_checkpoint(run))
        .expect("arm the run's checkpoint");
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(1_601).expect("connection"))
        .expect("the run's receipt");
    assert_eq!(checkpoint.consumed(), 1, "the run reached its checkpoint");
    (run, receipt)
}

/// The ledger sequences of the run's committed resets and of its lease's releases.
fn resets_and_releases(host: &RuntimeHost, run: RequestId) -> (Vec<u64>, Vec<u64>) {
    let events = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(run),
            ..EventQuery::default()
        })
        .expect("the run's records");
    let resets = events
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Input(InputPayload::Committed(detail))
                    if detail.action() == EventAction::InputReset
            )
        })
        .map(PersistedEvent::sequence)
        .collect();
    let releases = events
        .iter()
        .filter(|event| event.event_type() == EventType::LeaseReleased)
        .map(PersistedEvent::sequence)
        .collect();
    (resets, releases)
}

/// The resets the fake device was asked for.
fn device_resets(state: &FakeState) -> usize {
    state
        .input_actions
        .lock()
        .expect("fake input actions")
        .iter()
        .filter(|action| matches!(action, InputAction::Reset))
        .count()
}

/// The status rows of the instances.
fn instances(address: SocketAddr) -> Vec<actingcommand_contract::RuntimeInstanceStatus> {
    let mut reader = TestClient::connect_address(address);
    let request = reader.request(RuntimeOperation::Status);
    let receipt = reader.send(&request);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("the status: {receipt:?}");
    };
    status.instances().to_vec()
}

/// The status row of `alias`.
fn instance(address: SocketAddr, alias: &str) -> actingcommand_contract::RuntimeInstanceStatus {
    instances(address)
        .into_iter()
        .find(|row| row.instance_alias() == alias)
        .unwrap_or_else(|| panic!("{alias} is listed"))
}

#[test]
fn r6_stopping_a_direct_run_cancels_it_and_the_runtime_resets_it_before_its_lease_ends() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let a = instance_id();
    let host = start_host(&root, vec![(ALIAS, a, Arc::clone(&state))]);
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let (sent, answered) = mpsc::channel();
    let (run, receipt) = run_with(
        &host,
        ALIAS,
        a,
        &package,
        ContainedTaskCheckpointPoint::PackageAdmitted,
        move |run| {
            sent.send(stop(address, ALIAS, run, 0))
                .expect("the stop's answer");
        },
    );

    // Asked while the run waits at its checkpoint: still stopping.
    let first = answered.recv_timeout(WAIT).expect("the stop's answer");
    assert_eq!(
        stopped(&first),
        InstanceRunStop {
            request_id: run,
            settled: false,
            reset: None,
        }
    );
    assert!(first.terminal().is_some(), "the stop is recorded");
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCancelled {
                reason: ContainedTaskCancellationReason::ClientRequested,
                ..
            })
        ),
        "{receipt:?}"
    );

    // One reset with the run's lease, before that lease's release.
    let (resets, releases) = resets_and_releases(&host, run);
    assert_eq!(resets.len(), 1, "one reset: {resets:?}");
    assert_eq!(releases.len(), 1, "one release: {releases:?}");
    assert!(resets[0] < releases[0], "{resets:?} {releases:?}");
    assert_eq!(device_resets(&state), 1);

    // The stop asked again: settled, the reset done, no cause.
    let again = stop(address, ALIAS, run, 0);
    assert_eq!(
        stopped(&again),
        InstanceRunStop {
            request_id: run,
            settled: true,
            reset: Some(InstanceRunReset::Done),
        }
    );
    assert!(again.outcome().causes().is_empty(), "{again:?}");

    // An interruption, no failure; nothing holds or fences the instance.
    let mut reader = TestClient::connect_address(address);
    let request = reader.request(RuntimeOperation::RoutineSummary {
        instance: Some(ALIAS.to_owned()),
        granularity: RoutineSummaryGranularity::Day,
        since_revision: None,
    });
    let summary = reader.send(&request);
    let Some(RuntimeResult::RoutineSummary { summary }) = summary.result() else {
        panic!("a routine summary: {summary:?}");
    };
    let task = &summary.instances[0].buckets[0].tasks[0];
    assert_eq!(
        (task.successes, task.failures.len(), task.interruptions),
        (0, 0, 1),
        "{summary:?}"
    );
    let row = instance(address, ALIAS);
    assert!(row.activity().is_none(), "{row:?}");
    assert!(row.resource_fence().is_none(), "{row:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(reader);
    host.close().expect("close host");
}

#[test]
fn r6_the_reset_happens_after_the_requesting_client_disconnects() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let a = instance_id();
    let host = start_host(&root, vec![(ALIAS, a, Arc::clone(&state))]);
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let asked = host
        .notify_run_stops_for_test()
        .expect("the stops' notices");
    let (run, receipt) = run_with(
        &host,
        ALIAS,
        a,
        &package,
        ContainedTaskCheckpointPoint::PackageAdmitted,
        move |run| {
            // The stop is sent and its client gone before the Runtime answers it.
            let mut requester = TestClient::connect_address(address);
            let request = requester.request(RuntimeOperation::StopInstanceRun {
                instance_alias: ALIAS.to_owned(),
                request_id: run,
                wait_ms: 30_000,
            });
            write_frame(
                &mut requester.stream,
                &request,
                DEFAULT_RUNTIME_MAX_FRAME_BYTES,
            )
            .expect("the stop is sent");
            drop(requester);
            // The run goes on once the stop is asked.
            let _ = asked.recv_timeout(WAIT).expect("the stop is asked");
        },
    );
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCancelled { .. })
        ),
        "{receipt:?}"
    );
    let (resets, releases) = resets_and_releases(&host, run);
    assert_eq!(resets.len(), 1, "one reset: {resets:?}");
    assert_eq!(releases.len(), 1, "one release: {releases:?}");
    assert!(resets[0] < releases[0], "{resets:?} {releases:?}");
    assert_eq!(device_resets(&state), 1);
    assert_eq!(
        stopped(&stop(address, ALIAS, run, 0)),
        InstanceRunStop {
            request_id: run,
            settled: true,
            reset: Some(InstanceRunReset::Done),
        }
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn r6_a_host_run_and_a_policy_run_are_refused_with_their_origin_and_nothing_is_cancelled() {
    let root = TempDir::new().expect("tempdir");
    let a = instance_id();
    let host = start_host(&root, vec![(ALIAS, a, Arc::new(FakeState::default()))]);
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for (host_run, origin) in [(true, "host"), (false, "policy")] {
        let run = *ids.mint_request_id().expect("request id").transport();
        host.begin_unstoppable_run_for_test(run, a, host_run)
            .expect("a run no client may stop");
        let refused = stop(address, ALIAS, run, 1_000);
        assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
        assert_eq!(
            refused.outcome().code().as_str(),
            "scheduled_contained_task_not_client_cancellable",
            "{origin}"
        );
        assert_eq!(
            refused.outcome().value("run_origin"),
            Some(&json!(origin)),
            "{refused:?}"
        );
        assert_eq!(
            refused.outcome().value("request_id"),
            Some(&serde_json::to_value(run).expect("the run's id")),
            "{refused:?}"
        );
        assert!(refused.result().is_none(), "{origin}");
        assert!(
            refused.terminal().is_some(),
            "{origin}: the refusal is recorded"
        );
        assert_eq!(
            host.end_unstoppable_run_for_test(run)
                .expect("the run's registry"),
            Some(false),
            "{origin}: nothing asked the run to stop"
        );
    }

    let unknown = *ids.mint_request_id().expect("request id").transport();
    let refused = stop(address, ALIAS, unknown, 0);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        "contained_task_cancellation_identity_missing"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn r6_after_wait_ms_the_stop_answers_unsettled_without_a_reset() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let a = instance_id();
    let host = start_host(&root, vec![(ALIAS, a, Arc::clone(&state))]);
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let (sent, answered) = mpsc::channel();
    let (run, receipt) = run_with(
        &host,
        ALIAS,
        a,
        &package,
        ContainedTaskCheckpointPoint::PackageAdmitted,
        move |run| {
            // The run is held at its checkpoint for all of `wait_ms`.
            sent.send(stop(address, ALIAS, run, 50))
                .expect("the stop's answer");
        },
    );
    let answer = answered.recv_timeout(WAIT).expect("the stop's answer");
    assert_eq!(
        stopped(&answer),
        InstanceRunStop {
            request_id: run,
            settled: false,
            reset: None,
        }
    );
    let wire = serde_json::to_value(stopped(&answer)).expect("the stop encodes");
    assert!(wire.get("reset").is_none(), "{wire}");
    // The stop stayed asked: the run ended cancelled at its next check.
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCancelled { .. })
        ),
        "{receipt:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn r6_a_failed_reset_fences_only_its_instance_and_answers_failed() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let a = instance_id();
    let b = instance_id();
    let host = start_host(
        &root,
        vec![
            (ALIAS, a, Arc::clone(&state)),
            (OTHER, b, Arc::new(FakeState::default())),
        ],
    );
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let device = Arc::clone(&state);
    let (sent, answered) = mpsc::channel();
    let (run, receipt) = run_with(
        &host,
        ALIAS,
        a,
        &package,
        ContainedTaskCheckpointPoint::PackageAdmitted,
        move |run| {
            sent.send(stop(address, ALIAS, run, 0))
                .expect("the stop's answer");
            // The run's next input, the Runtime's reset, fails on the device.
            device.fail_input.store(true, Ordering::Release);
        },
    );
    let first = answered.recv_timeout(WAIT).expect("the stop's answer");
    assert!(!stopped(&first).settled, "{first:?}");
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCancelled { .. })
        ),
        "{receipt:?}"
    );

    let again = stop(address, ALIAS, run, 0);
    assert_eq!(
        stopped(&again),
        InstanceRunStop {
            request_id: run,
            settled: true,
            reset: Some(InstanceRunReset::Failed),
        }
    );
    let [cause] = again.outcome().causes() else {
        panic!("the reset's failure is the cause: {again:?}");
    };
    assert_eq!(
        cause.outcome().code().as_str(),
        "instance_resources_fenced",
        "{again:?}"
    );
    assert!(
        instance(address, ALIAS).resource_fence().is_some(),
        "the run's instance is fenced"
    );
    assert!(
        instance(address, OTHER).resource_fence().is_none(),
        "the other instance is untouched"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn r6_a_run_that_ended_on_its_own_needed_no_reset() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let a = instance_id();
    let host = start_host(&root, vec![(ALIAS, a, Arc::clone(&state))]);
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let (sent, answered) = mpsc::channel();
    // Past its last check: the run's steps are done and its end begins.
    let (run, receipt) = run_with(
        &host,
        ALIAS,
        a,
        &package,
        ContainedTaskCheckpointPoint::EndWork,
        move |run| {
            sent.send(stop(address, ALIAS, run, 0))
                .expect("the stop's answer");
        },
    );
    let first = answered.recv_timeout(WAIT).expect("the stop's answer");
    assert!(!stopped(&first).settled, "{first:?}");
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                ..
            })
        ),
        "{receipt:?}"
    );
    assert_eq!(
        stopped(&stop(address, ALIAS, run, 0)),
        InstanceRunStop {
            request_id: run,
            settled: true,
            reset: Some(InstanceRunReset::NotNeeded),
        }
    );
    let (resets, _) = resets_and_releases(&host, run);
    assert!(resets.is_empty(), "nothing was reset: {resets:?}");
    assert_eq!(device_resets(&state), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
