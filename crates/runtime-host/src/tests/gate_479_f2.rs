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
//
// Fix round 2 (rd-1119/brief-479-f2-fix2.md, items 2 and 3): that start also writes the one
// statement, each handset's availability withdrawn under `device_system:ios`, and the status says
// it in each handset's capability table; the ledger's rule is the other withdrawals' (on change), so
// a restart over the same ledger writes none, keeps that record in effect and the status still says
// it. A person's self-check and ladder drill (plan and device) on an iOS handset are each refused
// at their entry with their own failure code and that cause, before anything of the stand-in is
// asked. The coordinator's follow-up: its pause, a person's and a tool's hold, is quiet at once,
// and its resume lifts the pause with no reconnect (`scheduling_resume_without_backend`), nothing
// failed; a tool's hold over it and an emulator is quiet and both resume.
//
// Fix round 4 (rd-1119/brief-479-f2-fix4.md, item 5): a person's pause and resume of an iOS handset
// that is not bound answer as a bound one's do.

use super::gate_369_s6c::{instance_scope, instance_status, pause, resume, send};
use super::gate_398_rd1b::{last_availability, resolved_handset};
use super::gate_398_rd2::{fact, instance_facts};
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::{HostCode, HostLocation, IdleCode};
use actingcommand_contract::codes::{DeviceSelfCheckFailure, SharedCode};
use actingcommand_contract::{
    DeviceBoundVia, EmulatorCapability, EmulatorCapabilityAvailability,
    EmulatorCapabilityImplementation, FactPayload, FencedWrite, FencedWritePurpose,
    InstanceDeviceKind, InstancePauseStage, RecoveryDrillMode, RecoveryTriggerStage,
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

/// Ruling 6: on an iOS handset, bound or not, an input and an application request are each refused
/// by the device registry before anything is opened (the registry's adb is a name that never runs,
/// and no backend is opened), with the one cause `device_system_backend_missing` beside the
/// operation's own code. Workflow #479 F-3a (adapted, the requirement changing what it pins): an
/// iOS handset's capture has its backend, `acib` (`gate_479_f3a`), so the capture left this test.
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
        for kernel in [on_input, on_application] {
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

    // Workflow #479 F-3a (adapted, the requirement changing what it pins): a read-only capture of
    // an iOS handset has its backend, `acib`, and is no longer refused here (`gate_479_f3a`).

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

/// Every `session.instance.available` publication of `instance_alias` in `events`, in ledger
/// order: its value (`None` when not a boolean) and its source snapshot.
fn availabilities(events: &[PersistedEvent], instance_alias: &str) -> Vec<(Option<bool>, String)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => Some(payload.record()),
            _ => None,
        })
        .filter(|record| {
            record.key == crate::fact_store::POLICY_INSTANCE_AVAILABLE_KEY
                && record.scope
                    == FactScope::Instance {
                        instance_id: instance_alias.to_owned(),
                    }
        })
        .map(|record| {
            let value = match &record.content {
                FactContent::Inline {
                    value: ContractFactValue::Boolean(value),
                } => Some(*value),
                _ => None,
            };
            (value, record.source_snapshot_id.clone())
        })
        .collect()
}

/// The active `session.instance.available` record of the policy instance `instance_alias` of the
/// server `server_id` (the fixture's game), as the fact store holds it: its source snapshot and
/// when it was observed.
fn active_availability(host: &RuntimeHost, instance_alias: &str, server_id: &str) -> (String, u64) {
    let snapshot = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: instance_alias.to_owned(),
            server_id: server_id.to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("the instance's facts");
    let records = snapshot
        .records
        .iter()
        .filter(|record| record.key == crate::fact_store::POLICY_INSTANCE_AVAILABLE_KEY)
        .collect::<Vec<_>>();
    let [record] = records.as_slice() else {
        panic!("one availability record of {instance_alias}: {records:#?}");
    };
    (
        record.source_snapshot_id.clone(),
        record.observed_at_unix_ms,
    )
}

/// Fix round 2, item 2: `status` says what the ledger's statement says, at its read: every row of
/// the handset's capability table is unsupported and unavailable, from `device_system:ios`.
/// Workflow #479 F-3a (adapted, the requirement changing what it pins): but its frame row, which
/// `acib_screen_capture` implements, not verified.
fn status_says_no_backend(status: &actingcommand_contract::RuntimeInstanceStatus) {
    let alias = status.instance_alias();
    let table = status
        .capabilities()
        .unwrap_or_else(|| panic!("{alias}: a capability table in the status"));
    for capability in EmulatorCapability::ALL {
        let row = table.evidence(capability);
        let expected = if capability == EmulatorCapability::CaptureFrame {
            (
                "acib_screen_capture",
                EmulatorCapabilityAvailability::Unverified,
                Some(EmulatorCapabilityImplementation::Supported),
            )
        } else {
            (
                "device_system:ios",
                EmulatorCapabilityAvailability::Unavailable,
                Some(EmulatorCapabilityImplementation::Unsupported),
            )
        };
        assert_eq!(
            (row.evidence_ref(), row.availability(), row.implementation()),
            expected,
            "{alias}: {capability:?}"
        );
    }
}

/// Fix round 1, item 2: a start with an iOS handset bound and one unbound beside an emulator, each
/// of the three naming the idle action `close_app`. Neither handset is attempted: nothing is
/// opened on the fakes behind them (which would pass), no preparation, failure, ladder or idle
/// record names them, and the idle check that notes the emulator's missing way back meets neither
/// handset. The emulator's start preparation is what it is in a start without them: finished
/// without a failure.
///
/// Fix round 2, item 2: the handsets are the configuration's two policy instances, so each has an
/// availability. The first start writes the one statement for each, its availability withdrawn
/// under `device_system:ios` (information: no failure record, as above), and the status says it in
/// each one's capability table. The ledger's rule is the one of the other withdrawals (the
/// unbound handset's `device_link:unbound`, Workflow #398 RD-1b): written on change. A restart over
/// the same ledger writes none: the same record stays in effect, and the status still says it.
#[test]
fn f2_fix1_a_start_attempts_no_ios_handset_and_the_emulator_beside_them_starts_as_alone() {
    const IPHONE: &str = POLICY_INSTANCE_ALIAS;
    const IPHONE_UNBOUND: &str = POLICY_INSTANCE_ALIAS_B;
    const EMULATOR: &str = "neutral.emulator";
    // The fixture's servers of the two policy instances.
    const SERVERS: [(&str, &str); 2] = [
        (IPHONE, "fixture-server-a"),
        (IPHONE_UNBOUND, "fixture-server-b"),
    ];
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

    // The emulator beside an iOS handset bound and one unbound, started twice on one state root.
    let root = TempDir::new().expect("tempdir");
    let (bound, unbound, emulator) = (instance_id(), instance_id(), instance_id());
    let (bound_state, unbound_state) = (opening(), opening());
    let start = || {
        let host = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(
                    pending_policy_facts(),
                    pending_policy_resources(),
                ))
                .with_config_manifest(close_app_manifest(&[bound, unbound, emulator])),
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
        // The idle check, as at a policy cycle, with no plan: an instance whose action cannot
        // bring it back says so (none has a startup package).
        host.check_idle_instances_for_test(&BTreeMap::new())
            .expect("the idle check");
        assert!(host.fatal_error().expect("runtime health").is_none());
        host
    };

    // The first start: the statement in the ledger, in effect, and in the status.
    let host = start();
    let first = all_events(&host);
    let in_effect = SERVERS.map(|(alias, server)| active_availability(&host, alias, server));
    for alias in [IPHONE, IPHONE_UNBOUND] {
        assert_eq!(
            availabilities(&first, alias),
            [(Some(false), "device_system:ios".to_owned())],
            "{alias}: the one statement of the start"
        );
        status_says_no_backend(&instance_status(&host, alias));
    }
    for (snapshot, _) in &in_effect {
        assert_eq!(
            snapshot.as_str(),
            "device_system:ios",
            "the statement is in effect"
        );
    }
    host.close().expect("close host");

    // The restart: it finds the statement in effect and writes none; the same record stays in
    // effect and the status still says it.
    let host = start();
    assert_eq!(
        SERVERS.map(|(alias, server)| active_availability(&host, alias, server)),
        in_effect,
        "the same records in effect after the restart"
    );
    for alias in [IPHONE, IPHONE_UNBOUND] {
        status_says_no_backend(&instance_status(&host, alias));
    }
    let events = all_events(&host);
    host.close().expect("close the restarted host");
    for alias in [IPHONE, IPHONE_UNBOUND] {
        assert_eq!(
            availabilities(&events, alias),
            [(Some(false), "device_system:ios".to_owned())],
            "{alias}: the restart wrote no statement again"
        );
    }

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

/// The fake behind a bound iOS handset, whose one foreground failure is still there for as long as
/// nothing asks the foreground of it.
fn ios_fake() -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    *state
        .foreground_error_once
        .lock()
        .expect("the fake's foreground failure") = Some(actingcommand_device::DeviceError::fatal(
        "neutral foreground query failure",
    ));
    state
}

/// Nothing of the stand-in behind the handset `instance` on `state` was consulted (nothing
/// opened, sent, captured or commanded; the foreground never asked), and in `events` no failure,
/// preparation or ladder record names it.
#[track_caller]
fn stand_in_untouched(state: &FakeState, events: &[PersistedEvent], instance: InstanceId) {
    assert_eq!(asked(state), [0; 5], "nothing reached the stand-in");
    assert!(
        state
            .foreground_error_once
            .lock()
            .expect("the fake's foreground failure")
            .is_some(),
        "the foreground query never asked"
    );
    assert!(
        events
            .iter()
            .filter(|event| event.links().instance_id() == Some(&instance))
            .all(|event| !matches!(
                event.payload(),
                EventPayload::Runtime(RuntimePayload::Failed(_))
            )),
        "no failure record names the handset"
    );
    let attempted = phases(events)
        .into_iter()
        .filter(|(event, _)| event.links().instance_id() == Some(&instance))
        .map(|(_, phase)| phase)
        .filter(|phase| {
            matches!(
                phase,
                RuntimeLifecyclePhase::InstancePreparationFinished { .. }
                    | RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                    | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
            )
        })
        .collect::<Vec<_>>();
    assert!(attempted.is_empty(), "{attempted:?}");
}

/// The stage of the instance pause of `alias`, if one holds.
fn pause_stage(host: &RuntimeHost, alias: &str) -> Option<InstancePauseStage> {
    host.scheduling_pauses_for_test()
        .expect("the pauses")
        .1
        .get(alias)
        .map(|pause| pause.stage)
}

/// The resume of an iOS handset, as the host answers it: Completed, a self-check whose two sides
/// are not ok with no failure code (no reconnect ran, so none is reported), and
/// `scheduling_resume_without_backend` the one cause of the receipt's success code.
#[track_caller]
fn resumed_without_backend(resumed: &RuntimeReceipt) {
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    let Some(RuntimeResult::SchedulingResumed {
        selfcheck: Some(selfcheck),
        ..
    }) = resumed.result()
    else {
        panic!("a resume with its self-check: {resumed:?}");
    };
    assert!(!selfcheck.capture.ok && !selfcheck.touch.ok, "{resumed:?}");
    assert_eq!(selfcheck.failure_code, None, "{resumed:?}");
    let [cause] = resumed.outcome().causes() else {
        panic!("one cause of the resume: {resumed:?}");
    };
    assert!(
        cause.outcome().is(HostCode::SchedulingResumeWithoutBackend),
        "{resumed:?}"
    );
}

/// The `device_state_observed` records of `instance` in `events` whose outcome is `code`.
fn moments(events: &[PersistedEvent], instance: InstanceId, code: &str) -> usize {
    phases(events)
        .into_iter()
        .filter(|(event, phase)| {
            event.links().instance_id() == Some(&instance)
                && matches!(
                    phase,
                    RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                        if outcome.code().as_str() == code
                )
        })
        .count()
}

/// Fix round 2, item 3: the paths a person starts that would otherwise reach a bound iOS handset's
/// stand-in registration are each refused at their entry with their own failure code caused by
/// `device_system_backend_missing`: a self-check `input_backend_open_failed` (the code its
/// preparation ends with at its first open), before its claim; a drill of the ladder, plan or
/// device, `recovery_ladder_drill_unsupported`, before any ladder is admitted. Nothing reaches the
/// stand-in: nothing is opened, the foreground query never asks (the fake's one foreground failure
/// is still there), and no preparation or ladder record names the handset.
#[test]
fn f2_fix2_a_self_check_and_a_drill_are_refused_at_their_entry() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let instance_id = instance_id();
    let state = ios_fake();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id, Arc::clone(&state))
                .with_resolved_for(ALIAS, resolved_handset(ALIAS, instance_id, bound_ios())),
        ),
    )
    .expect("runtime host");

    // A self-check.
    let checked = send(
        &host,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: ALIAS.to_owned(),
        },
        4792,
    );
    refused_by_device_system(
        &checked,
        DeviceSelfCheckFailure::InputBackendOpenFailed
            .code()
            .as_str(),
    );

    // A drill of the ladder, in either mode.
    for mode in [RecoveryDrillMode::Plan, RecoveryDrillMode::Device] {
        let drilled = send(
            &host,
            RuntimeOperation::DrillRecoveryLadder {
                instance_alias: ALIAS.to_owned(),
                mode,
            },
            4793,
        );
        refused_by_device_system(&drilled, HostCode::RecoveryLadderDrillUnsupported.as_str());
    }

    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = all_events(&host);
    host.close().expect("close host");
    stand_in_untouched(&state, &events, instance_id);
}

/// A person's `stand` pause of an iOS handset registered as `handset` (bound or not), on a host of
/// its own, is quiet at once (Completed with no standing cause, stage `released`), and its resume
/// succeeds: the pause is lifted and no reconnect is attempted (no claim: no lease is granted on the
/// handset; no preparation, no open), the answer's self-check reports none, the receipt's cause and
/// one record of the ledger say `scheduling_resume_without_backend`, and no failure is answered or
/// recorded.
fn stand_pause_and_resume_reconnect_nothing(handset: HandsetEndpoint) {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let instance_id = instance_id();
    let state = ios_fake();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id, Arc::clone(&state))
                .with_resolved_for(ALIAS, resolved_handset(ALIAS, instance_id, handset)),
        ),
    )
    .expect("runtime host");

    let paused = pause(&host, instance_scope(ALIAS), 4794);
    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
    assert!(paused.outcome().causes().is_empty(), "quiet: {paused:?}");
    assert_eq!(
        pause_stage(&host, ALIAS),
        Some(InstancePauseStage::Released)
    );
    let resumed = resume(&host, instance_scope(ALIAS), 4795);
    resumed_without_backend(&resumed);
    assert_eq!(pause_stage(&host, ALIAS), None, "the pause is lifted");

    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = all_events(&host);
    host.close().expect("close host");
    stand_in_untouched(&state, &events, instance_id);
    assert_eq!(
        moments(
            &events,
            instance_id,
            HostCode::SchedulingResumeWithoutBackend.as_str()
        ),
        1,
        "the ledger says the resume reconnected nothing"
    );
    assert!(
        !events.iter().any(|event| {
            event.event_type() == EventType::LeaseGranted
                && event.links().instance_id() == Some(&instance_id)
        }),
        "no claim was granted on the handset"
    );
}

/// Fix round 2, the coordinator's follow-up: `stand_pause_and_resume_reconnect_nothing` of a bound
/// iOS handset.
#[test]
fn f2_fix2_a_stand_pause_of_an_ios_handset_is_quiet_at_once_and_its_resume_reconnects_nothing() {
    stand_pause_and_resume_reconnect_nothing(bound_ios());
}

/// Fix round 4 (the third review: the pause and the resume were tested on a bound handset only):
/// an iOS handset that is not bound answers its pause and its resume as a bound one does,
/// `stand_pause_and_resume_reconnect_nothing` of `HandsetEndpoint::Ios(None)`.
#[test]
fn f2_fix4_an_unbound_ios_handsets_pause_and_resume_answer_as_a_bound_ones() {
    stand_pause_and_resume_reconnect_nothing(HandsetEndpoint::Ios(None));
}

/// Fix round 2, the coordinator's follow-up: a tool's hold (`quiet_or_lift`, one pause per instance
/// as the rescue module sends them) over an emulator and a bound iOS handset, every pause's grace
/// already past, while a client holds a lease on the handset (which, on any other instance, the
/// hold would meet as a lease still in the way and be lifted for): both pauses are quiet at once
/// (Completed, no cause, stage `released`), then both are resumed: the emulator's resume reconnects
/// it, the handset's lifts its pause with no reconnect (`scheduling_resume_without_backend`). No
/// failure record names the handset and nothing of its stand-in is consulted.
#[test]
fn f2_fix2_a_tools_hold_over_an_emulator_and_an_ios_handset_is_quiet_and_both_resume() {
    const IPHONE: &str = "neutral.iphone";
    const EMULATOR: &str = "neutral.emulator";
    let root = TempDir::new().expect("tempdir");
    let (handset, emulator) = (instance_id(), instance_id());
    let state = ios_fake();
    let emulator_state = Arc::new(FakeState::default());
    emulator_state.observed_open.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root).with_scheduling_pause_deadlines_for_test(Duration::ZERO),
        Arc::new(
            FakeProvider::from_entries([
                (IPHONE.to_owned(), handset, Arc::clone(&state)),
                (EMULATOR.to_owned(), emulator, emulator_state),
            ])
            .with_resolved_for(IPHONE, resolved_handset(IPHONE, handset, bound_ios())),
        ),
    )
    .expect("runtime host");
    let holder = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_holder_id()
        .expect("holder id");
    let leased = send(&host, RuntimeOperation::acquire_lease(IPHONE, holder), 4796);
    let Some(RuntimeResult::LeaseGranted { token }) = leased.result() else {
        panic!("a client's lease on the handset: {leased:?}");
    };
    let token = token.clone();

    let hold = |alias: &str, connection: u64| {
        send(
            &host,
            RuntimeOperation::PauseScheduling {
                scope: instance_scope(alias),
                pause_reason: actingcommand_contract::SchedulingPauseReason::new("f2_hold")
                    .expect("pause reason"),
                drain_timeout_ms: 1_000,
                pause_mode: actingcommand_contract::SchedulingPauseMode::QuietOrLift,
            },
            connection,
        )
    };
    for (alias, connection) in [(IPHONE, 4797), (EMULATOR, 4798)] {
        let held = hold(alias, connection);
        assert_eq!(
            held.state(),
            RuntimeReceiptState::Completed,
            "{alias}: {held:?}"
        );
        assert!(held.outcome().causes().is_empty(), "{alias}: {held:?}");
        assert_eq!(
            pause_stage(&host, alias),
            Some(InstancePauseStage::Released),
            "{alias}: quiet"
        );
    }

    let resumed = resume(&host, instance_scope(EMULATOR), 4799);
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    resumed_without_backend(&resume(&host, instance_scope(IPHONE), 4800));
    for alias in [IPHONE, EMULATOR] {
        assert_eq!(pause_stage(&host, alias), None, "{alias}: resumed");
    }
    let released = send(&host, RuntimeOperation::ReleaseLease { token }, 4796);
    assert_eq!(
        released.state(),
        RuntimeReceiptState::Completed,
        "{released:?}"
    );

    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = all_events(&host);
    host.close().expect("close host");
    stand_in_untouched(&state, &events, handset);
}
