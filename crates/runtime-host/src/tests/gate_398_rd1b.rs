// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-1b (model-398 v1.1 §12, rows T2, T4 and T8): a handset instance is bound to
// its device by the private binding file. Its `runtime.instance_bound` records the device by
// fingerprint only and no ADB host or port; a handset the file binds no device to is
// unavailable under `device_link:unbound` while the other instances run; emulator control
// refuses a handset. The registrations are the registry's own; the serials are neutral fakes,
// distinct from the other test modules' so the process-wide label of each stays its own.

use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{
    DeviceBoundVia, EmulatorInstanceAction, FactPayload, InstanceDeviceKind, RuntimePayload,
    SecretField, SecretFingerprinter,
};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceSerial, DeviceTarget,
    MaaTouchConfig, TouchBackendChoice, TouchBackendConfig,
};
use actingcommand_ledger::Sha256SecretFingerprinter;

/// The salt of `config` (tests/support/startup.rs).
const SALT: &[u8] = b"runtime-host-test-salt";
const BOUND_AT_UNIX_MS: u64 = 1_700_000_000_000;

/// The registration of the handset `alias`: its targets addressed by the bound serial, or by the
/// unbound stand-in, and never `adb connect`ed, as actingd assembles a handset.
fn handset_registration(
    alias: &str,
    instance_id: InstanceId,
    handset: HandsetEndpoint,
) -> ExecutionBackendRegistration {
    let serial = handset.binding().map_or_else(
        || DeviceSerial::unbound_handset(alias),
        |binding| binding.serial().clone(),
    );
    let target = DeviceTarget {
        serial: Some(serial),
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
    .expect("handset registration")
    .with_handset(handset)
}

pub(super) fn bound(alias: &str, serial: &str) -> HandsetEndpoint {
    HandsetEndpoint::Bound(HandsetBinding::new(
        DeviceSerial::handset(serial, alias),
        BOUND_AT_UNIX_MS,
        DeviceBoundVia::Cli,
    ))
}

/// The registry's resolution of the handset `alias`.
pub(super) fn resolved_handset(
    alias: &str,
    instance_id: InstanceId,
    handset: HandsetEndpoint,
) -> ResolvedExecutionInstance {
    ExecutionBackendRegistry::new([handset_registration(alias, instance_id, handset)])
        .expect("handset registry")
        .resolve(alias)
        .expect("registered handset")
}

fn all_events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

/// T2 (§3.4): the handset's `runtime.instance_bound` names no ADB host or port and records its
/// device: kind `handset`, the fingerprint of its serial under the ledger's salt (field
/// `device_serial`), when it was bound and by which channel.
#[test]
fn rd1b_t2_a_handset_binding_records_its_device_by_fingerprint_without_host_or_port() {
    const SERIAL: &str = "TESTSERIAL0013";
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance_id = instance_id();
    let resolved = resolved_handset(
        POLICY_INSTANCE_ALIAS,
        instance_id,
        bound(POLICY_INSTANCE_ALIAS, SERIAL),
    );
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id, state)
                .with_resolved_for(POLICY_INSTANCE_ALIAS, resolved),
        ),
    )
    .expect("runtime host");
    let events = all_events(&host);
    host.close().expect("close host");

    let bindings = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::InstanceBound(binding)) => Some(binding),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [binding] = bindings.as_slice() else {
        panic!("one instance binding: {bindings:?}");
    };
    assert_eq!(binding.instance_alias(), POLICY_INSTANCE_ALIAS);
    assert_eq!((binding.adb_host(), binding.adb_port()), (None, None));
    assert!(binding.serial_configured());
    let device = binding.device().expect("the handset's device");
    let expected = Sha256SecretFingerprinter::new(SALT)
        .expect("fingerprinter")
        .fingerprint(SecretField::DeviceSerial, SERIAL)
        .expect("fingerprint");
    assert_eq!(device.kind(), InstanceDeviceKind::Handset);
    assert_eq!(device.fingerprint(), Some(&expected));
    assert_eq!(device.bound_at_unix_ms(), Some(BOUND_AT_UNIX_MS));
    assert_eq!(device.bound_via(), Some(DeviceBoundVia::Cli));
}

/// The last `session.instance.available` publication of `instance_alias`: its value and its
/// source snapshot.
pub(super) fn last_availability(events: &[PersistedEvent], instance_alias: &str) -> (bool, String) {
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
        .filter_map(|record| match &record.content {
            FactContent::Inline {
                value: ContractFactValue::Boolean(value),
            } => Some((*value, record.source_snapshot_id.clone())),
            _ => None,
        })
        .next_back()
        .unwrap_or_else(|| panic!("an availability of {instance_alias}"))
}

/// T4, the start (§3.6): a handset the private binding file binds no device to is registered and
/// unavailable under `device_link:unbound`, whatever its self-check says; the other instance
/// (an emulator) is available, and the Runtime runs.
#[test]
fn rd1b_t4_an_unbound_handset_is_unavailable_and_the_other_instance_runs() {
    let root = TempDir::new().expect("tempdir");
    let handset = instance_id();
    let emulator_state = Arc::new(FakeState::default());
    emulator_state.observed_open.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root).with_policy_inputs(PolicyInputSnapshot::new(
            pending_policy_facts(),
            pending_policy_resources(),
        )),
        Arc::new(
            FakeProvider::from_entries([
                (
                    POLICY_INSTANCE_ALIAS.to_owned(),
                    handset,
                    Arc::new(FakeState::default()),
                ),
                (
                    POLICY_INSTANCE_ALIAS_B.to_owned(),
                    instance_id(),
                    emulator_state,
                ),
            ])
            .with_device_endpoints()
            .with_resolved_for(
                POLICY_INSTANCE_ALIAS,
                resolved_handset(POLICY_INSTANCE_ALIAS, handset, HandsetEndpoint::Unbound),
            ),
        ),
    )
    .expect("the start goes on with an unbound handset");
    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = all_events(&host);
    host.close().expect("close host");

    assert_eq!(
        last_availability(&events, POLICY_INSTANCE_ALIAS),
        (false, "device_link:unbound".to_owned())
    );
    assert!(last_availability(&events, POLICY_INSTANCE_ALIAS_B).0);
    let unbound = events
        .iter()
        .find_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::InstanceBound(binding))
                if binding.instance_alias() == POLICY_INSTANCE_ALIAS =>
            {
                binding.device()
            }
            _ => None,
        })
        .expect("the handset's binding");
    assert_eq!(unbound.kind(), InstanceDeviceKind::Handset);
    assert_eq!(unbound.fingerprint(), None);
}

/// T8 (§3.1): emulator control refuses a handset as it refuses every instance bound without
/// discovery: `emulator_control_unavailable`, denied.
#[test]
fn rd1b_t8_emulator_control_refuses_a_handset() {
    const SERIAL: &str = "TESTSERIAL0014";
    let root = TempDir::new().expect("tempdir");
    let registry = ExecutionBackendRegistry::new([handset_registration(
        POLICY_INSTANCE_ALIAS,
        instance_id(),
        bound(POLICY_INSTANCE_ALIAS, SERIAL),
    )])
    .expect("handset registry");
    // The registry's adb is a neutral name that never runs, so the start's preparation fails;
    // no ladder climbs meanwhile.
    let no_ladder = BTreeMap::from([(
        POLICY_INSTANCE_ALIAS.to_owned(),
        actingcommand_contract::InstanceStuckRecovery {
            enabled: false,
            cooldown_secs: 600,
        },
    )]);
    let host = RuntimeHost::start(
        config(&root).with_stuck_recovery(no_ladder),
        Arc::new(registry),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for action in [EmulatorInstanceAction::Start, EmulatorInstanceAction::Stop] {
        let receipt = host
            .process_request_for_test(
                &runtime_request(
                    &ids,
                    RuntimeOperation::ControlEmulatorInstance {
                        instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
                        action,
                    },
                ),
                ConnectionId::new(398).expect("connection"),
            )
            .expect("receipt");
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{action:?}");
        assert_eq!(
            receipt.outcome().code().as_str(),
            HostCode::EmulatorControlUnavailable.as_str(),
            "{action:?}"
        );
    }
    host.close().expect("close host");
}
