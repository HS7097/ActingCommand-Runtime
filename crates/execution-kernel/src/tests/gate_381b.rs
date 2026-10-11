// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 B, test plan E-2 G2 (EXEC-I2): the kernel drives emulator control and
//! instance discovery on the provider directly, opening and touching no device session; it
//! refuses to rebind an endpoint while a session is open and leaves the binding as it was; a
//! Start binds the port it reported and a Stop returns the binding to pending
//! (`contracts/emulator-control.md`, "Cold start" and the registry paragraph).

use super::emulator_controller::{
    EmulatorCall, FAKE_EMULATOR_PORT, FakeEmulatorController, ScriptedControl,
};
use super::{FakeProvider, FakeState, instance, step_witness};
use crate::{ExecutionKernel, ResolvedInstanceEndpoint};
use actingcommand_contract::{EmulatorInstanceAction, InputAction, InstanceId};
use std::sync::{Arc, Mutex};

const ALIAS: &str = "node.a";

fn emulator_kernel(
    state: &Arc<Mutex<FakeState>>,
    controller: &Arc<FakeEmulatorController>,
) -> (ExecutionKernel, InstanceId) {
    let instance_id = instance();
    let provider = FakeProvider::new(Arc::clone(state), &[(ALIAS, instance_id, "private-a")])
        .with_emulator(ALIAS, Arc::clone(controller));
    (ExecutionKernel::new(Arc::new(provider)), instance_id)
}

/// Every session open, use and close the fake backends counted.
fn session_counts(state: &Mutex<FakeState>) -> [usize; 6] {
    let state = state.lock().expect("state");
    [
        state.input_opens,
        state.capture_opens,
        state.input_calls,
        state.capture_calls,
        state.input_closes,
        state.capture_closes,
    ]
}

fn binding(kernel: &ExecutionKernel) -> Option<ResolvedInstanceEndpoint> {
    kernel
        .resolve(ALIAS)
        .expect("the instance resolves")
        .adb_endpoint()
        .cloned()
}

fn bound_port(kernel: &ExecutionKernel) -> Option<u16> {
    binding(kernel)
        .as_ref()
        .and_then(ResolvedInstanceEndpoint::bound)
        .map(|endpoint| endpoint.port())
}

#[test]
fn gate_emulator_control_and_discovery_open_and_touch_no_device_session() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let controller = Arc::new(FakeEmulatorController::default());
    let (kernel, instance_id) = emulator_kernel(&state, &controller);
    kernel
        .input(ALIAS, InputAction::Reset, step_witness())
        .expect("an input opens the instance's session");
    let before = session_counts(&state);
    let stopped = kernel
        .control_instance(ALIAS, EmulatorInstanceAction::Stop)
        .expect("the scripted Stop");
    assert!(!stopped.process_started);
    kernel.discover_instances().expect("the scripted discovery");
    assert_eq!(
        session_counts(&state),
        before,
        "no session was opened, used or closed"
    );
    assert!(kernel.has_session(instance_id).expect("session state"));
    assert_eq!(
        controller.calls(),
        [
            EmulatorCall::Control(EmulatorInstanceAction::Stop),
            EmulatorCall::Discover,
        ]
    );
}

#[test]
fn gate_a_rebind_while_a_device_session_is_open_is_refused_and_the_binding_stays() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let controller = Arc::new(FakeEmulatorController::default());
    let (kernel, instance_id) = emulator_kernel(&state, &controller);
    kernel
        .input(ALIAS, InputAction::Reset, step_witness())
        .expect("an input opens the instance's session");
    assert!(
        kernel.rebind_discovered_endpoint(ALIAS, None).is_err(),
        "a session must not outlive the endpoint it was opened on"
    );
    assert_eq!(bound_port(&kernel), Some(FAKE_EMULATOR_PORT));
    assert!(
        !controller
            .calls()
            .iter()
            .any(|call| matches!(call, EmulatorCall::Rebind(_))),
        "{:?}",
        controller.calls()
    );
    assert!(kernel.has_session(instance_id).expect("session state"));
}

#[test]
fn gate_a_start_binds_the_reported_port_and_a_stop_returns_the_binding_to_pending() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let controller = Arc::new(FakeEmulatorController::pending());
    controller.script_control(
        EmulatorInstanceAction::Start,
        ScriptedControl::Running {
            adb_port: Some(16448),
        },
    );
    let (kernel, _) = emulator_kernel(&state, &controller);
    assert!(
        binding(&kernel).is_some_and(|endpoint| endpoint.is_pending()),
        "a stopped instance starts pending"
    );
    let started = kernel
        .control_instance(ALIAS, EmulatorInstanceAction::Start)
        .expect("the scripted Start");
    kernel
        .rebind_discovered_endpoint(ALIAS, started.adb_port)
        .expect("bind the reported port");
    assert_eq!(bound_port(&kernel), Some(16448));
    let stopped = kernel
        .control_instance(ALIAS, EmulatorInstanceAction::Stop)
        .expect("the scripted Stop");
    kernel
        .rebind_discovered_endpoint(ALIAS, stopped.adb_port)
        .expect("return to pending");
    assert!(
        binding(&kernel).is_some_and(|endpoint| endpoint.is_pending()),
        "a stopped instance is pending again"
    );
    assert_eq!(session_counts(&state), [0; 6], "no session was involved");
}
