// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #403 (approved 2026-10-13): an unconfirmed close of one instance's own resources
//! fences only that instance, hands it to the recovery ladder and waits for a person when the
//! ladder ends without proving the close; a whole-Runtime fatal stays for shared things. Every
//! wait returns as soon as what it waits for is recorded; the bound only stops a broken run.

use super::instance_workers::{phases, rung_outcomes, wait_for_ladder_end};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    FactPayload, InstanceResourceFence, RecoveryLadderOutcome, RecoveryRung, RecoveryRungOutcome,
    RecoveryTriggerStage, RuntimeLifecyclePhase, RuntimePayload,
};
use actingcommand_device::DeviceResourceQuiescence;

const WAIT: Duration = Duration::from_secs(120);
const FENCED_ALIAS: &str = "fence.instance.a";
const OTHER_ALIAS: &str = "fence.instance.b";

/// An application command that ends without confirming that its own resources (its adb child)
/// are gone: the effect is uncertain and the close unconfirmed.
fn unconfirmed_application_failure() -> DeviceError {
    DeviceError::fatal("synthetic application command left its child unconfirmed")
        .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1)
}

/// An application lifecycle command on `instance_alias`, as `actingctl` sends it.
fn application_stop(ids: &IdentifierIssuer, instance_alias: &str) -> RuntimeRequest {
    runtime_request(
        ids,
        RuntimeOperation::ApplicationLifecycle {
            instance_alias: instance_alias.to_owned(),
            holder_id: *ids.mint_holder_id().expect("holder").transport(),
            action: ApplicationLifecycleAction::Stop,
        },
    )
}

fn all_events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

/// The `runtime.failed` records of the fence code, with the outcome each carries.
fn fence_records(
    events: &[PersistedEvent],
) -> Vec<(&PersistedEvent, &actingcommand_contract::outcome::Outcome)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str()
                    == HostCode::InstanceResourcesFenced.as_str() =>
            {
                Some((event, record.outcome()))
            }
            _ => None,
        })
        .collect()
}

/// The lift records: the instance and the preparation that proved the close.
fn lift_records(events: &[PersistedEvent]) -> Vec<(&PersistedEvent, InstanceId, TerminalEvent)> {
    phases(events)
        .into_iter()
        .filter_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::ResourceFenceLifted {
                instance_id,
                preparation,
                ..
            } => Some((event, instance_id, preparation)),
            _ => None,
        })
        .collect()
}

/// The resource fence `Status` reports for `instance_alias`.
fn status_fence(host: &RuntimeHost, instance_alias: &str) -> Option<InstanceResourceFence> {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::Status),
            ConnectionId::new(190).expect("connection"),
        )
        .expect("status receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status result: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == instance_alias)
        .expect("the instance's status")
        .resource_fence()
        .cloned()
}

/// Asserts that `outcome` is the fence of `instance` caused by the application command's
/// unconfirmed failure.
fn assert_application_fence(
    outcome: &actingcommand_contract::outcome::Outcome,
    instance: InstanceId,
) {
    assert_eq!(
        outcome.value("instance_id"),
        Some(&serde_json::to_value(instance).expect("instance id")),
        "{outcome:?}"
    );
    assert!(
        outcome
            .codes()
            .iter()
            .any(|code| code.as_str() == SharedCode::ApplicationBackendOperationFailed.as_str()),
        "the fence names the unconfirmed failure: {outcome:?}"
    );
}

/// Workflow #403 (the 10-13 incident's shape): an application command on instance A ends with an
/// unconfirmed close while instance B has queued work. A is fenced with
/// `instance_resources_fenced`, recorded once and caused by the command's failure; the command
/// fails without a fatal projection; B's queued run is dispatched and completes; the Runtime has
/// no fatal state and answers; its status shows A's fence and none for B.
#[test]
fn gate_an_unconfirmed_application_close_fences_only_its_instance_and_the_other_runs() {
    let root = TempDir::new().expect("tempdir");
    let fenced_state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let (package, expected) = super::gate_369s5::direct_run_package(&root, &other_state);
    let fenced = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0))),
        Arc::new(FakeProvider::from_entries([
            (FENCED_ALIAS.to_owned(), fenced, Arc::clone(&fenced_state)),
            (
                OTHER_ALIAS.to_owned(),
                instance_id(),
                Arc::clone(&other_state),
            ),
        ])),
    )
    .expect("runtime host");
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(OTHER_ALIAS);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            OTHER_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package, expected).expect("task request"),
        ),
    );
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    *fenced_state
        .application_error
        .lock()
        .expect("application error") = Some(unconfirmed_application_failure());
    let application = application_stop(&ids, FENCED_ALIAS);

    let (fenced_receipt, run_receipt) = thread::scope(|scope| {
        let waiting = scope.spawn(|| {
            host.process_request_for_test(&run, ConnectionId::new(181).expect("connection"))
        });
        wait_until(WAIT, || {
            !host
                .query_persisted_events_for_test(EventQuery {
                    event_type: Some(EventType::SchedulerQueued),
                    request_id: Some(run.request_id()),
                    ..EventQuery::default()
                })
                .expect("queued run")
                .is_empty()
        });
        let fenced_receipt = host
            .process_request_for_test(&application, ConnectionId::new(182).expect("connection"))
            .expect("application receipt");
        assert_eq!(
            holder.send(&release).state(),
            RuntimeReceiptState::Completed
        );
        let run_receipt = waiting
            .join()
            .expect("waiting run thread")
            .expect("run receipt");
        (fenced_receipt, run_receipt)
    });

    assert_eq!(fenced_receipt.state(), RuntimeReceiptState::Failed);
    let projection = fenced_receipt
        .error_projection()
        .expect("the command's failure");
    assert!(!projection.fatal, "{projection:?}");
    assert_eq!(
        projection.host_code(),
        Some(HostCode::InstanceResourcesFenced.as_str())
    );
    assert_eq!(fenced_state.application_count.load(Ordering::Acquire), 1);
    assert_eq!(
        run_receipt.state(),
        RuntimeReceiptState::Completed,
        "{run_receipt:?}"
    );
    assert!(matches!(
        run_receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            ..
        })
    ));
    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(record, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_eq!(record.severity(), EventSeverity::Error);
    assert_eq!(record.links().instance_id(), Some(&fenced));
    assert_application_fence(outcome, fenced);
    let fence = status_fence(&host, FENCED_ALIAS).expect("A's fence in status");
    assert_eq!(
        fence.cause.as_str(),
        SharedCode::ApplicationBackendOperationFailed.as_str()
    );
    assert_eq!(status_fence(&host, OTHER_ALIAS), None);
    // With no rung that can prove the close (no startup package, no emulator control), the
    // fence's ladder ends unproven and A waits for a person; the Runtime still runs.
    wait_until(WAIT, || {
        status_fence(&host, FENCED_ALIAS).is_some_and(|fence| fence.awaiting_person)
    });
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(holder);
    host.close().expect("close host");
}

/// A held fenced instance `FENCED` (the policy fixture's instance, so its availability is gated
/// on its self-check) with a bound device endpoint and the neutral startup package; its opens
/// pass a preparation's self-check.
fn fenced_ladder_host(root: &TempDir, state: &Arc<FakeState>) -> (RuntimeHost, InstanceId) {
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let package = neutral_contained_task_package(true);
    let package_path = root.path().join("fence-startup-package.zip");
    fs::write(&package_path, &package).expect("write startup package");
    let startup = ContainedTaskRequest::new(
        package_path.to_string_lossy().into_owned(),
        format!("{:x}", Sha256::digest(&package)),
    )
    .and_then(|request| request.with_response_deadline_ms(60_000))
    .expect("startup package request");
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(root).with_startup_packages(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            startup,
        )])),
        Arc::new(
            FakeProvider::from_entries([(
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance,
                Arc::clone(state),
            )])
            .with_device_endpoints(),
        ),
    )
    .expect("runtime host");
    (host, instance)
}

/// The instance's policy availability (`session.instance.available`) in each publication, in
/// ledger order.
fn availability_publications(events: &[PersistedEvent]) -> Vec<(u64, bool)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => Some((event, payload.record())),
            _ => None,
        })
        .filter(|(_, record)| {
            record.key == crate::fact_store::POLICY_INSTANCE_AVAILABLE_KEY
                && record.scope
                    == FactScope::Instance {
                        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                    }
        })
        .filter_map(|(event, record)| match &record.content {
            FactContent::Inline {
                value: ContractFactValue::Boolean(value),
            } => Some((event.sequence(), *value)),
            _ => None,
        })
        .collect()
}

/// Workflow #403: the ladder proves the close. A's application command ends unconfirmed and A
/// is fenced; its ladder skips the return-home rung (it cannot prove a close), restarts the
/// application and prepares A again, and that preparation's passing self-check lifts the fence
/// with one record that names it. A's availability, withdrawn by the fence, follows its
/// self-check again, and status shows no fence: A schedules again.
#[test]
fn gate_a_ladder_whose_re_preparation_passes_lifts_the_fence() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let (host, instance) = fenced_ladder_host(&root, &state);
    *state.application_error.lock().expect("application error") =
        Some(unconfirmed_application_failure());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &application_stop(&ids, POLICY_INSTANCE_ALIAS),
            ConnectionId::new(183).expect("connection"),
        )
        .expect("application receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert!(!receipt.error_projection().expect("failure").fatal);
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(fenced, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_application_fence(outcome, instance);
    let trigger = phases(&events)
        .into_iter()
        .find_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. } => Some(trigger),
            _ => None,
        })
        .expect("the fence's ladder");
    assert_eq!(trigger.stage, RecoveryTriggerStage::ResourceFence);
    assert_eq!(
        trigger.failure_code.as_str(),
        SharedCode::ApplicationBackendOperationFailed.as_str()
    );
    assert_eq!(
        rung_outcomes(&events),
        vec![
            (
                RecoveryRung::ReturnHome,
                RecoveryRungOutcome::Skipped,
                Some(
                    HostCode::RecoveryRungSkippedResourceFence
                        .as_str()
                        .to_owned()
                ),
            ),
            (
                RecoveryRung::ApplicationRestart,
                RecoveryRungOutcome::Recovered,
                None,
            ),
        ]
    );
    let lifts = lift_records(&events);
    let [(lifted, lifted_instance, preparation)] = lifts.as_slice() else {
        panic!("one lift record: {lifts:#?}");
    };
    assert_eq!(*lifted_instance, instance);
    assert_eq!(lifted.severity(), EventSeverity::Info);
    assert!(fenced.sequence() < preparation.sequence);
    assert!(preparation.sequence < lifted.sequence());
    let proof = events
        .iter()
        .find(|event| event.sequence() == preparation.sequence)
        .expect("the proving preparation");
    assert!(matches!(
        phases(std::slice::from_ref(proof))
            .first()
            .map(|(_, phase)| phase.clone()),
        Some(RuntimeLifecyclePhase::InstancePreparationFinished {
            stage: RecoveryTriggerStage::RecoveryPreparation,
            outcome: None,
            ..
        })
    ));
    let availability = availability_publications(&events);
    assert!(
        availability.iter().any(|(sequence, available)| !available
            && *sequence > fenced.sequence()
            && *sequence < lifted.sequence()),
        "the fence withdraws A's availability: {availability:?}"
    );
    assert_eq!(
        availability.last().map(|(_, available)| *available),
        Some(true),
        "A schedules again: {availability:?}"
    );
    assert_eq!(status_fence(&host, POLICY_INSTANCE_ALIAS), None);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Workflow #403: the ladder ends without proof. A's re-preparation after the application
/// restart fails its self-check (A's input does not open), so the rung fails, the emulator rung
/// is skipped and the climb is exhausted. A stays fenced, with no lift record, and status says
/// it waits for a person; the Runtime runs.
#[test]
fn gate_a_ladder_that_cannot_prove_the_close_leaves_the_instance_waiting_for_a_person() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let (host, instance) = fenced_ladder_host(&root, &state);
    *state.input_open_error.lock().expect("input open error") = Some(
        DeviceError::transient("synthetic adb shell timeout")
            .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1),
    );
    *state.application_error.lock().expect("application error") =
        Some(unconfirmed_application_failure());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &application_stop(&ids, POLICY_INSTANCE_ALIAS),
            ConnectionId::new(184).expect("connection"),
        )
        .expect("application receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert!(!receipt.error_projection().expect("failure").fatal);
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(_, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_application_fence(outcome, instance);
    assert_eq!(
        rung_outcomes(&events),
        vec![
            (
                RecoveryRung::ReturnHome,
                RecoveryRungOutcome::Skipped,
                Some(
                    HostCode::RecoveryRungSkippedResourceFence
                        .as_str()
                        .to_owned()
                ),
            ),
            (
                RecoveryRung::ApplicationRestart,
                RecoveryRungOutcome::Failed,
                Some(HostCode::RecoveryEnvironmentNotReady.as_str().to_owned()),
            ),
            (
                RecoveryRung::EmulatorRestart,
                RecoveryRungOutcome::Skipped,
                Some(
                    HostCode::RecoveryRungSkippedNoEmulatorControl
                        .as_str()
                        .to_owned()
                ),
            ),
        ]
    );
    assert!(phases(&events).iter().any(|(_, phase)| matches!(
        phase,
        RuntimeLifecyclePhase::RecoveryLadderFinished {
            ladder_result: RecoveryLadderOutcome::Exhausted,
            ..
        }
    )));
    assert!(lift_records(&events).is_empty());
    let fence = status_fence(&host, POLICY_INSTANCE_ALIAS).expect("A stays fenced");
    assert!(fence.awaiting_person, "{fence:?}");
    assert_eq!(
        fence.cause.as_str(),
        SharedCode::ApplicationBackendOperationFailed.as_str()
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Workflow #403 (regression guard): a shared resource still ends the whole Runtime. A ledger
/// append the run on A needs fails; the Runtime is fatal, the other instance's work is refused
/// and no instance is fenced.
#[test]
fn gate_a_ledger_writer_failure_still_ends_the_whole_runtime() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("fence-ledger-failure.zip");
    let bytes = neutral_stability_contained_task_package(true, 2, 5);
    fs::write(&package, &bytes).expect("write stability package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let failing = Arc::new(FakeState::default());
    failing
        .physical_task_geometry
        .store(true, Ordering::Release);
    let other = Arc::new(FakeState::default());
    let (other_package, other_expected) = super::gate_369s5::direct_run_package(&root, &other);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::from_entries([
            (FENCED_ALIAS.to_owned(), instance_id(), Arc::clone(&failing)),
            (OTHER_ALIAS.to_owned(), instance_id(), Arc::clone(&other)),
        ])),
    )
    .expect("runtime host");
    host.fail_next_contained_task_stability_event_append_for_test()
        .expect("inject a ledger append failure");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let run = client.request(RuntimeOperation::run_contained_task(
        FENCED_ALIAS,
        client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(package.display().to_string(), expected).expect("task request"),
    ));

    let failed = client.send(&run);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    assert!(failed.error_projection().expect("fatal failure").fatal);
    assert_eq!(
        host.fatal_error()
            .expect("runtime health")
            .expect("the whole Runtime ended")
            .code(),
        "contained_task_stability_event_append_injected_failure"
    );
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let other_run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            OTHER_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(other_package, other_expected).expect("task request"),
        ),
    );
    let refused = host
        .process_request_for_test(&other_run, ConnectionId::new(185).expect("connection"))
        .expect("refusal receipt");
    assert_eq!(refused.state(), RuntimeReceiptState::Failed);
    assert!(refused.error_projection().expect("refusal").fatal);
    assert_eq!(other.capture_count.load(Ordering::Acquire), 0);
    assert!(fence_records(&all_events(&host)).is_empty());
    drop(client);
    assert!(host.close().is_err());
}
