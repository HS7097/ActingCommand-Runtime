// SPDX-License-Identifier: AGPL-3.0-only

// #411 A7 task path (v0.12.2; host gate 108; register §1.106; Alice 2026-10-19, pending item 13 =
// A): a contained task's capture failure on one instance is that instance's failure, not the
// Runtime's. A failure with one of the instance's capture codes (`capture_backend_open_failed`,
// `paired_backend_open_failed`, `capture_backend_operation_failed`, `capture_frame_invalid`) whose
// temporary resources were released fails only that run, on every capture of the run: the receipt
// is `Failed` with the kernel code, projected as a nonfatal `capture_failed`; the host is not
// fatal and the other instance runs. A scheduled run's such failure starts the recovery ladder by
// the shipped trigger item `task_capture_backend`, whose first two rungs the failed capture
// self-check skips. A physical instance's first capture still probes the ADB baseline once: a
// probe that fails too invalidates the ADB facts, and a probe whose own adb child cannot be
// confirmed gone fences the instance (Workflow #403).
//
// The capture failure is fatal (not transient) with confirmed disposal and no diagnostic
// category, as in `gate_0121_a7`; the ladder configuration is the shipped default. Every wait is
// bounded by `GATE_WAIT` and returns as soon as what it waits for is recorded.

use super::gate_398_rd2::{fact, instance_facts};
use super::gate_403::{assert_fence, fence_records, status_fence};
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_setup, phases, rung_outcomes, wait_for_ladder_end,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::{DeviceSelfCheckFailure, SharedCode};
use actingcommand_contract::{
    EmulatorInstanceAction, RecoveryRung, RecoveryRungOutcome, RecoveryTriggerStage, RequestId,
    RuntimeFactInvalidationReason, RuntimeLifecyclePhase, RuntimePayload,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_execution_kernel::ExecutionKernelCode;

const GATE_WAIT: Duration = Duration::from_secs(120);
const INSTANCE_A: &str = "a7t.instance.a";
const INSTANCE_B: &str = "a7t.instance.b";
const CAPTURE_OPEN_FAILED: &str = DeviceSelfCheckFailure::CaptureBackendOpenFailed.as_str();
const TRIGGER_ITEM: &str = "task_capture_backend";
const CAPTURE_SELFCHECK_STATUS: &str = "backend.selfcheck.capture.status";
const ADB_FACTS: [&str; 3] = ["device.connected", "application.foreground", "task.page"];

// The capture open fails fatally without a diagnostic category, its one acquired resource
// closed (confirmed disposal).
fn capture_open_failure() -> DeviceError {
    DeviceError::fatal("fixture capture open got no positive connection handle")
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
}

// An ADB baseline probe that finds no device; its adb child is gone (confirmed).
fn lost_adb_probe() -> DeviceError {
    DeviceError::fatal("synthetic adb probe found no device")
}

// An ADB baseline probe whose adb child could not be confirmed gone.
fn unconfirmed_adb_probe() -> DeviceError {
    DeviceError::fatal("synthetic adb probe left its child unconfirmed")
        .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1)
}

// A started host with instance A (`state`, id `a`) and instance B (`other_state`).
fn two_instances(
    root: &TempDir,
    a: InstanceId,
    state: &Arc<FakeState>,
    other_state: &Arc<FakeState>,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::from_entries([
            (INSTANCE_A.to_owned(), a, Arc::clone(state)),
            (
                INSTANCE_B.to_owned(),
                instance_id(),
                Arc::clone(other_state),
            ),
        ])),
    )
    .expect("runtime host with two instances")
}

// A direct run of `package` on `instance_alias`, as `actingctl` sends it.
fn direct_run(instance_alias: &str, package: &str) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            instance_alias,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    )
}

fn send(host: &RuntimeHost, request: &RuntimeRequest, connection: u64) -> RuntimeReceipt {
    host.process_request_for_test(request, ConnectionId::new(connection).expect("connection"))
        .expect("a receipt")
}

// The run failed only itself: `Failed` with the kernel code `code` on top, projected as a
// nonfatal `capture_failed`.
fn assert_run_failed_nonfatal(receipt: &RuntimeReceipt, code: &str) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
    let projection = receipt.error_projection().expect("the run's failure");
    assert_eq!(
        projection.code,
        RuntimeErrorCode::CaptureFailed,
        "{receipt:?}"
    );
    assert!(!projection.fatal, "{receipt:?}");
}

// The other instance's direct run completes on its target page.
fn assert_completes(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success,
            ..
        })
    ));
}

fn assert_not_fatal(host: &RuntimeHost) {
    assert!(host.fatal_error().expect("runtime health").is_none());
}

// The `task.failed` terminals recorded under `request`.
fn task_failed(host: &RuntimeHost, request: RequestId) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        request_id: Some(request),
        event_type: Some(EventType::TaskFailed),
        ..EventQuery::default()
    })
    .expect("the run's events")
}

fn head(host: &RuntimeHost) -> u64 {
    all_events(host).last().map_or(0, PersistedEvent::sequence)
}

// Requirement 1: instance A's first capture cannot open its capture backend while adbd answers.
// Only A's run fails: `Failed` with `capture_backend_open_failed`, a nonfatal `capture_failed`;
// its `task.failed` terminal has the code's category as its severity (ledger revision 3), and A's
// capture self-check reads `failed`. The host is not fatal, B's direct run then completes, and the
// host closes normally.
#[test]
fn a7t_a_task_capture_open_failure_fails_only_that_run_and_the_other_instance_runs() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let a = instance_id();
    let host = two_instances(&root, a, &state, &other_state);
    *state.capture_open_error.lock().expect("capture open error") = Some(capture_open_failure());

    let request = direct_run(INSTANCE_A, &package);
    let failed = send(&host, &request, 1221);

    assert_run_failed_nonfatal(&failed, CAPTURE_OPEN_FAILED);
    let terminals = task_failed(&host, request.request_id());
    let [terminal] = terminals.as_slice() else {
        panic!("one task.failed of the run: {terminals:#?}");
    };
    assert_eq!(terminal.severity(), EventSeverity::Error);
    assert_eq!(
        fact(&instance_facts(&host, a), CAPTURE_SELFCHECK_STATUS),
        Some(ContractFactValue::String("failed".to_owned()))
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert_not_fatal(&host);

    let other = send(&host, &direct_run(INSTANCE_B, &other_package), 1222);
    assert_completes(&other);
    assert_not_fatal(&host);
    host.close().expect("close host");
}

// Requirement 1, every capture of the run: A's run took its first frame and its second capture
// fails (not transient) on the open session. The run fails with that capture's code,
// `capture_backend_operation_failed`, nonfatal; the host is not fatal and B's run completes.
#[test]
fn a7t_a_later_capture_failure_fails_only_that_run() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let host = two_instances(&root, instance_id(), &state, &other_state);
    state.fail_capture_on.store(2, Ordering::Release);

    let failed = send(&host, &direct_run(INSTANCE_A, &package), 1223);

    assert_run_failed_nonfatal(&failed, SharedCode::CaptureBackendOperationFailed.as_str());
    assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
    assert_not_fatal(&host);

    let other = send(&host, &direct_run(INSTANCE_B, &other_package), 1224);
    assert_completes(&other);
    assert_not_fatal(&host);
    host.close().expect("close host");
}

// Requirement 4: a scheduled run on the ladder instance (stuck recovery on, shipped defaults)
// whose capture open fails ends nonfatal, its policy class not `Severe`. Its failed lease end
// admits one ladder, whose trigger names the item `task_capture_backend` at stage `task` and the
// failed open; with the capture self-check failed, its `return_home` and `application_restart`
// rungs are skipped `capture_unavailable`. The host is not fatal.
#[test]
fn a7t_a_scheduled_run_whose_capture_open_fails_starts_the_task_capture_backend_ladder() {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let context = admit_ladder_run(&host, &request);
    *state.capture_open_error.lock().expect("capture open error") = Some(capture_open_failure());

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{}", error.code());
    assert_eq!(error.code(), CAPTURE_OPEN_FAILED);
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    let events = all_events(&host);
    let classes = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::ExecutionRecorded(execution)) => {
                match execution.policy_result() {
                    PolicyExecutionOutcome::Failed { failure } => Some(failure.original_class),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [class] = classes.as_slice() else {
        panic!("one failed policy execution: {classes:?}");
    };
    assert_ne!(*class, PolicyFailureClass::Severe);
    let triggers = phases(&events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. } => Some(trigger),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [trigger] = triggers.as_slice() else {
        panic!("one ladder: {triggers:#?}");
    };
    assert_eq!(trigger.stage, RecoveryTriggerStage::Task);
    assert_eq!(trigger.rule.as_deref(), Some(TRIGGER_ITEM));
    assert_eq!(trigger.failure_code.as_str(), CAPTURE_OPEN_FAILED);
    let skipped = Some(
        HostCode::RecoveryRungSkippedCaptureUnavailable
            .as_str()
            .to_owned(),
    );
    let rungs = rung_outcomes(&events);
    for rung in [RecoveryRung::ReturnHome, RecoveryRung::ApplicationRestart] {
        assert!(
            rungs.contains(&(rung, RecoveryRungOutcome::Skipped, skipped.clone())),
            "{rung:?}: {rungs:?}"
        );
    }
    assert_not_fatal(&host);
    host.close().expect("close host");
}

// Requirement 2: the instance's ADB facts are present (an emulator start wrote
// `device.connected`, a completed run `application.foreground` and `task.page`). Then the
// emulator goes away: the next run's first capture fails on the kept session and the run's one
// ADB baseline probe fails too, its adb child gone. The three facts are invalidated with
// `adb_unreachable`; the run fails nonfatal and the host is not fatal.
#[test]
fn a7t_a_first_capture_failure_with_adb_lost_invalidates_the_adb_facts() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let a = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(INSTANCE_A, a, Arc::clone(&state))),
    )
    .expect("runtime host");
    bind_fake_emulator(&host, INSTANCE_A, a, &state);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let started = send(
        &host,
        &runtime_request(
            &ids,
            RuntimeOperation::ControlEmulatorInstance {
                instance_alias: INSTANCE_A.to_owned(),
                action: EmulatorInstanceAction::Start,
            },
        ),
        1225,
    );
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    assert_completes(&send(&host, &direct_run(INSTANCE_A, &package), 1226));
    let present = instance_facts(&host, a);
    for key in ADB_FACTS {
        assert!(
            fact(&present, key).is_some(),
            "{key} is present: {present:#?}"
        );
    }
    state.fail_capture.store(true, Ordering::Release);
    *state.adb_probe_error.lock().expect("adb probe error") = Some(lost_adb_probe());
    let before = head(&host);

    let failed = send(&host, &direct_run(INSTANCE_A, &package), 1227);

    assert_run_failed_nonfatal(&failed, SharedCode::CaptureBackendOperationFailed.as_str());
    let mut invalidated = all_events(&host)
        .iter()
        .filter(|event| event.sequence() > before)
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactInvalidated(record))
                if record.invalidation().reason
                    == RuntimeFactInvalidationReason::AdbUnreachable =>
            {
                Some(record.invalidation().key.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    invalidated.sort();
    let mut expected = Vec::from(ADB_FACTS.map(str::to_owned));
    expected.sort();
    assert_eq!(invalidated, expected);
    assert_not_fatal(&host);
    host.close().expect("close host");
}

// Requirement 2, the #403 rule: A's first capture open fails and the run's ADB baseline probe
// leaves its adb child unconfirmed. Instance A is fenced (one `instance_resources_fenced` record
// caused by the probe's failure; it waits for a person, as manual work does) and its run fails
// nonfatal with `instance_resources_fenced`. The host is not fatal, B has no fence and its run
// completes.
#[test]
fn a7t_a_first_capture_probe_whose_adb_child_is_unconfirmed_fences_the_instance() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let a = instance_id();
    let host = two_instances(&root, a, &state, &other_state);
    *state.capture_open_error.lock().expect("capture open error") = Some(capture_open_failure());
    *state.adb_probe_error.lock().expect("adb probe error") = Some(unconfirmed_adb_probe());

    let failed = send(&host, &direct_run(INSTANCE_A, &package), 1228);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert!(
        !failed.error_projection().expect("the run's failure").fatal,
        "{failed:?}"
    );
    assert_eq!(
        failed.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str()
    );
    wait_until(GATE_WAIT, || {
        status_fence(&host, INSTANCE_A).is_some_and(|fence| fence.awaiting_person)
    });
    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(record, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_eq!(record.links().instance_id(), Some(&a));
    assert_fence(
        outcome,
        a,
        ExecutionKernelCode::ApplicationAdbBaselineFailed.as_str(),
    );
    assert_eq!(status_fence(&host, INSTANCE_B), None);
    assert_not_fatal(&host);

    let other = send(&host, &direct_run(INSTANCE_B, &other_package), 1229);
    assert_completes(&other);
    assert_not_fatal(&host);
    host.close().expect("close host");
}
