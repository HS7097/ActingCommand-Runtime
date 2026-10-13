// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #403 (approved 2026-10-13): an unconfirmed close of one instance's own resources
//! fences only that instance, hands it to the recovery ladder and waits for a person when the
//! ladder ends without proving the close; a whole-Runtime fatal stays for shared things. A fence
//! that manual work met starts no ladder (M387 B1). Every wait returns as soon as what it waits
//! for is recorded; the bound only stops a broken run.

use super::instance_workers::{
    admit_ladder_run, ladder_setup, phases, rung_outcomes, wait_for_ladder_end,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    FactPayload, InstanceResourceFence, RecoveryLadderOutcome, RecoveryRung, RecoveryRungOutcome,
    RecoveryTriggerStage, RuntimeLifecyclePhase, RuntimePayload, SchedulingPauseScope,
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

/// An input whose adb command ends without confirming that its child is gone.
fn unconfirmed_input_failure() -> DeviceError {
    DeviceError::fatal("synthetic input command left its child unconfirmed")
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

/// Asserts that `outcome` is the fence of `instance` caused by the unconfirmed failure `cause`.
fn assert_fence(
    outcome: &actingcommand_contract::outcome::Outcome,
    instance: InstanceId,
    cause: &str,
) {
    assert_eq!(
        outcome.value("instance_id"),
        Some(&serde_json::to_value(instance).expect("instance id")),
        "{outcome:?}"
    );
    assert!(
        outcome.codes().iter().any(|code| code.as_str() == cause),
        "the fence names the unconfirmed failure: {outcome:?}"
    );
}

/// Whether any recovery ladder started.
fn ladder_started(events: &[PersistedEvent]) -> bool {
    phases(events)
        .iter()
        .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. }))
}

/// `actingctl resume` of `instance_alias`: the self-check its reconnect made.
fn resume_selfcheck(
    host: &RuntimeHost,
    instance_alias: &str,
) -> actingcommand_contract::SchedulingResumeSelfCheck {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::ResumeScheduling {
                    scope: SchedulingPauseScope::Instance {
                        instance_alias: instance_alias.to_owned(),
                    },
                    expected: None,
                },
            ),
            ConnectionId::new(191).expect("connection"),
        )
        .expect("resume receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::SchedulingResumed {
        selfcheck: Some(selfcheck),
        ..
    }) = receipt.result()
    else {
        panic!("an instance resume with its self-check: {receipt:?}");
    };
    selfcheck.clone()
}

/// Workflow #403 (the 10-13 incident's shape): an application command on instance A ends with an
/// unconfirmed close while instance B has queued work. A is fenced with
/// `instance_resources_fenced`, recorded once and caused by the command's failure; the command
/// fails without a fatal projection; B's queued run is dispatched and completes; the Runtime has
/// no fatal state and answers; its status shows A's fence and none for B. The command is a
/// person's manual work, so the fence starts no ladder and A waits for a person (M387 B1).
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
    // Receipt v2 (Workflow #378 A4): the refusal is the top of the receipt's outcome.
    assert_eq!(
        fenced_receipt.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str()
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
            task_result: TaskOutcome::Success,
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
    assert_fence(
        outcome,
        fenced,
        SharedCode::ApplicationBackendOperationFailed.as_str(),
    );
    let fence = status_fence(&host, FENCED_ALIAS).expect("A's fence in status");
    assert_eq!(
        fence.cause.as_str(),
        SharedCode::ApplicationBackendOperationFailed.as_str()
    );
    assert_eq!(status_fence(&host, OTHER_ALIAS), None);
    assert!(fence.awaiting_person, "{fence:?}");
    assert!(
        !ladder_started(&all_events(&host)),
        "a manual fence starts no ladder"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(holder);
    host.close().expect("close host");
}

/// Workflow #403, M387 B1 (coordinator ruling on #738): a direct task run (CLI, console or MCP)
/// whose input ends with an unconfirmed close fences its instance and starts no ladder; status
/// says it waits for a person. A person's resume lifts the fence only once its reconnect's
/// self-check passes: a resume whose self-check fails leaves it fenced, the next one, which
/// passes, lifts it with one record naming that preparation.
#[test]
fn gate_a_direct_run_fence_starts_no_ladder_and_a_passing_resume_lifts_it() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let (package, expected) = super::gate_369s5::direct_run_package(&root, &state);
    let fenced = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0))),
        Arc::new(FakeProvider::one(FENCED_ALIAS, fenced, Arc::clone(&state))),
    )
    .expect("runtime host");
    *state.input_error_once.lock().expect("one-shot input error") =
        Some(unconfirmed_input_failure());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            FENCED_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package, expected).expect("task request"),
        ),
    );

    let receipt = host
        .process_request_for_test(&run, ConnectionId::new(186).expect("connection"))
        .expect("run receipt");

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let projection = receipt.error_projection().expect("the run's failure");
    assert!(!projection.fatal, "{projection:?}");
    // Receipt v2 (Workflow #378 A4): the failure is the top of the receipt's outcome.
    assert_eq!(
        receipt.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str()
    );
    let fence = status_fence(&host, FENCED_ALIAS).expect("the fence in status");
    assert!(fence.awaiting_person, "{fence:?}");
    assert_eq!(
        fence.cause.as_str(),
        SharedCode::InputBackendOperationFailed.as_str()
    );
    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(_, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_fence(
        outcome,
        fenced,
        SharedCode::InputBackendOperationFailed.as_str(),
    );
    assert!(
        !ladder_started(&events),
        "a direct-run fence starts no ladder"
    );

    // The fake's opens report no passed native open: the resume's self-check fails.
    let failed = resume_selfcheck(&host, FENCED_ALIAS);
    assert!(!(failed.capture.ok && failed.touch.ok), "{failed:?}");
    assert!(
        status_fence(&host, FENCED_ALIAS).is_some_and(|fence| fence.awaiting_person),
        "a failing self-check leaves the fence"
    );
    assert!(lift_records(&all_events(&host)).is_empty());

    state.observed_open.store(true, Ordering::Release);
    let passed = resume_selfcheck(&host, FENCED_ALIAS);
    assert!(
        passed.capture.ok && passed.touch.ok && passed.failure_code.is_none(),
        "{passed:?}"
    );
    assert_eq!(status_fence(&host, FENCED_ALIAS), None);
    let events = all_events(&host);
    let lifts = lift_records(&events);
    let [(_, lifted_instance, preparation)] = lifts.as_slice() else {
        panic!("one lift record: {lifts:#?}");
    };
    assert_eq!(*lifted_instance, fenced);
    let proof = events
        .iter()
        .find(|event| event.sequence() == preparation.sequence)
        .expect("the proving preparation");
    assert!(matches!(
        phases(std::slice::from_ref(proof))
            .first()
            .map(|(_, phase)| phase.clone()),
        Some(RuntimeLifecyclePhase::InstancePreparationFinished {
            stage: RecoveryTriggerStage::ConnectionPreparation,
            outcome: None,
            ..
        })
    ));
    assert!(!ladder_started(&events));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// The policy fixture's instance (its availability gated on its self-check) with a bound device
/// endpoint, the neutral startup package and a scheduled run admitted (`ladder_setup`,
/// `admit_ladder_run`); its opens pass a preparation's self-check. The scheduled run's first input
/// ends with an unconfirmed close.
fn scheduled_fence_host(
    root: &TempDir,
    state: &Arc<FakeState>,
) -> (
    RuntimeHost,
    InstanceId,
    Box<PolicyRunContext>,
    ContainedTaskRequest,
) {
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let (host_config, request) = ladder_setup(root.path());
    let instance = instance_id();
    let host = RuntimeHost::start(
        host_config,
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
    let context = admit_ladder_run(&host, &request);
    *state.input_error_once.lock().expect("one-shot input error") =
        Some(unconfirmed_input_failure());
    (host, instance, context, request)
}

/// Runs the admitted scheduled run: its input's unconfirmed close fences the instance and the
/// run fails non-fatally with the fence's code.
fn fail_scheduled_run(
    host: &RuntimeHost,
    context: &PolicyRunContext,
    request: &ContainedTaskRequest,
) {
    let error = host
        .run_scheduled_contained_task(context, request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{error:?}");
    assert_eq!(error.code(), HostCode::InstanceResourcesFenced.as_str());
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

/// Workflow #403: the ladder proves the close. A scheduled run's input on A ends unconfirmed
/// and A is fenced; the fence's ladder skips the return-home rung (it cannot prove a close),
/// restarts the application and prepares A again, and that preparation's passing self-check
/// lifts the fence with one record that names it. A's availability, withdrawn by the fence,
/// follows its self-check again, and status shows no fence: A schedules again.
#[test]
fn gate_a_ladder_whose_re_preparation_passes_lifts_the_fence() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let (host, instance, context, request) = scheduled_fence_host(&root, &state);
    fail_scheduled_run(&host, &context, &request);
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(fenced, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_fence(
        outcome,
        instance,
        SharedCode::InputBackendOperationFailed.as_str(),
    );
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
        SharedCode::InputBackendOperationFailed.as_str()
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

/// Workflow #403: the ladder ends without proof. A scheduled run's input on A ends unconfirmed
/// and A is fenced; after the fence's application restart, A's re-preparation fails its
/// self-check (its opens report no passed native open), so the rung fails, the emulator rung is
/// skipped and the climb is exhausted. A stays fenced, with no lift record, and status says it
/// waits for a person; the Runtime runs.
#[test]
fn gate_a_ladder_that_cannot_prove_the_close_leaves_the_instance_waiting_for_a_person() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let (host, instance, context, request) = scheduled_fence_host(&root, &state);
    state.observed_open.store(false, Ordering::Release);
    fail_scheduled_run(&host, &context, &request);
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(_, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_fence(
        outcome,
        instance,
        SharedCode::InputBackendOperationFailed.as_str(),
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
        SharedCode::InputBackendOperationFailed.as_str()
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
