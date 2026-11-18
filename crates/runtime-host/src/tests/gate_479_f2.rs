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

use super::gate_398_rd1b::{last_availability, resolved_handset};
use super::gate_398_rd2::{fact, instance_facts};
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::{
    DeviceBoundVia, FencedWrite, FencedWritePurpose, InstanceDeviceKind,
    RuntimeInstanceBindingPayload, RuntimePayload, Sha256Fingerprint, issue_fenced_write,
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
