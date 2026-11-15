// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 H3 (0.12.62; register §1.399; the design's place 8): an emulator control that
// fails with `emulator_control_failed` says where it failed. The outcome carries the stage the
// failure arose at (`stage`, a location) and the device diagnostic's category (`device_category`),
// the two its primary detail records, which stays as it was. One discovery-bound instance on the
// scripted emulator controller (HOST-I1), which gives each stage its family's category: an
// operator `emulator start`, then a `stop` the controller fails at one stage of each family (the
// vendor command exited non-zero, it could not be launched, its answer was refused). The receipt
// and the control's one `runtime.failed` record carry the values; the record's other values and
// its primary detail are the failure's own. The default stage of a failure without a diagnostic
// is the unit gate `host/emulator_instance/gate_469_h3.rs`.

use super::emulator_controller::{SCRIPTED_FAILED_MS, ScriptedControl, running_state};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::outcome::{Location, Outcome};
use actingcommand_contract::{EmulatorInstanceAction, RuntimePayload};
use actingcommand_device::DeviceProcessPresence;
use actingcommand_device::codes::DeviceStage;
use serde_json::{Value, json};

const ALIAS: &str = "emulator.h3";

/// A started host whose one physical instance is discovery-bound to the scripted emulator,
/// running on its default port.
struct EmulatorHost {
    _root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
}

fn emulator_host() -> EmulatorHost {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id, Arc::clone(&state))),
    )
    .expect("runtime host");
    bind_fake_emulator(&host, ALIAS, instance_id, &state);
    EmulatorHost {
        _root: root,
        host,
        state,
    }
}

/// `actingctl emulator <action>`, a Cli request.
fn control(host: &RuntimeHost, action: EmulatorInstanceAction) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.process_request_for_test(
        &runtime_request(
            &ids,
            RuntimeOperation::ControlEmulatorInstance {
                instance_alias: ALIAS.to_owned(),
                action,
            },
        ),
        ConnectionId::new(469).expect("connection"),
    )
    .expect("receipt")
}

/// The two values that say where the control failed.
fn words(outcome: &Outcome) -> (Option<Value>, Option<Value>) {
    (
        outcome.value("stage").cloned(),
        outcome.value("device_category").cloned(),
    )
}

/// After a confirmed `start`, a `stop` the controller fails at `stage` with `exit_code` (its
/// process check unknown, the state read after it running): the receipt and the control's one
/// `runtime.failed` record are `emulator_control_failed` with `stage` and `category` as the values
/// `stage` and `device_category`; the record keeps its other values (the time, the exit code, the
/// process check) and its primary detail is the stage and category, as before.
fn assert_stop_failure_says_where(
    stage: Location,
    exit_code: Option<i32>,
    category: DeviceErrorCategory,
) {
    let run = emulator_host();
    let started = control(&run.host, EmulatorInstanceAction::Start);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    run.state.emulator.script_control(
        EmulatorInstanceAction::Stop,
        ScriptedControl::Failed {
            stage,
            exit_code,
            last_state: None,
            device_process_present: Some(DeviceProcessPresence::Unknown),
        },
    );
    run.state.emulator.script_read(running_state(16384));
    let receipt = control(&run.host, EmulatorInstanceAction::Stop);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let expected = (Some(json!(stage.as_str())), Some(json!(category.as_str())));
    assert!(
        receipt.outcome().is(HostCode::EmulatorControlFailed),
        "{receipt:?}"
    );
    assert_eq!(words(receipt.outcome()), expected, "the receipt");

    let request_id = receipt.request_id();
    let events = run
        .host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let failures = events
        .iter()
        .filter(|event| event.links().request_id() == Some(&request_id))
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record)) => record
                .lifecycle_failure()
                .map(|failure| (record.outcome(), failure.primary_detail())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(recorded, detail)] = failures.as_slice() else {
        panic!("one failure record: {failures:#?}");
    };
    assert!(recorded.is(HostCode::EmulatorControlFailed), "{recorded:?}");
    assert_eq!(words(recorded), expected, "the record");
    assert_eq!(
        recorded.value("elapsed_ms"),
        Some(&json!(SCRIPTED_FAILED_MS))
    );
    assert_eq!(
        recorded.value("exit_code"),
        exit_code.map(|code| json!(code)).as_ref()
    );
    assert_eq!(
        recorded.value("device_process_present"),
        Some(&json!(DeviceProcessPresence::Unknown.as_str()))
    );
    let detail = detail.expect("the record's primary detail");
    assert_eq!(
        (
            detail.category(),
            detail.stage(),
            detail.backend(),
            detail.operation()
        ),
        (
            category.as_str(),
            stage.as_str(),
            "mumu_manager",
            ContractLocation::ControlInstance.as_str()
        )
    );
    run.host.close().expect("close host");
}

// The vendor command ran and exited non-zero: `mumu_manager_control_exit`, `child_exit`.
#[test]
fn h3_a_control_whose_vendor_command_exited_says_its_stage_and_child_exit() {
    assert_stop_failure_says_where(
        DeviceStage::MumuManagerControlExit.location(),
        Some(1),
        DeviceErrorCategory::ChildExit,
    );
}

// The vendor tool could not be launched: `mumu_manager_run`, `backend_launch`, no exit code.
#[test]
fn h3_a_control_whose_tool_could_not_run_says_its_stage_and_backend_launch() {
    assert_stop_failure_says_where(
        DeviceStage::MumuManagerRun.location(),
        None,
        DeviceErrorCategory::BackendLaunch,
    );
}

// The vendor answered with an error envelope: `mumu_manager_control_errcode`, `response`.
#[test]
fn h3_a_control_the_vendor_refused_says_its_stage_and_response() {
    assert_stop_failure_says_where(
        DeviceStage::MumuManagerControlErrcode.location(),
        Some(0),
        DeviceErrorCategory::Response,
    );
}
