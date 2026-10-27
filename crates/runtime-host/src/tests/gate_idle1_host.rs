// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-1 (Alice's ruling 41 of 10-24 and her ruling of 10-25; #419, Alice: 「一个模拟器故障之后
//! 不能崩掉整个 acting command」): the host glue of the idle action (`host/idle.rs`) when its stop
//! fails. Two instances on a manual clock: X, whose configuration names `idle.action`
//! `stop_emulator` (with `start_emulator`, its configured emulator start and a startup package),
//! discovery-bound to the scripted emulator, which runs; and Y, which the configuration does not
//! name. The scheduler's plan for X is a fixed input, as in `gate_idle1`: no task due, the next
//! one four hours away. X's provider Stop fails. The failure is recorded under X and goes through
//! `contain_failure`, so it fails only X's action: no `instance_idle_action` record and no wake
//! follow for X, the Runtime does not latch, Y is not fenced and Y's run completes. Every wait
//! returns as soon as what it waits for has happened; `WAIT` only bounds a broken run.
//!
//! IDLE-2 (the owed wake; v0.12.13 in production on 10-26): X's stop succeeds, a check meets X
//! while the test holds its admission guard (as the emulator watch's status read does), and X
//! still owes its wake: one action, then at next due minus the default lead its wake and the
//! configured emulator start it queues.

use super::emulator_controller::{
    EmulatorCall, FAKE_EMULATOR_PORT, ScriptedControl, running_state,
};
use super::gate_369_s6c::{claims_after, connectivity, head};
use super::gate_403::status_fence;
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::{HostCode, IdleCode, IdleLocation};
use crate::idle_action::IdlePlan;
use actingcommand_contract::{
    EmulatorInstanceAction, InstanceStuckRecovery, RuntimeConfigManifest, RuntimeLifecyclePhase,
    RuntimePayload,
};
use actingcommand_device::DeviceProcessPresence;
use actingcommand_device::codes::DeviceStage;
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(60);
const X_ALIAS: &str = "idle1.instance.x";
const Y_ALIAS: &str = "idle1.instance.y";
/// The manual clock's Unix reading.
const NOW_UNIX_MS: u64 = 1_000;
/// X's next due task in the plan: four hours away, past `idle.min_gap_ms`'s default.
const NEXT_DUE_UNIX_MS: u64 = NOW_UNIX_MS + 4 * 3_600_000;
const MINUTE_MS: u64 = 60_000;
/// `idle.stop_emulator_wake_lead_ms`'s default.
const STOP_EMULATOR_LEAD_MS: u64 = 300_000;

fn instance_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// The configuration actingd assembles for X alone over the defaults file: X's entry names
/// `start_emulator` and the idle action `stop_emulator`.
fn idle_manifest(x: InstanceId) -> RuntimeConfigManifest {
    let id = instance_text(x);
    let file = json!({"instances": [{
        "instance_id": id,
        "start_emulator": true,
        "idle": {"action": "stop_emulator"}
    }]});
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        std::slice::from_ref(&id),
        &defaults,
        &file,
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

struct Fixture {
    host: RuntimeHost,
    x: InstanceId,
    x_state: Arc<FakeState>,
    y_package: String,
    clock: Arc<ManualRuntimeClock>,
    _root: TempDir,
}

/// X (emulator-controlled from the start, so it registers with its emulator running) and Y (a
/// direct-run package), X's stuck recovery off so no ladder acts on it.
fn fixture() -> Fixture {
    let root = TempDir::new().expect("tempdir");
    let x_state = Arc::new(FakeState::default());
    x_state.emulator_control.store(true, Ordering::Release);
    x_state
        .physical_task_geometry
        .store(true, Ordering::Release);
    x_state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    x_state.observed_open.store(true, Ordering::Release);
    let y_state = Arc::new(FakeState::default());
    let y_package = super::gate_369s5::direct_run_package(&root, &y_state);
    let package_path = root.path().join("startup-package.zip");
    fs::write(&package_path, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    let x = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_stuck_recovery(BTreeMap::from([(
                X_ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )]))
            .with_emulator_autostart(vec![X_ALIAS.to_owned()])
            .with_network_connectivity_for_test(read)
            .with_startup_packages(BTreeMap::from([(X_ALIAS.to_owned(), startup)]))
            .with_config_manifest(idle_manifest(x)),
        Arc::new(FakeProvider::from_entries([
            (X_ALIAS.to_owned(), x, Arc::clone(&x_state)),
            (Y_ALIAS.to_owned(), instance_id(), y_state),
        ])),
    )
    .expect("runtime host");
    Fixture {
        host,
        x,
        x_state,
        y_package,
        clock,
        _root: root,
    }
}

/// `alias`'s direct run of `package`, as a client sends it.
fn direct_run(host: &RuntimeHost, alias: &str, package: &str, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            alias,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.to_owned()).expect("task request"),
        ),
    );
    host.process_request_for_test(&run, ConnectionId::new(connection).expect("connection"))
        .expect("the run's receipt")
}

/// X's plan: evaluated now, a task due (`due_now`) or none and its next one at
/// `NEXT_DUE_UNIX_MS`.
fn x_plan(due_now: bool) -> BTreeMap<String, IdlePlan> {
    BTreeMap::from([(
        X_ALIAS.to_owned(),
        IdlePlan {
            evaluated_at_unix_ms: NOW_UNIX_MS,
            due_now,
            next_wake_unix_ms: (!due_now).then_some(NEXT_DUE_UNIX_MS),
            reading_due_unix_ms: None,
        },
    )])
}

/// X's plan evaluated at `at`: no task due, the next one at `NEXT_DUE_UNIX_MS`.
fn x_plan_at(at: u64) -> BTreeMap<String, IdlePlan> {
    BTreeMap::from([(
        X_ALIAS.to_owned(),
        IdlePlan {
            evaluated_at_unix_ms: at,
            due_now: false,
            next_wake_unix_ms: Some(NEXT_DUE_UNIX_MS),
            reading_due_unix_ms: None,
        },
    )])
}

#[test]
fn idle1_g_a_failed_stop_is_contained_to_its_instance_and_the_runtime_goes_on() {
    let fixture = fixture();
    // X's provider Stop fails: the vendor tool exits non-zero.
    fixture.x_state.emulator.script_control(
        EmulatorInstanceAction::Stop,
        ScriptedControl::Failed {
            stage: DeviceStage::MumuManagerControlExit.location(),
            exit_code: Some(1),
            last_state: None,
            device_process_present: Some(DeviceProcessPresence::Unknown),
        },
    );
    fixture
        .x_state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    let before = head(&fixture.host);

    fixture
        .host
        .check_idle_instances_for_test(&x_plan(false))
        .expect("the idle check");
    assert_eq!(
        fixture.host.await_idle_action_ends_for_test(1, WAIT),
        Some(1),
        "X's idle action ended"
    );

    // X's action asked for one Stop, and the Stop's failure is recorded under X.
    let stops = fixture
        .x_state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| *call == EmulatorCall::Control(EmulatorInstanceAction::Stop))
        .count();
    assert_eq!(stops, 1, "X's idle action asked for its Stop once");
    let events = all_events(&fixture.host);
    let under_x = |event: &PersistedEvent| {
        event.sequence() > before && event.links().instance_id() == Some(&fixture.x)
    };
    let failures = events
        .iter()
        .filter(|event| under_x(event))
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record)) => {
                Some(record.outcome().code().as_str().to_owned())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        failures
            .iter()
            .any(|code| code == HostCode::EmulatorControlFailed.as_str()),
        "the Stop's failure is recorded under X: {failures:?}"
    );
    // The action did not happen: no record says it ran.
    let taken = phases(&events)
        .into_iter()
        .filter(|(event, _)| under_x(event))
        .filter(|(_, phase)| {
            matches!(
                phase,
                RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                    if outcome.code().as_str() == IdleCode::Taken.as_str()
            )
        })
        .count();
    assert_eq!(taken, 0, "no instance_idle_action record for X");

    // X is left as it is: a due task at the planned wake queues no wake and no second action.
    let after = head(&fixture.host);
    fixture
        .host
        .check_idle_instances_for_test(&x_plan(true))
        .expect("the idle check at X's due task");
    assert!(
        claims_after(&fixture.host, fixture.x, after).is_empty(),
        "no wake and no second action follow X's failed action"
    );

    // The Runtime does not latch, Y is not fenced, and Y's run completes.
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none(),
        "X's failed stop latched the Runtime"
    );
    assert!(
        !fixture
            .host
            .is_shutdown_requested()
            .expect("shutdown state"),
        "X's failed stop requested a shutdown"
    );
    assert_eq!(
        status_fence(&fixture.host, Y_ALIAS),
        None,
        "Y is not fenced"
    );
    let receipt = direct_run(&fixture.host, Y_ALIAS, &fixture.y_package, 4301);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
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
    let Fixture { host, _root, .. } = fixture;
    host.close().expect("close the host");
}

#[test]
fn idle2_h_a_held_admission_guard_keeps_the_idle_instances_wake_through_the_host() {
    let fixture = fixture();
    let before = head(&fixture.host);

    // 1. X's action succeeds (an unscripted Stop is confirmed).
    fixture
        .host
        .check_idle_instances_for_test(&x_plan(false))
        .expect("the idle check");
    assert_eq!(
        fixture.host.await_idle_action_ends_for_test(1, WAIT),
        Some(1),
        "X's idle action ended"
    );

    // 2. A check while the test holds X's admission guard, as the emulator watch's status read
    // does.
    let admission = fixture
        .host
        .instance_admission_for_test(X_ALIAS)
        .expect("X's admission guard");
    let read = NOW_UNIX_MS + MINUTE_MS;
    fixture.clock.set_unix_ms(read);
    {
        let _read = admission.lock().expect("hold X's admission guard");
        fixture
            .host
            .check_idle_instances_for_test(&x_plan_at(read))
            .expect("the idle check during the read");
    }

    // 3. The guard is free again: a check with a fresh far plan.
    let fresh = read + MINUTE_MS;
    fixture.clock.set_unix_ms(fresh);
    fixture
        .host
        .check_idle_instances_for_test(&x_plan_at(fresh))
        .expect("the idle check after the read");

    // 4. The clock at X's wake: next due minus the default lead.
    let wake = NEXT_DUE_UNIX_MS - STOP_EMULATOR_LEAD_MS;
    fixture.clock.set_unix_ms(wake);
    fixture
        .host
        .check_idle_instances_for_test(&x_plan_at(wake))
        .expect("the idle check at the wake");

    // X's idle records and its wake's claim record, in order.
    let events = all_events(&fixture.host);
    let records = phases(&events)
        .into_iter()
        .filter(|(event, _)| {
            event.sequence() > before && event.links().instance_id() == Some(&fixture.x)
        })
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome } => {
                let code = outcome.code().as_str();
                if code == IdleCode::Taken.as_str() {
                    let operation = outcome
                        .value("operation")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Some(format!("{code} {operation}"))
                } else if code == HostCode::EmulatorAutostartQueued.as_str() {
                    Some(code.to_owned())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        records,
        [
            format!(
                "{} {}",
                IdleCode::Taken.as_str(),
                IdleLocation::Action.as_str()
            ),
            format!(
                "{} {}",
                IdleCode::Taken.as_str(),
                IdleLocation::Wake.as_str()
            ),
            HostCode::EmulatorAutostartQueued.as_str().to_owned(),
        ],
        "one action for X, then its wake and the wake's claim"
    );
    let Fixture { host, _root, .. } = fixture;
    host.close().expect("close the host");
}
