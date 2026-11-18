// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #479 F-2 (register §1.482; contracts/device-binding.md, "An iOS handset"; the
// coordinator's rulings 6 and 7 in work order rd-1118/brief-479-f2-ios-handset-config-binding.md):
// an iOS handset is an instance the Runtime knows. Bound, its `runtime.instance_bound` carries the
// private file's key as its fingerprint, as it is, in the record of an Android handset's shape; it
// is never available to policy (`device_system:ios`), bound or not; the program facts the Lab's
// status shows say its system and whether it is bound; and the device registry refuses every
// capture, input and application operation on it before anything is opened, with the one cause
// `device_system_backend_missing`. The registrations are the registry's own; the key is a neutral
// fake, and no identifier of a phone exists anywhere here.
//
// Fix round 1 (rd-1119/brief-479-f2-fix1.md, items 1 and 2): through the host, a person's tap, key
// (one the stand-in's key table holds and one it does not), swipe, capture and application launch
// on an iOS handset are each refused at the front of the host's admission with the operation's
// own failure code and that cause, before any key table, foreground gate, baseline or backend of
// the stand-in is looked at; and a start with an iOS handset bound and one unbound beside an
// emulator attempts neither handset (no preparation, no failure record naming it, no ladder, no
// idle step) while the emulator starts as it does alone. `business_step` is test-only, as the
// other gate files of this crate keep the scheduler's issuing bridge (architecture guard
// `fenced_close_requires_scheduler_witness`).

use super::gate_398_rd1b::{last_availability, resolved_handset};
use super::gate_398_rd2::{fact, instance_facts};
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::{HostCode, HostLocation, IdleCode};
use actingcommand_contract::codes::{DeviceSelfCheckFailure, SharedCode};
use actingcommand_contract::{
    DeviceBoundVia, FencedWrite, FencedWritePurpose, InstanceDeviceKind, RecoveryTriggerStage,
    RuntimeConfigManifest, RuntimeInstanceBindingPayload, RuntimeLifecyclePhase, RuntimePayload,
    Sha256Fingerprint, issue_fenced_write,
};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceSerial, DeviceTarget,
    MaaTouchConfig, TouchBackendChoice, TouchBackendConfig,
};

/// The key the private file holds for the phone, made under the installation's own secret.
const KEY: &str = "sha256:4790f2000000000000000000000000000000000000000000000000000000f2a1";
const BOUND_AT_UNIX_MS: u64 = 1_700_000_000_479;

/// An iOS handset bound by [`KEY`].
fn bound_ios() -> HandsetEndpoint {
    HandsetEndpoint::Ios(Some(KeyedHandsetBinding::new(
        Sha256Fingerprint::stored(KEY).expect("a key of the fingerprint's form"),
        BOUND_AT_UNIX_MS,
        DeviceBoundVia::Cli,
    )))
}

/// The registration of the iOS handset `alias`, as actingd registers one: stand-in backends on the
/// stand-in target of an unbound handset, an adb of a neutral name that never runs.
fn ios_registration(
    alias: &str,
    instance_id: InstanceId,
    handset: HandsetEndpoint,
) -> ExecutionBackendRegistration {
    let target = DeviceTarget {
        serial: Some(DeviceSerial::unbound_handset(alias)),
        port: 0,
        connect: false,
        ..DeviceTarget::default()
    };
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    ExecutionBackendRegistration::new(
        alias,
        instance_id,
        "neutral.application",
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::AdbShellInput),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::Adb),
    )
    .expect("iOS handset registration")
    .with_handset(handset)
}

/// A business step of a lease on `instance_id`, as the host admits an input with.
#[cfg(test)]
fn business_step(instance_id: InstanceId) -> Arc<FencedWrite> {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    Arc::new(issue_fenced_write(
        LeaseToken::new(
            *ids.mint_owner_epoch().expect("owner epoch").transport(),
            *ids.mint_lease_id().expect("lease").transport(),
            instance_id,
            *ids.mint_holder_id().expect("holder").transport(),
            100,
        )
        .expect("a lease token"),
        1,
        std::num::NonZeroU64::new(1).expect("step"),
        FencedWritePurpose::Business,
    ))
}

/// Every `runtime.instance_bound` of `instance_alias` in `events`.
fn bindings_of<'a>(
    events: &'a [PersistedEvent],
    instance_alias: &str,
) -> Vec<&'a RuntimeInstanceBindingPayload> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::InstanceBound(binding))
                if binding.instance_alias() == instance_alias =>
            {
                Some(binding)
            }
            _ => None,
        })
        .collect()
}

/// Ruling 6: a bound iOS handset's `runtime.instance_bound` names no ADB host or port and records
/// its device with the stored key as its fingerprint, as it is (no fingerprint is made of it), the
/// time and the channel; its device member has exactly the members of an Android handset's.
#[test]
fn f2_a_bound_ios_handsets_binding_records_its_stored_key_as_its_fingerprint() {
    let root = TempDir::new().expect("tempdir");
    let instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id,
                Arc::new(FakeState::default()),
            )
            .with_resolved_for(
                POLICY_INSTANCE_ALIAS,
                resolved_handset(POLICY_INSTANCE_ALIAS, instance_id, bound_ios()),
            ),
        ),
    )
    .expect("runtime host");
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    host.close().expect("close host");

    let [binding] = bindings_of(&events, POLICY_INSTANCE_ALIAS)[..] else {
        panic!("one instance binding");
    };
    assert_eq!((binding.adb_host(), binding.adb_port()), (None, None));
    assert!(binding.serial_configured());
    let device = binding.device().expect("the handset's device");
    assert_eq!(device.kind(), InstanceDeviceKind::Handset);
    assert_eq!(
        device.fingerprint().map(Sha256Fingerprint::as_str),
        Some(KEY)
    );
    assert_eq!(device.bound_at_unix_ms(), Some(BOUND_AT_UNIX_MS));
    assert_eq!(device.bound_via(), Some(DeviceBoundVia::Cli));
    // Ruling 7, by construction: the record's device carries these members and no other.
    let members = serde_json::to_value(device)
        .expect("the device's JSON")
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        members,
        BTreeSet::from([
            "kind".to_owned(),
            "fingerprint".to_owned(),
            "bound_at_unix_ms".to_owned(),
            "bound_via".to_owned(),
        ])
    );
}

/// Ruling 6: an iOS handset, bound or not, is never available to policy, under
/// `device_system:ios` (the unbound handset's row is the model), whatever its self-check says;
/// its program facts say its system and whether it is bound, which the Lab's status shows; an
/// unbound one records `{kind: handset}` alone.
#[test]
fn f2_an_ios_handset_is_never_available_and_its_facts_say_its_system_and_binding() {
    let root = TempDir::new().expect("tempdir");
    let (bound, unbound) = (instance_id(), instance_id());
    // Both fakes would pass a self-check: the device system alone keeps them unavailable.
    let opened = || {
        let state = Arc::new(FakeState::default());
        state.observed_open.store(true, Ordering::Release);
        state
    };
    let host = RuntimeHost::start(
        config(&root).with_policy_inputs(PolicyInputSnapshot::new(
            pending_policy_facts(),
            pending_policy_resources(),
        )),
        Arc::new(
            FakeProvider::from_entries([
                (POLICY_INSTANCE_ALIAS.to_owned(), bound, opened()),
                (POLICY_INSTANCE_ALIAS_B.to_owned(), unbound, opened()),
            ])
            .with_device_endpoints()
            .with_resolved_for(
                POLICY_INSTANCE_ALIAS,
                resolved_handset(POLICY_INSTANCE_ALIAS, bound, bound_ios()),
            )
            .with_resolved_for(
                POLICY_INSTANCE_ALIAS_B,
                resolved_handset(POLICY_INSTANCE_ALIAS_B, unbound, HandsetEndpoint::Ios(None)),
            ),
        ),
    )
    .expect("the start goes on with iOS handsets");
    assert!(host.fatal_error().expect("runtime health").is_none());
    let bound_facts = instance_facts(&host, bound);
    let unbound_facts = instance_facts(&host, unbound);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    host.close().expect("close host");

    for alias in [POLICY_INSTANCE_ALIAS, POLICY_INSTANCE_ALIAS_B] {
        assert_eq!(
            last_availability(&events, alias),
            (false, "device_system:ios".to_owned()),
            "{alias}"
        );
    }
    assert_eq!(
        fact(&bound_facts, "device.system"),
        Some(ContractFactValue::String("ios".to_owned()))
    );
    assert_eq!(
        fact(&bound_facts, "device.bound"),
        Some(ContractFactValue::Boolean(true))
    );
    assert_eq!(
        fact(&unbound_facts, "device.system"),
        Some(ContractFactValue::String("ios".to_owned()))
    );
    assert_eq!(
        fact(&unbound_facts, "device.bound"),
        Some(ContractFactValue::Boolean(false))
    );
    let [binding] = bindings_of(&events, POLICY_INSTANCE_ALIAS_B)[..] else {
        panic!("one binding of the unbound handset");
    };
    let device = binding.device().expect("the handset's device");
    assert_eq!(
        (device.kind(), device.fingerprint()),
        (InstanceDeviceKind::Handset, None)
    );
}

/// Ruling 6: on an iOS handset, bound or not, a capture, an input and an application request are
/// each refused by the device registry before anything is opened (the registry's adb is a name
/// that never runs, and no backend is opened), with the one cause `device_system_backend_missing`
/// beside the operation's own code.
#[test]
fn f2_a_capture_an_input_and_an_application_request_are_refused_before_anything_opens() {
    const ALIAS: &str = "neutral.iphone";
    for handset in [bound_ios(), HandsetEndpoint::Ios(None)] {
        let instance_id = instance_id();
        // A kernel of its own for each operation, on the registry's own registration.
        let kernel = || {
            let registry = ExecutionBackendRegistry::new([ios_registration(
                ALIAS,
                instance_id,
                handset.clone(),
            )])
            .expect("the registry");
            ExecutionKernel::new(Arc::new(registry))
        };
        let on_capture = kernel();
        let capture = on_capture
            .capture(ALIAS)
            .expect_err("no capture opens on an iOS handset");
        let on_input = kernel();
        let input = on_input
            .input(ALIAS, InputAction::Reset, business_step(instance_id))
            .expect_err("no input opens on an iOS handset");
        let on_application = kernel();
        let application = on_application
            .control_application(
                ALIAS,
                ApplicationLifecycleAction::Launch,
                business_step(instance_id),
            )
            .expect_err("no application control runs on an iOS handset");
        for (location, refused) in [
            (HostLocation::ExecuteCaptureBackend, &capture),
            (HostLocation::ExecuteInputBackend, &input),
            (HostLocation::ControlApplication, &application),
        ] {
            assert!(
                refused.failure_context().device_system_unsupported(),
                "{location:?}: the registry's refusal, before anything ran"
            );
            let outcome = RuntimeHostError::execution(location, refused).outcome();
            assert!(
                outcome.causes().iter().any(|link| {
                    link.relation().as_str() == "caused_by"
                        && link.outcome().is(HostCode::DeviceSystemBackendMissing)
                }),
                "{location:?}: {outcome:?}"
            );
        }
        for kernel in [on_capture, on_input, on_application] {
            kernel
                .close()
                .expect("nothing was opened, so nothing is left");
        }
    }
}

/// `receipt` is the denial of an operation on an iOS handset: `code`, the operation's own failure
/// code, caused by `device_system_backend_missing`.
fn refused_by_device_system(receipt: &RuntimeReceipt, code: &str) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let outcome = receipt.outcome();
    assert_eq!(outcome.code().as_str(), code, "{receipt:?}");
    assert!(
        outcome.causes().iter().any(|link| {
            link.relation().as_str() == "caused_by"
                && link.outcome().is(HostCode::DeviceSystemBackendMissing)
        }),
        "{receipt:?}"
    );
}

/// The counts of what the fake behind an instance was asked: input opens, inputs, capture opens,
/// captures and application commands.
fn asked(state: &FakeState) -> [usize; 5] {
    [
        state.open_count.load(Ordering::Acquire),
        state.input_count.load(Ordering::Acquire),
        state.capture_open_count.load(Ordering::Acquire),
        state.capture_count.load(Ordering::Acquire),
        state.application_count.load(Ordering::Acquire),
    ]
}

/// Fix round 1, item 1: through the host, a person's tap, key inside the stand-in's key table
/// (`back`) and outside it, swipe, capture and application launch on a bound iOS handset are each
/// denied with the operation's own failure code caused by `device_system_backend_missing`. None
/// reaches the stand-in: the foreground gate never asks (the fake's one foreground failure, which
/// would have refused the tap as `application_foreground_unknown` and dropped the ADB baseline, is
/// still there), the key table never refuses (`input_key_unsupported`), and nothing is opened,
/// sent, captured or commanded on the fake behind it.
#[test]
fn f2_fix1_a_tap_a_key_a_swipe_a_capture_and_a_launch_are_refused_at_the_hosts_front() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let instance_id = instance_id();
    let state = Arc::new(FakeState::default());
    *state
        .foreground_error_once
        .lock()
        .expect("the fake's foreground failure") = Some(actingcommand_device::DeviceError::fatal(
        "neutral foreground query failure",
    ));
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id, Arc::clone(&state))
                .with_resolved_for(ALIAS, resolved_handset(ALIAS, instance_id, bound_ios())),
        ),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let connection = ConnectionId::new(4791).expect("connection");
    let process = |operation| {
        host.process_request_for_test(&runtime_request(&ids, operation), connection)
            .expect("the request's receipt")
    };

    // The inputs, on a lease of the person's.
    let lease = process(RuntimeOperation::acquire_lease(
        ALIAS,
        ids.mint_holder_id().expect("holder"),
    ));
    let Some(RuntimeResult::LeaseGranted { token }) = lease.result() else {
        panic!("a lease on the handset: {lease:?}");
    };
    let token = token.clone();
    for action in [
        InputAction::Tap { x: 10, y: 20 },
        InputAction::Key {
            key: "back".to_owned(),
        },
        InputAction::Key {
            key: "KEYCODE_HOME".to_owned(),
        },
        InputAction::Swipe {
            x1: 10,
            y1: 20,
            x2: 30,
            y2: 40,
            duration_ms: 300,
        },
    ] {
        let receipt = process(RuntimeOperation::Input {
            token: token.clone(),
            action,
            frame: None,
        });
        refused_by_device_system(
            &receipt,
            DeviceSelfCheckFailure::InputBackendOpenFailed
                .code()
                .as_str(),
        );
    }
    let released = process(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        released.state(),
        RuntimeReceiptState::Completed,
        "{released:?}"
    );

    // A capture, as the Lab's look asks for one.
    let observed = process(RuntimeOperation::ObserveReadonly {
        instance_alias: ALIAS.to_owned(),
    });
    refused_by_device_system(
        &observed,
        DeviceSelfCheckFailure::CaptureBackendOpenFailed
            .code()
            .as_str(),
    );

    // An application launch, on the request's own claim.
    let launched = process(RuntimeOperation::application_lifecycle(
        ALIAS,
        ids.mint_holder_id().expect("holder"),
        ApplicationLifecycleAction::Launch,
    ));
    refused_by_device_system(
        &launched,
        SharedCode::ApplicationBackendOperationFailed
            .code()
            .as_str(),
    );

    assert_eq!(asked(&state), [0; 5], "nothing reached the stand-in");
    assert!(
        state
            .foreground_error_once
            .lock()
            .expect("the fake's foreground failure")
            .is_some(),
        "the foreground gate never asked"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// The configuration manifest of `instances` over the defaults file, each instance naming the idle
/// action `close_app`.
fn close_app_manifest(instances: &[InstanceId]) -> RuntimeConfigManifest {
    let ids = instances
        .iter()
        .map(
            |instance| match serde_json::to_value(instance).expect("an instance id encodes") {
                serde_json::Value::String(text) => text,
                other => panic!("an instance id is a string: {other}"),
            },
        )
        .collect::<Vec<_>>();
    let file = serde_json::json!({
        "instances": ids
            .iter()
            .map(|id| serde_json::json!({"instance_id": id, "idle": {"action": "close_app"}}))
            .collect::<Vec<_>>()
    });
    let (_, defaults) =
        actingcommand_runtime_config::DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = actingcommand_runtime_config::render_parameters(
        &actingcommand_runtime_config::mechanism_rows(),
        &ids,
        &defaults,
        &file,
        &|_| None,
    )
    .expect("the sections resolve");
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// Whether the start's preparation of `instance` finished without a failure.
fn prepared_at_start(events: &[PersistedEvent], instance: InstanceId) -> bool {
    phases(events).into_iter().any(|(event, phase)| {
        event.links().instance_id() == Some(&instance)
            && matches!(
                phase,
                RuntimeLifecyclePhase::InstancePreparationFinished {
                    stage: RecoveryTriggerStage::StartupPreparation,
                    outcome: None,
                    ..
                }
            )
    })
}

/// Fix round 1, item 2: a start with an iOS handset bound and one unbound beside an emulator, each
/// of the three naming the idle action `close_app`. Neither handset is attempted: nothing is
/// opened on the fakes behind them (which would pass), no preparation, failure, ladder or idle
/// record names them, and the idle check that notes the emulator's missing way back meets neither
/// handset. The emulator's start preparation is what it is in a start without them: finished
/// without a failure.
#[test]
fn f2_fix1_a_start_attempts_no_ios_handset_and_the_emulator_beside_them_starts_as_alone() {
    const IPHONE: &str = "neutral.iphone";
    const IPHONE_UNBOUND: &str = "neutral.iphone-unbound";
    const EMULATOR: &str = "neutral.emulator";
    let opening = || {
        let state = Arc::new(FakeState::default());
        state.observed_open.store(true, Ordering::Release);
        state
    };

    // The emulator alone.
    let alone = {
        let root = TempDir::new().expect("tempdir");
        let emulator = instance_id();
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(EMULATOR, emulator, opening()).with_device_endpoints()),
        )
        .expect("runtime host");
        let events = all_events(&host);
        host.close().expect("close host");
        prepared_at_start(&events, emulator)
    };
    assert!(alone, "the emulator alone prepares at its start");

    // The emulator beside an iOS handset bound and one unbound.
    let root = TempDir::new().expect("tempdir");
    let (bound, unbound, emulator) = (instance_id(), instance_id(), instance_id());
    let (bound_state, unbound_state) = (opening(), opening());
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(close_app_manifest(&[bound, unbound, emulator])),
        Arc::new(
            FakeProvider::from_entries([
                (IPHONE.to_owned(), bound, Arc::clone(&bound_state)),
                (
                    IPHONE_UNBOUND.to_owned(),
                    unbound,
                    Arc::clone(&unbound_state),
                ),
                (EMULATOR.to_owned(), emulator, opening()),
            ])
            .with_device_endpoints()
            .with_resolved_for(IPHONE, resolved_handset(IPHONE, bound, bound_ios()))
            .with_resolved_for(
                IPHONE_UNBOUND,
                resolved_handset(IPHONE_UNBOUND, unbound, HandsetEndpoint::Ios(None)),
            ),
        ),
    )
    .expect("the start goes on beside iOS handsets");
    // The idle check, as at a policy cycle, with no plan: an instance whose action cannot bring
    // it back says so once (none has a startup package).
    host.check_idle_instances_for_test(&BTreeMap::new())
        .expect("the idle check");
    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = all_events(&host);
    host.close().expect("close host");

    assert_eq!(
        prepared_at_start(&events, emulator),
        alone,
        "the emulator starts as it does alone"
    );
    let idle_noted = |instance: InstanceId| {
        phases(&events).into_iter().any(|(event, phase)| {
            event.links().instance_id() == Some(&instance)
                && matches!(
                    &phase,
                    RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                        if outcome.code().as_str() == IdleCode::Unsupported.as_str()
                )
        })
    };
    assert!(idle_noted(emulator), "the idle check met the emulator");
    for (handset, state) in [(bound, &bound_state), (unbound, &unbound_state)] {
        assert_eq!(asked(state), [0; 5], "nothing is opened on {handset:?}");
        assert!(
            events
                .iter()
                .filter(|event| event.links().instance_id() == Some(&handset))
                .all(|event| !matches!(
                    event.payload(),
                    EventPayload::Runtime(RuntimePayload::Failed(_))
                )),
            "no failure record names {handset:?}"
        );
        let attempted = phases(&events)
            .into_iter()
            .filter(|(event, _)| event.links().instance_id() == Some(&handset))
            .map(|(_, phase)| phase)
            .filter(|phase| {
                matches!(
                    phase,
                    RuntimeLifecyclePhase::InstancePreparationFinished { .. }
                        | RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                        | RuntimeLifecyclePhase::DeviceStateObserved { .. }
                )
            })
            .collect::<Vec<_>>();
        assert!(attempted.is_empty(), "{handset:?}: {attempted:?}");
        assert!(!idle_noted(handset));
    }
}
