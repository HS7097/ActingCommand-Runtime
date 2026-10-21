// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A48 (0.12.9; the production host on 10-21, after a reboot): the Runtime started before
//! the emulators, which were then started outside it. The Runtime still held each instance
//! stopped (its discovery binding pending), so `actingctl selfcheck` opened nothing and reported
//! `input_backend_open_failed` with an unobserved provider and no attempt, as if adb were broken;
//! one Runtime `emulator start` per instance was needed before a self-check passed. Requirements
//! of the slice brief, through `SelfCheckInstance` (what `actingctl selfcheck` sends):
//!
//! - T1 (requirements 1 and 2): the emulator runs, started outside the Runtime, while the
//!   Runtime holds the instance stopped: the self-check reads the status, recognizes the device
//!   as after a Runtime start (without starting anything), updates the instance's state and
//!   passes once the backends open;
//! - T2 (requirement 3): the emulator is stopped: the self-check reports `instance_not_running`,
//!   not `input_backend_open_failed`, and no start is requested;
//! - T3 (requirement 4): the emulator runs, but its device cannot be recognized (its Android has
//!   not started and it reports no ADB port): the self-check reports
//!   `emulator_device_unrecognized`, not `input_backend_open_failed`, and its record carries the
//!   status read as typed values.
//!
//! The instance is discovery-bound to the scripted emulator, which was stopped when the host
//! started, so the Runtime holds it stopped: its binding pending and its reading `off`. Its opens
//! are refused while the binding is pending, as the device registry refuses them
//! (`refuse_pending_opens`), and pass a self-check once a port is bound.
//!
//! Existing tests missed this because their fake opens answered whatever the binding (the
//! registry's refusal of a pending endpoint was not modelled), so a self-check of a pending
//! instance passed in every test, and no test started an emulator outside the Runtime: every
//! emulator-controlled fake was started or stopped through the Runtime's own control, which
//! binds the port itself, and no self-check read the emulator status before its opens.

use super::emulator_controller::{
    EmulatorCall, FAKE_EMULATOR_PORT, FakeEmulatorController, running_state, stopped_state,
};
use super::*;
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::{
    EmulatorRunningState, RequestId, RuntimePayload, SchedulingResumeSelfCheck,
};

const ALIAS: &str = "a48.instance";

/// A host whose one physical instance is discovery-bound to the scripted emulator, started while
/// that emulator was stopped: the Runtime holds the instance stopped. Its opens are refused while
/// its binding is pending and pass a self-check once a port is bound.
fn host_started_before_its_emulator(root: &TempDir) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState {
        emulator: FakeEmulatorController::pending(),
        ..FakeState::default()
    });
    state.emulator_control.store(true, Ordering::Release);
    state.refuse_pending_opens.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host started before its emulator");
    assert!(
        matches!(emulator_state(&host), EmulatorRunningState::Off { .. }),
        "the Runtime holds the instance stopped"
    );
    (host, state)
}

/// `actingctl selfcheck` of the instance on `connection`: its request id and its self-check.
fn self_check(host: &RuntimeHost, connection: u64) -> (RequestId, SchedulingResumeSelfCheck) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: ALIAS.to_owned(),
        },
    );
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(connection).expect("connection"))
        .expect("self-check receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = receipt.result() else {
        panic!("a self-check result: {receipt:?}");
    };
    (request.request_id(), selfcheck.clone())
}

/// The instance's emulator state in the Runtime's status.
fn emulator_state(host: &RuntimeHost) -> EmulatorRunningState {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::Status),
            ConnectionId::new(480).expect("connection"),
        )
        .expect("status receipt");
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status result: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == ALIAS)
        .expect("the instance's status")
        .emulator_state()
}

/// The emulator control actions (start, stop, restart) the scripted emulator was asked for.
fn control_calls(state: &FakeState) -> Vec<EmulatorCall> {
    state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| matches!(call, EmulatorCall::Control(_)))
        .collect()
}

/// The outcome of the `runtime.failed` record of `code` under the request `request_id`.
fn failure_record(
    host: &RuntimeHost,
    request_id: RequestId,
    code: &str,
) -> Option<actingcommand_contract::outcome::Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        request_id: Some(request_id),
        ..EventQuery::default()
    })
    .expect("events of the self-check")
    .iter()
    .find_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(record))
            if record.outcome().code().as_str() == code =>
        {
            Some(record.outcome().clone())
        }
        _ => None,
    })
}

// T1 (requirements 1 and 2): the emulator was started outside the Runtime and its status reads
// running on its port while the Runtime holds the instance stopped. The self-check recognizes
// the device without starting anything, binds the reported port, holds the emulator running and
// passes once the backends open.
#[test]
fn a48_t1_a_self_check_recognizes_an_emulator_started_outside_the_runtime_and_passes() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = host_started_before_its_emulator(&root);
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));

    let (_, selfcheck) = self_check(&host, 481);

    assert!(
        selfcheck.capture.ok && selfcheck.touch.ok && selfcheck.failure_code.is_none(),
        "the self-check recognizes the device and passes: {selfcheck:?}"
    );
    assert!(
        matches!(emulator_state(&host), EmulatorRunningState::On { .. }),
        "the Runtime holds the emulator running"
    );
    assert_eq!(
        state.emulator.bound_port(),
        Some(FAKE_EMULATOR_PORT),
        "the instance is bound to the port the status reported"
    );
    let controls = control_calls(&state);
    assert!(controls.is_empty(), "nothing was started: {controls:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// T2 (requirement 3): the emulator's status reads stopped. The self-check reports
// `instance_not_running`, not `input_backend_open_failed`, and asks for no start.
#[test]
fn a48_t2_a_self_check_of_a_stopped_emulator_reports_instance_not_running_and_starts_nothing() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = host_started_before_its_emulator(&root);
    state.emulator.script_read(stopped_state());

    let (_, selfcheck) = self_check(&host, 482);

    assert_eq!(
        selfcheck.failure_code.as_deref(),
        Some("instance_not_running"),
        "{selfcheck:?}"
    );
    let controls = control_calls(&state);
    assert!(controls.is_empty(), "no start was requested: {controls:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// T3 (requirement 4): the emulator's process runs, but its Android has not started and it
// reports no ADB port, so its device cannot be recognized. The self-check reports
// `emulator_device_unrecognized`, not `input_backend_open_failed`; its record carries the status
// read as typed values and the step it failed at as its stage. Nothing is started.
#[test]
fn a48_t3_a_running_emulator_whose_device_is_not_recognized_reports_why_with_the_status_read() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = host_started_before_its_emulator(&root);
    state.emulator.script_read(crate::InstanceState {
        process_started: true,
        android_started: false,
        adb_port: None,
        player_state: None,
        launch_err_code: 0,
        launch_err_msg: String::new(),
    });

    let (request_id, selfcheck) = self_check(&host, 483);

    assert_eq!(
        selfcheck.failure_code.as_deref(),
        Some("emulator_device_unrecognized"),
        "{selfcheck:?}"
    );
    let record = failure_record(&host, request_id, "emulator_device_unrecognized")
        .expect("the self-check records why the device was not recognized");
    let boolean = |value: bool| Some(serde_json::Value::Bool(value));
    assert_eq!(
        record.value("process_started").cloned(),
        boolean(true),
        "{record:?}"
    );
    assert_eq!(
        record.value("android_started").cloned(),
        boolean(false),
        "{record:?}"
    );
    assert_eq!(
        record.value("adb_port_present").cloned(),
        boolean(false),
        "{record:?}"
    );
    assert_eq!(
        record.value("stage").and_then(serde_json::Value::as_str),
        Some(ContractLocation::ReadInstanceState.as_str()),
        "{record:?}"
    );
    let controls = control_calls(&state);
    assert!(controls.is_empty(), "nothing was started: {controls:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
