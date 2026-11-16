// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 KB-1 (0.12.69; register §1.447; the owner's ruling on board #463 that a package
//! may press the back key, the coordinator's rulings of 11-16): the host's half of the back key.
//!
//! - The capability table says which inputs send a key: `nemu_ipc` and `adb_shell_input` the back
//!   key alone (their row's failure semantics name it), `maatouch` every key, `minitouch` none;
//!   `input.text` stays `maatouch`'s alone. It follows a backend switch, as HS-2's table does.
//! - On an instance whose input is `nemu_ipc` (configured so, neutral values; the fake provider
//!   opens no Nemu session, so the device's own key calls are `capture/nemu_input/gate_463_kb1.rs`'s):
//!   every other key and a text are refused before anything is sent
//!   (`nemu_input_capability_unsupported`), the back key without a frame
//!   (`nemu_input_frame_required`), and the back key on a frame the Runtime captured reaches the
//!   device, once.
//! - The foreground gate holds a key as it holds a tap: with another application in front both are
//!   refused `application_not_foreground` and nothing reaches the device; with the assigned one in
//!   front the key reaches it.
//!
//! Fix round 1 (the coordinator's rulings R2, R3 and R5 of 11-16):
//!
//! - A back key the Nemu input refuses before it sends anything because its library lacks the key
//!   calls (the device's refusal `input_unsupported`, which the fake input answers here) is denied
//!   `nemu_input_capability_unsupported`: nothing performed, the input's failure record says its
//!   effect not performed, and the lease is kept.
//! - On an instance whose input is `adb_shell_input`, another key is refused before anything is
//!   sent and before an intent is written, as on the Nemu input; the back key reaches the device.
//!
//! Nothing here sleeps or reads a clock.

use super::*;
use actingcommand_contract::{
    EffectiveDeviceConfiguration, EmulatorCapability, EmulatorCapabilityImplementation,
    InputFrameReference,
};

const ALIAS: &str = "neutral.instance";
const NOT_FOREGROUND: &str = "application_not_foreground";
const UNSUPPORTED: &str = "nemu_input_capability_unsupported";
/// The key train: the refusal of a key `adb_shell_input` does not send.
const KEY_UNSUPPORTED: &str = "input_key_unsupported";
const FRAME_REQUIRED: &str = "nemu_input_frame_required";

fn back() -> InputAction {
    InputAction::Key {
        key: "back".to_owned(),
    }
}

/// The instance's input actions the fake device took, in order.
fn device_inputs(state: &FakeState) -> Vec<InputAction> {
    state.input_actions.lock().expect("input actions").clone()
}

/// One keyless observation of the instance on `client`'s connection, and the reference of the
/// frame the Runtime committed for it, as a client names it in an input.
fn observed_frame(client: &mut TestClient) -> InputFrameReference {
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: ALIAS.to_owned(),
    });
    let observed = client.send(&observe);
    let Some(RuntimeResult::ReadonlyObservationCompleted { observation }) = observed.result()
    else {
        panic!("the observation: {observed:?}");
    };
    InputFrameReference {
        frame_id: observation
            .artifact()
            .frame_id
            .expect("an observed frame has its id"),
        width: observation.width(),
        height: observation.height(),
    }
}

/// Fix round 1: a host whose one instance is configured with the touch backend `touch` and the
/// capture backend `capture` (neutral values), on the fake device `state`.
fn configured_host(
    root: &TempDir,
    state: &Arc<FakeState>,
    touch: &str,
    capture: &str,
) -> RuntimeHost {
    let provider = FakeProvider::one(ALIAS, instance_id(), Arc::clone(state));
    let resolved = provider
        .resolve(ALIAS)
        .expect("the instance")
        .with_configuration(EffectiveDeviceConfiguration {
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
        });
    RuntimeHost::start(
        config(root),
        Arc::new(provider.with_resolved_for(ALIAS, resolved)),
    )
    .expect("runtime host")
}

/// `action` under `token`, on `frame` when one is given; its receipt.
fn input(
    client: &mut TestClient,
    token: &LeaseToken,
    frame: Option<InputFrameReference>,
    action: InputAction,
) -> RuntimeReceipt {
    let request = client.request(RuntimeOperation::Input {
        frame,
        token: token.clone(),
        action,
    });
    client.send(&request)
}

// The capability table of an instance says which input sends a key, and follows a switch of its
// backends: minitouch none; adb shell input and the Nemu input the back key alone, which their
// row's failure semantics name; maatouch every key. Text is maatouch's alone. Nothing is opened.
#[test]
fn kb1_the_capability_table_says_which_inputs_send_the_back_key() {
    use actingcommand_device::{
        AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceTarget, MaaTouchConfig,
        TouchBackendChoice, TouchBackendConfig,
    };
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    let target = DeviceTarget::default();
    let registry = ExecutionBackendRegistry::new([ExecutionBackendRegistration::new(
        ALIAS,
        instance_id(),
        "neutral.application",
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::Minitouch),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::Adb),
    )
    .expect("registration")])
    .expect("registry");
    // Whether the row of `capability` says supported, what its failure semantics say and the
    // input it names.
    let row = |registry: &ExecutionBackendRegistry, capability: EmulatorCapability| {
        let resolved = registry.resolve(ALIAS).expect("registered");
        let evidence = resolved
            .capabilities()
            .expect("capabilities")
            .evidence(capability);
        (
            evidence.implementation() == Some(EmulatorCapabilityImplementation::Supported),
            evidence.failure_semantics().to_owned(),
            evidence.evidence_ref().to_owned(),
        )
    };
    let (key, _, source) = row(&registry, EmulatorCapability::InputKey);
    assert!(!key, "minitouch sends no key");
    assert_eq!(source, "minitouch");
    assert!(!row(&registry, EmulatorCapability::InputText).0);

    for (touch, capture, every_key, text) in [
        (
            TouchBackendChoice::AdbShellInput,
            CaptureBackendChoice::Adb,
            false,
            false,
        ),
        (
            TouchBackendChoice::MaaTouch,
            CaptureBackendChoice::Adb,
            true,
            true,
        ),
        (
            TouchBackendChoice::NemuIpc,
            CaptureBackendChoice::NemuIpc,
            false,
            false,
        ),
    ] {
        registry
            .switch_instance_backends(ALIAS, touch, capture)
            .expect("the switch");
        let (key, semantics, source) = row(&registry, EmulatorCapability::InputKey);
        assert!(key, "{touch:?}: input.key supported");
        assert_eq!(source, touch.as_str(), "{touch:?}");
        assert_eq!(
            semantics.contains("for the back key alone (back)"),
            !every_key,
            "{touch:?}: which keys: {semantics}"
        );
        assert_eq!(
            row(&registry, EmulatorCapability::InputText).0,
            text,
            "{touch:?}: input.text"
        );
    }
    let (_, semantics, _) = row(&registry, EmulatorCapability::InputKey);
    assert!(
        semantics.contains("a library without those calls sends no key"),
        "the Nemu input's row: {semantics}"
    );
}

// On an instance whose input is the emulator's own: every other key (by its word, its Android
// code and a word of no key) and a text are refused before anything is sent
// (`nemu_input_capability_unsupported`); the back key without a frame is refused
// (`nemu_input_frame_required`); the back key on a frame the Runtime captured and committed is
// completed and reaches the device once, the key it names.
#[test]
fn kb1_the_host_sends_the_back_key_on_a_frame_of_a_nemu_input_and_refuses_the_rest() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let provider = FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state));
    let resolved = provider
        .resolve(ALIAS)
        .expect("the instance")
        .with_configuration(EffectiveDeviceConfiguration {
            input_backend: "nemu_ipc".to_owned(),
            capture_backend: "nemu_ipc".to_owned(),
            input_adb: "<sealed-test>".to_owned(),
            capture_adb: "<sealed-test>".to_owned(),
            configured_serial: None,
            resolved_serial: "TESTSERIAL0463".to_owned(),
            input_command_timeout_ms: 5_000,
            capture_command_timeout_ms: 5_000,
            capture_timeout_ms: 5_000,
            configured_mumu_root: None,
            configured_capture_dll: None,
        });
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(provider.with_resolved_for(ALIAS, resolved)),
    )
    .expect("runtime host");
    // The capture is a keyless observation on a connection of its own before the lease, as the
    // Lab's `key` takes it.
    let mut observer = TestClient::connect(&host);
    let frame = observed_frame(&mut observer);
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire(ALIAS);

    for action in [
        InputAction::Key {
            key: "home".to_owned(),
        },
        InputAction::Key {
            key: "3".to_owned(),
        },
        InputAction::Key {
            key: "neutral".to_owned(),
        },
        InputAction::Text {
            text: "neutral".to_owned(),
        },
    ] {
        let receipt = input(&mut client, &token, Some(frame), action.clone());
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "{action:?}: {receipt:?}"
        );
        assert_eq!(
            receipt.outcome().code().as_str(),
            UNSUPPORTED,
            "{action:?}: {receipt:?}"
        );
    }
    let receipt = input(&mut client, &token, None, back());
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        FRAME_REQUIRED,
        "{receipt:?}"
    );
    assert!(device_inputs(&state).is_empty(), "nothing was sent");

    let receipt = input(&mut client, &token, Some(frame), back());
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(device_inputs(&state), [back()], "the back key, once");
    let release = client.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(observer);
    drop(client);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The foreground gate holds a key as it holds a tap: with another application in front, the back
// key and a tap are each refused `application_not_foreground` and nothing reaches the device; with
// the assigned application in front again the back key reaches it.
#[test]
fn kb1_the_foreground_gate_holds_a_key_as_it_holds_a_tap() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    *state.foreground_script.lock().expect("foreground script") =
        Some(Arc::new(|_inputs: usize, _applications: usize| {
            Some("neutral.other_application".to_owned())
        }));
    let host = host_with_state(&root, ALIAS, Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire(ALIAS);

    for action in [back(), InputAction::Tap { x: 1, y: 1 }] {
        let receipt = input(&mut client, &token, None, action.clone());
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "{action:?}: {receipt:?}"
        );
        assert_eq!(
            receipt.outcome().code().as_str(),
            NOT_FOREGROUND,
            "{action:?}: {receipt:?}"
        );
    }
    assert!(
        device_inputs(&state).is_empty(),
        "nothing reached the device"
    );

    *state.foreground_script.lock().expect("foreground script") = None;
    let receipt = input(&mut client, &token, None, back());
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(device_inputs(&state), [back()]);
    let release = client.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    host.close().expect("close host");
}

// Fix round 1 (R2, R5): on an instance whose input is the emulator's own, the back key on a frame
// the Runtime captured, which its input refuses before it sends anything because the library lacks
// the key calls (the device's refusal `input_unsupported`), is the capability's refusal: denied
// `nemu_input_capability_unsupported`, nothing performed on the device, the input's one failure
// record saying its effect not performed (nothing recorded as indeterminate, nothing committed),
// and the lease kept, as for a key the host refuses itself.
#[test]
fn kb1_fix1_a_key_the_library_cannot_send_is_denied_and_recorded_not_performed() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    *state.input_error_once.lock().expect("one-shot input error") =
        Some(DeviceError::fatal("neutral library without the key calls").with_input_unsupported());
    let host = configured_host(&root, &state, "nemu_ipc", "nemu_ipc");
    let mut observer = TestClient::connect(&host);
    let frame = observed_frame(&mut observer);
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire(ALIAS);

    let receipt = input(&mut client, &token, Some(frame), back());
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        UNSUPPORTED,
        "{receipt:?}"
    );
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        0,
        "nothing performed"
    );
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("the ledger");
    let effects = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Input(InputPayload::Failed(failed)) => Some(failed.effect_disposition()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        effects,
        [EffectDisposition::NotPerformed],
        "the input's record"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type() == EventType::InputCommitted),
        "nothing committed"
    );
    let release = client.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed,
        "the lease was kept"
    );
    drop(observer);
    drop(client);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Fix round 1 (R3, R5): on an instance whose input is `adb_shell_input`, which sends the back key
// alone, another key (by its word, its Android code and a word of no key) is refused before
// anything is sent and before an intent is written, as on the Nemu input: denied, no input record
// in the ledger, nothing on the device. The back key is then completed and reaches the device
// once. Adapted by the key train (register §1.447, "Train": the coordinator's correction of the
// code, one code for one cause and no other backend's name): the refusal is
// `input_key_unsupported`, the Nemu input keeping its own.
#[test]
fn kb1_fix1_adb_shell_input_refuses_another_key_before_its_intent() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = configured_host(&root, &state, "adb_shell_input", "adb");
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire(ALIAS);

    for key in ["home", "3", "neutral"] {
        let receipt = input(
            &mut client,
            &token,
            None,
            InputAction::Key {
                key: key.to_owned(),
            },
        );
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "{key}: {receipt:?}"
        );
        assert_eq!(
            receipt.outcome().code().as_str(),
            KEY_UNSUPPORTED,
            "{key}: {receipt:?}"
        );
    }
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("the ledger");
    assert!(
        !events.iter().any(|event| matches!(
            event.event_type(),
            EventType::InputIntent | EventType::InputFailed | EventType::InputCommitted
        )),
        "no input record"
    );
    assert!(device_inputs(&state).is_empty(), "nothing was sent");

    let receipt = input(&mut client, &token, None, back());
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(device_inputs(&state), [back()], "the back key, once");
    let release = client.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
