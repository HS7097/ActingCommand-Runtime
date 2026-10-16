// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 九处-1 (board #390 comment 6095942731, item 1; the coordinator's corrected ruling
// 4 of wave 18; host gate 101): the status shows each instance's emulator running state as the
// Runtime last read it, with the time of that reading. The Runtime reads it at the start's
// discovery (a discovery-bound instance bound to its port is on, one bound pending is off), at a
// confirmed stop (off) and start (on), and at an on-demand discovery. An instance it never read,
// an explicit HOST:PORT one, is unknown, never off. The clock is manual: each reading's time lies
// between the clock before and after the step that took it.

use super::*;
use actingcommand_contract::{EmulatorInstanceAction, EmulatorRunningState};

const RUNNING: &str = "nine1.running";
const STOPPED: &str = "nine1.stopped";
const EXPLICIT: &str = "nine1.explicit";
// The discovered index of the emulator `STOPPED` is bound to; `RUNNING` is the fake emulator's
// own index (`FAKE_EMULATOR_INDEX`).
const STOPPED_INDEX: u16 = 1;
const STOPPED_PORT: u16 = 16_416;
const EXPLICIT_PORT: u16 = 16_500;
// The manual clock at the start and before each later step; far enough apart that a step's own
// waits never reach the next one.
const START_MS: u64 = 1_000_000_000;
const STOP_MS: u64 = 2_000_000_000;
const RESTART_MS: u64 = 3_000_000_000;
const DISCOVERY_MS: u64 = 4_000_000_000;

fn send(host: &RuntimeHost, operation: RuntimeOperation, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.process_request_for_test(
        &runtime_request(&ids, operation),
        ConnectionId::new(connection).expect("connection"),
    )
    .expect("receipt")
}

// `actingctl emulator <action>` of `alias`, completed.
fn control(host: &RuntimeHost, alias: &str, action: EmulatorInstanceAction, connection: u64) {
    let receipt = send(
        host,
        RuntimeOperation::ControlEmulatorInstance {
            instance_alias: alias.to_owned(),
            action,
        },
        connection,
    );
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{alias} {action:?}: {receipt:?}"
    );
}

// Each instance's `emulator_state` in the Runtime's status, by alias.
fn emulator_states(host: &RuntimeHost, connection: u64) -> BTreeMap<String, EmulatorRunningState> {
    let receipt = send(host, RuntimeOperation::Status, connection);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("expected the status, got {receipt:?}");
    };
    status
        .instances()
        .iter()
        .map(|instance| {
            (
                instance.instance_alias().to_owned(),
                instance.emulator_state(),
            )
        })
        .collect()
}

// The manual clock's Unix time, as the Runtime samples it.
fn clock_unix_ms(clock: &ManualRuntimeClock) -> u64 {
    clock.sample().expect("manual clock").unix_ms
}

// `state` is a reading of `running` taken between `from` and `to`.
#[track_caller]
fn assert_read(state: EmulatorRunningState, running: bool, from: u64, to: u64) {
    let observed_at_unix_ms = match (state, running) {
        (
            EmulatorRunningState::On {
                observed_at_unix_ms,
            },
            true,
        )
        | (
            EmulatorRunningState::Off {
                observed_at_unix_ms,
            },
            false,
        ) => observed_at_unix_ms,
        _ => panic!("expected a reading running={running}, got {state:?}"),
    };
    assert!(
        (from..=to).contains(&observed_at_unix_ms),
        "the reading's time {observed_at_unix_ms} is not within [{from}, {to}]"
    );
}

#[test]
fn nine1_the_status_shows_each_emulator_as_the_runtime_last_read_it() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(START_MS, 0));
    let running = Arc::new(FakeState::default());
    let stopped = Arc::new(FakeState::default());
    let explicit = Arc::new(FakeState::default());
    // Opens that pass a preparation's self-check, as the S6c gate's instances have.
    for state in [&running, &explicit] {
        state.physical_task_geometry.store(true, Ordering::Release);
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        state.observed_open.store(true, Ordering::Release);
    }
    // `RUNNING` is discovery-bound to the fake emulator, which runs on its port.
    running.emulator_control.store(true, Ordering::Release);
    let stopped_id = instance_id();
    let explicit_id = instance_id();
    let pending = ResolvedExecutionInstance::new(stopped_id, format!("127.0.0.1:{STOPPED_PORT}"))
        .with_pending_endpoint(actingcommand_execution_kernel::PendingAdbEndpoint::new(
            "127.0.0.1",
            actingcommand_execution_kernel::DiscoveredInstanceBinding::new(
                STOPPED_INDEX,
                "fixture-stopped",
                "1.0.0",
                "C:/fixture/MuMuManager.exe",
            ),
        ));
    let bound_explicitly =
        ResolvedExecutionInstance::new(explicit_id, format!("127.0.0.1:{EXPLICIT_PORT}"))
            .with_adb_endpoint(actingcommand_execution_kernel::ResolvedAdbEndpoint::new(
                "127.0.0.1",
                EXPLICIT_PORT,
                false,
            ));
    // An on-demand discovery reports the fake emulator stopped and `STOPPED`'s emulator running.
    let discovered = |instance_index: u16, adb_port: Option<u16>| ProviderDiscoveredInstance {
        instance_index,
        instance_name: format!("fixture-{instance_index}"),
        adb_host: adb_port.map(|_| "127.0.0.1".to_owned()),
        adb_port,
        running: adb_port.is_some(),
        android_version: None,
    };
    let provider = FakeProvider::from_entries([
        (RUNNING.to_owned(), instance_id(), Arc::clone(&running)),
        (STOPPED.to_owned(), stopped_id, Arc::clone(&stopped)),
        (EXPLICIT.to_owned(), explicit_id, Arc::clone(&explicit)),
    ])
    .with_resolved_for(STOPPED, pending)
    .with_resolved_for(EXPLICIT, bound_explicitly)
    .with_discovery(ProviderInstanceDiscovery {
        provider_version: "fake.provider.1".to_owned(),
        instances: vec![
            discovered(FAKE_EMULATOR_INDEX, None),
            discovered(STOPPED_INDEX, Some(STOPPED_PORT)),
        ],
    });
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(provider),
    )
    .expect("runtime host");

    // The start's discovery: bound to its port is on, bound pending is off, an explicit HOST:PORT
    // instance was never read.
    let started = clock_unix_ms(&clock);
    let states = emulator_states(&host, 1);
    assert_read(states[RUNNING], true, START_MS, started);
    assert_read(states[STOPPED], false, START_MS, started);
    assert_eq!(states[EXPLICIT], EmulatorRunningState::Unknown);

    // A confirmed stop is a reading: off, at the stop.
    clock.set_unix_ms(STOP_MS);
    control(&host, RUNNING, EmulatorInstanceAction::Stop, 2);
    let stopped_at = clock_unix_ms(&clock);
    let states = emulator_states(&host, 3);
    assert_read(states[RUNNING], false, STOP_MS, stopped_at);
    assert_read(states[STOPPED], false, START_MS, started);
    assert_eq!(states[EXPLICIT], EmulatorRunningState::Unknown);

    // A confirmed start is a reading: on, at the start.
    clock.set_unix_ms(RESTART_MS);
    control(&host, RUNNING, EmulatorInstanceAction::Start, 4);
    let restarted_at = clock_unix_ms(&clock);
    let states = emulator_states(&host, 5);
    assert_read(states[RUNNING], true, RESTART_MS, restarted_at);
    assert_eq!(states[EXPLICIT], EmulatorRunningState::Unknown);

    // An on-demand discovery is a reading of every discovery-bound instance; the explicit one
    // is still unknown.
    clock.set_unix_ms(DISCOVERY_MS);
    let receipt = send(&host, RuntimeOperation::DiscoverInstances, 6);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let discovered_at = clock_unix_ms(&clock);
    let states = emulator_states(&host, 7);
    assert_read(states[RUNNING], false, DISCOVERY_MS, discovered_at);
    assert_read(states[STOPPED], true, DISCOVERY_MS, discovered_at);
    assert_eq!(states[EXPLICIT], EmulatorRunningState::Unknown);

    host.close().expect("close host");
}
