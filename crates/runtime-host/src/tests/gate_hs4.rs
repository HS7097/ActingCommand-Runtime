// SPDX-License-Identifier: AGPL-3.0-only

//! HS-4 (#402; design `rd-1031/backend-switch/model-backend-switch.md` §4.4; register §1.268):
//! on an instance whose input is `nemu_ipc`, the reset a client owes after its cancelled direct
//! run (`SafeReset` on the run's connection, `host/contained_task.rs`) passes the host's Nemu
//! input gate without a frame and reaches the device; before, the gate refused it
//! (`nemu_input_capability_unsupported`), and an instance pause waiting for that reset never
//! handed the device back.
//!
//! The instance is configured as a Nemu one (neutral test values); the fake provider opens no
//! Nemu session, so the device's own reset is `capture/nemu_input/gate_hs4.rs`'s. The run is
//! cancelled by its deadline on a manual clock, before any input; nothing here sleeps.

use super::*;
use actingcommand_contract::EffectiveDeviceConfiguration;

const ALIAS: &str = "neutral.instance";

#[test]
fn hs4_cancelled_run_reset_succeeds_on_nemu() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("cancelled-task.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write package");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let x = instance_id();
    let provider = FakeProvider::one(ALIAS, x, Arc::clone(&state));
    let resolved = provider
        .resolve(ALIAS)
        .expect("the instance")
        .with_configuration(EffectiveDeviceConfiguration {
            input_backend: "nemu_ipc".to_owned(),
            capture_backend: "nemu_ipc".to_owned(),
            input_adb: "<sealed-test>".to_owned(),
            capture_adb: "<sealed-test>".to_owned(),
            configured_serial: None,
            resolved_serial: "TESTSERIAL0004".to_owned(),
            input_command_timeout_ms: 5_000,
            capture_command_timeout_ms: 5_000,
            capture_timeout_ms: 5_000,
            configured_mumu_root: None,
            configured_capture_dll: None,
        });
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(provider.with_resolved_for(ALIAS, resolved)),
    )
    .expect("runtime host");
    let connection = ConnectionId::new(4404).expect("connection");
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    // The client's run, cancelled by its deadline before any input.
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string())
                .expect("contained task request")
                .with_response_deadline_ms(25)
                .expect("bounded task deadline"),
        ),
    );
    let checkpoint_clock = Arc::clone(&clock);
    let checkpoint = host
        .run_at_contained_task_checkpoint_for_test(run.request_id(), x, None, move |_| {
            checkpoint_clock.advance(25)
        })
        .expect("the deadline checkpoint");
    let receipt = host
        .process_request_for_test(&run, connection)
        .expect("the run's receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Cancelled,
        "{receipt:#?}"
    );
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCancelled { .. })
        ),
        "{receipt:#?}"
    );
    assert_eq!(checkpoint.consumed(), 1);
    assert!(
        state
            .input_actions
            .lock()
            .expect("input actions")
            .is_empty(),
        "the run made no input"
    );

    // The reset the client owes, on the run's connection: no frame, and the Nemu gate admits it.
    let reset = runtime_request(
        &ids,
        RuntimeOperation::safe_reset(ALIAS, ids.mint_holder_id().expect("holder")),
    );
    let receipt = host
        .process_request_for_test(&reset, connection)
        .expect("the reset's receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:#?}"
    );
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::SafeResetCompleted { .. })
        ),
        "{receipt:#?}"
    );
    assert_eq!(
        *state.input_actions.lock().expect("input actions"),
        [InputAction::Reset],
        "the device was reset once"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
