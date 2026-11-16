// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463, the key train (0.12.69; register §1.447 and §1.448, "Train"; coordinator work
//! order `rd-1116/brief-463-train-key.md`, point 4): KP-1's key step and KB-1's input meet, in one
//! run through every layer a host test reaches. The package is KP-1's source package
//! (`gate_463_kp1::write_package`: `open` taps from `home` into `list`, `back` presses the back key
//! from `list` to `done`, guarded by a mark on the list), loaded by the host from its directory as
//! the product loads one, and run directly as `actingctl task-run` sends it, on the test's manual
//! clock. The instance's capability table is the one the Runtime's own registry declares for its
//! input (`capability_profile`), read from a registry of the instance's backends.
//!
//! - On an instance whose input is the emulator's own (`nemu_ipc`, its table saying `input.key`
//!   supported): the run starts, the guard passes on the list's captured frame, the key goes out
//!   bound to that frame (the host refuses a key without one on this input,
//!   `nemu_input_frame_required`, and one the input does not send,
//!   `nemu_input_capability_unsupported`), the device takes exactly one key, the kernel's back key,
//!   and the run goes on to `done`.
//! - On an instance whose input is `minitouch` (its table saying `input.key` unsupported): the run
//!   is refused at its start, `contained_task_linear_key_unsupported` naming `back`, nothing
//!   captured and nothing sent.
//!
//! What it does not reach: the fake provider opens no Nemu session (a `NemuSessionBackends` holds
//! the vendor library's own session, which only the library opens), so below the host the key
//! reaches the fake device's input as a plain key. The session's hand-over of the key with its
//! frame (`execute_action` to `key_in_frame`), the registry's wrapper and the worker's key down and
//! key up on the scripted library are KB-1's tests (`session/gate_463_kb1.rs`,
//! `provider/gate_463_kb1.rs`, `capture/nemu_input/gate_463_kb1.rs`). Nothing here sleeps or
//! reads a clock.

use super::gate_463_kp1::write_package;
use super::instance_workers::all_events;
use super::*;
use actingcommand_contract::{
    EffectiveDeviceConfiguration, EmulatorCapability, EmulatorCapabilityImplementation,
    EmulatorCapabilityProfile,
};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceKey, DeviceTarget, MaaTouchConfig,
    NemuInputSupport, TouchBackendChoice, TouchBackendConfig,
};
use actingcommand_execution_kernel::StepKey;
use serde_json::json;

const KEY_UNSUPPORTED: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearKeyUnsupported.as_str();
/// The alias of the registry the tables are read from (KB-1's registration of a neutral instance).
const REGISTERED_ALIAS: &str = "neutral.instance";

/// The instance's configured backends, as the host reads them (neutral values).
fn configuration(touch: &str, capture: &str) -> EffectiveDeviceConfiguration {
    EffectiveDeviceConfiguration {
        input_backend: touch.to_owned(),
        capture_backend: capture.to_owned(),
        input_adb: "<sealed-test>".to_owned(),
        capture_adb: "<sealed-test>".to_owned(),
        configured_serial: None,
        resolved_serial: "TESTSERIAL0463".to_owned(),
        input_command_timeout_ms: 5_000,
        capture_command_timeout_ms: 5_000,
        capture_timeout_ms: 5_000,
        configured_mumu_root: None,
        configured_capture_dll: None,
    }
}

/// The capability tables the Runtime's registry declares for one instance: on the emulator's own
/// input and capture (`nemu_ipc`), then on `minitouch` with adb capture. Nothing is opened.
fn registry_tables() -> (EmulatorCapabilityProfile, EmulatorCapabilityProfile) {
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    let target = DeviceTarget::default();
    let registry = ExecutionBackendRegistry::new([ExecutionBackendRegistration::new(
        REGISTERED_ALIAS,
        instance_id(),
        "neutral.application",
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::Minitouch),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::Adb),
    )
    .expect("registration")])
    .expect("registry");
    let table = |registry: &ExecutionBackendRegistry| {
        registry
            .resolve(REGISTERED_ALIAS)
            .expect("registered")
            .capabilities()
            .cloned()
            .expect("the registry's capability table")
    };
    let without_key = table(&registry);
    registry
        .switch_instance_backends(
            REGISTERED_ALIAS,
            TouchBackendChoice::NemuIpc,
            CaptureBackendChoice::NemuIpc,
        )
        .expect("the switch");
    (table(&registry), without_key)
}

/// Whether `table`'s row `input.key` says the input sends keys.
fn sends_key(table: &EmulatorCapabilityProfile) -> bool {
    table
        .evidence(EmulatorCapability::InputKey)
        .implementation()
        == Some(EmulatorCapabilityImplementation::Supported)
}

/// One direct run of the package: its receipt, the host's ledger and the fake device.
struct DirectRun {
    receipt: RuntimeReceipt,
    events: Vec<PersistedEvent>,
    device: Arc<FakeState>,
}

/// Runs the package directly on a device instance configured with the touch backend `touch` and
/// the capture backend `capture`, whose capability table is `table`. The device shows `home`
/// before any input, `list` (its mark kept) after one and `done` after two.
fn run_on(touch: &str, capture: &str, table: EmulatorCapabilityProfile) -> DirectRun {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("train-key-back");
    write_package(&package);
    let device = Arc::new(FakeState::default());
    *device.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(|_: usize, inputs: usize| match inputs {
            0 => Vec::<(usize, [u8; 3])>::new(),
            1 => vec![(0_usize, [0_u8, 0, 255])],
            _ => vec![(0_usize, [255_u8, 255, 255])],
        }));
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    device.physical_task_geometry.store(true, Ordering::Release);
    *device
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let provider = FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&device));
    let resolved = provider
        .resolve(POLICY_INSTANCE_ALIAS)
        .expect("the instance")
        .with_configuration(configuration(touch, capture))
        .with_capabilities(table);
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(provider.with_resolved_for(POLICY_INSTANCE_ALIAS, resolved)),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            POLICY_INSTANCE_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.to_string_lossy().into_owned())
                .expect("task request"),
        ),
    );
    let receipt = host
        .process_request_for_test(&run, ConnectionId::new(463).expect("connection"))
        .expect("run receipt");
    let events = all_events(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
    DirectRun {
        receipt,
        events,
        device,
    }
}

// The key train, through the layers: the key the kernel sends is the device layer's back key; on
// the emulator's own input the run with the key step starts, its key goes out once, bound to the
// frame its guard passed on, and the run goes on to its next page; on an input whose table declares
// no key the run is refused at its start, nothing captured, nothing sent.
#[test]
fn train_key_a_key_step_reaches_the_device_on_its_frame_or_is_refused_at_the_start() {
    // One spelling: the kernel's back key is the one the device layer names, and the emulator's
    // input takes it on a frame.
    assert_eq!(
        DeviceKey::named(StepKey::Back.input_key()),
        Some(DeviceKey::Back)
    );
    assert_eq!(
        NemuInputSupport::of(&StepKey::Back.action()),
        NemuInputSupport::InFrame
    );
    let (nemu_table, minitouch_table) = registry_tables();
    assert!(sends_key(&nemu_table), "the Nemu input's row input.key");
    assert!(!sends_key(&minitouch_table), "minitouch's row input.key");

    // The emulator's own input: Completed on `done` after the two steps.
    let sent = run_on("nemu_ipc", "nemu_ipc", nemu_table);
    assert_eq!(
        sent.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        sent.receipt.outcome()
    );
    assert!(
        matches!(
            sent.receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                final_page: Some(final_page),
                executed_steps: 2,
                ..
            }) if final_page == "neutral/done"
        ),
        "{:?}",
        sent.receipt.result()
    );
    // The device took the tap, then exactly one key: the kernel's back key.
    let inputs = sent
        .device
        .input_actions
        .lock()
        .expect("input actions")
        .clone();
    let [InputAction::Tap { .. }, key] = inputs.as_slice() else {
        panic!("the tap and one key: {inputs:?}");
    };
    assert_eq!(key, &StepKey::Back.action(), "the back key");
    // The key's input record is bound to the frame its step's intent names, the last frame
    // captured before it, captured after the tap was committed; its source is that step.
    let step_intent = sent
        .events
        .iter()
        .find(|event| {
            matches!(
                event.payload(),
                EventPayload::Task(TaskPayload::Semantic(payload))
                    if matches!(
                        payload.fact(),
                        TaskSemanticFact::EffectIntent { operation_label, .. }
                            if operation_label == "back"
                    )
            )
        })
        .expect("the key step's effect intent");
    let key_records = sent
        .events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Input(InputPayload::Intent(input)) => input
                .provenance()
                .filter(|provenance| matches!(provenance.input_action, InputAction::Key { .. }))
                .map(|provenance| (event.sequence(), provenance.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(key_sequence, provenance)] = key_records.as_slice() else {
        panic!("one input record of the key: {key_records:?}");
    };
    assert!(
        provenance.before_frame_id.is_some(),
        "the key names its frame"
    );
    assert_eq!(
        provenance.before_frame_id.as_ref(),
        step_intent.links().frame_id(),
        "the frame of the key step's intent"
    );
    assert_eq!(
        provenance.source_step_action_id.as_ref(),
        step_intent.links().action_id(),
        "the key step's action"
    );
    let last_capture = sent
        .events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::CaptureCompleted && event.sequence() < *key_sequence
        })
        .max_by_key(|event| event.sequence())
        .expect("a frame captured before the key");
    assert_eq!(
        last_capture.links().frame_id(),
        provenance.before_frame_id.as_ref(),
        "the last frame captured before the key"
    );
    let tap_committed = sent
        .events
        .iter()
        .filter(|event| event.event_type() == EventType::InputCommitted)
        .min_by_key(|event| event.sequence())
        .expect("the tap committed");
    assert!(
        tap_committed.sequence() < last_capture.sequence(),
        "the key's frame was captured after the tap"
    );
    assert!(
        !sent
            .events
            .iter()
            .any(|event| event.event_type() == EventType::InputFailed),
        "no input failed"
    );

    // An input whose table declares no key: refused at the start, nothing captured, nothing sent.
    let refused = run_on("minitouch", "adb", minitouch_table);
    assert_eq!(
        refused.receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        refused.receipt
    );
    assert_eq!(
        refused.receipt.outcome().code().as_str(),
        KEY_UNSUPPORTED,
        "{:?}",
        refused.receipt
    );
    assert_eq!(
        refused
            .receipt
            .outcome()
            .value("pack_operation_id")
            .cloned(),
        Some(json!("back"))
    );
    assert_eq!(
        refused.device.capture_count.load(Ordering::Acquire),
        0,
        "nothing captured"
    );
    assert!(
        refused
            .device
            .input_actions
            .lock()
            .expect("input actions")
            .is_empty(),
        "nothing sent"
    );
    let records = refused
        .events
        .iter()
        .map(PersistedEvent::event_type)
        .filter(|kind| {
            matches!(
                kind,
                EventType::CaptureRequested
                    | EventType::CaptureCompleted
                    | EventType::InputIntent
                    | EventType::InputCommitted
                    | EventType::InputFailed
                    | EventType::TaskEffectIntent
            )
        })
        .collect::<Vec<_>>();
    assert!(records.is_empty(), "{records:?}");
}
