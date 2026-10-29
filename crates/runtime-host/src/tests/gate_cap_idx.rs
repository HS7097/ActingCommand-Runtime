// SPDX-License-Identifier: AGPL-3.0-only

// CAP-IDX (0.12 wave 2b, since 0.12.27): a discovery-bound entry's capture takes its MuMu index
// from the binding, never from the ADB port. MuMu takes the next free port when an index's base
// port is busy at launch; the registry then opens capture with the bound index, after one bounded
// `info -v <index>` read whose reported port must be the bound one (#419: never drive the wrong
// instance). The read is a fake; no device, emulator or MuMuManager is touched. Indices and ports
// are symbolic: the ports are derived from the indices by the device crate's base-port rule.

use super::*;
use actingcommand_contract::{BackendObservationStatus, BackendOpenEntry};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceTarget, MaaTouchConfig,
    NemuResolutionReason, TouchBackendChoice, TouchBackendConfig, mumu_instance_port,
};
use std::sync::Mutex;

const ALIAS_SHIFTED: &str = "cap-idx-shifted";
const ALIAS_OTHER: &str = "cap-idx-other";
const INDEX_SHIFTED: u16 = 3;
const INDEX_OTHER: u16 = 4;

pub(super) fn base_port(index: u16) -> u16 {
    mumu_instance_port(index).expect("base port")
}

/// A Nemu IPC capture registration bound through discovery to `index` on its base port, as
/// actingd assembles a discovered instance.
pub(super) fn discovered_registration(alias: &str, index: u16) -> ExecutionBackendRegistration {
    let target = DeviceTarget {
        port: base_port(index),
        ..DeviceTarget::default()
    };
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    ExecutionBackendRegistration::new(
        alias,
        instance_id(),
        "neutral.application",
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::AdbShellInput),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::NemuIpc),
    )
    .expect("discovered registration")
    .with_discovered_binding(DiscoveredInstanceBinding::new(
        index,
        format!("neutral-instance-{index}"),
        "neutral-provider-version",
        "neutral-mumu-manager",
    ))
}

/// A fake `info -v <index>` read: a running instance reporting `reported[index]` as its ADB
/// port; every index it is asked for is recorded in `reads`.
pub(super) fn fake_reader(
    reported: BTreeMap<u16, u16>,
    reads: Arc<Mutex<Vec<u16>>>,
) -> impl Fn(&Path, u16) -> DeviceResult<InstanceState> + Send + Sync + 'static {
    move |_, index| {
        reads.lock().expect("reads").push(index);
        Ok(InstanceState {
            process_started: true,
            android_started: true,
            adb_port: reported.get(&index).copied(),
            player_state: None,
            launch_err_code: 0,
            launch_err_msg: String::new(),
        })
    }
}

fn bound_port(registry: &ExecutionBackendRegistry, alias: &str) -> Option<u16> {
    registry
        .resolve(alias)
        .expect("registered instance")
        .adb_endpoint()
        .and_then(ResolvedInstanceEndpoint::bound)
        .map(ResolvedAdbEndpoint::port)
}

#[test]
fn cap_idx_a_discovered_entry_rebound_to_a_shifted_port_opens_capture_with_the_bound_index() {
    let shifted = base_port(INDEX_SHIFTED) + 1;
    let reads = Arc::new(Mutex::new(Vec::new()));
    let registry =
        ExecutionBackendRegistry::new([discovered_registration(ALIAS_SHIFTED, INDEX_SHIFTED)])
            .expect("registry")
            .with_instance_state_reader(fake_reader(
                BTreeMap::from([(INDEX_SHIFTED, shifted)]),
                Arc::clone(&reads),
            ));

    // On the base port the capture already carries the bound index, and nothing is read.
    let at_base = registry
        .capture_for_open(ALIAS_SHIFTED)
        .expect("base-port capture");
    assert_eq!(at_base.target.port, base_port(INDEX_SHIFTED));
    assert_eq!(at_base.nemu.instance_id, Some(i32::from(INDEX_SHIFTED)));
    assert!(reads.lock().expect("reads").is_empty());

    // MuMu relaunched the instance on the next port and reports it there; the entry is rebound.
    registry
        .rebind_discovered_endpoint(ALIAS_SHIFTED, Some(shifted))
        .expect("rebind to the shifted port");
    let capture = registry
        .capture_for_open(ALIAS_SHIFTED)
        .expect("shifted-port capture");

    assert_eq!(capture.target.port, shifted);
    assert_eq!(capture.nemu.instance_id, Some(i32::from(INDEX_SHIFTED)));
    assert_eq!(*reads.lock().expect("reads"), vec![INDEX_SHIFTED]);
}

#[test]
fn cap_idx_a_port_mumu_does_not_report_refuses_capture_and_spares_the_other_instance() {
    let shifted = base_port(INDEX_SHIFTED) + 1;
    let reads = Arc::new(Mutex::new(Vec::new()));
    // MuMu reports the index on its base port, not on the port the entry is bound to.
    let registry = ExecutionBackendRegistry::new([
        discovered_registration(ALIAS_SHIFTED, INDEX_SHIFTED),
        discovered_registration(ALIAS_OTHER, INDEX_OTHER),
    ])
    .expect("registry")
    .with_instance_state_reader(fake_reader(
        BTreeMap::from([
            (INDEX_SHIFTED, base_port(INDEX_SHIFTED)),
            (INDEX_OTHER, base_port(INDEX_OTHER)),
        ]),
        Arc::clone(&reads),
    ));
    let other_before = registry.resolve(ALIAS_OTHER).expect("other instance");
    let other_capture_before = registry
        .capture_for_open(ALIAS_OTHER)
        .expect("other capture");
    registry
        .rebind_discovered_endpoint(ALIAS_SHIFTED, Some(shifted))
        .expect("rebind to the shifted port");

    let Err(error) = registry.open_capture(ALIAS_SHIFTED, None) else {
        panic!("a port MuMu does not report for the index must not open capture");
    };

    assert_eq!(
        error
            .nemu_resolution_context()
            .map(|context| context.reason()),
        Some(NemuResolutionReason::TargetIdentityMismatch)
    );
    assert!(
        error
            .message()
            .contains(&format!("reports ADB port {}", base_port(INDEX_SHIFTED))),
        "{}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains(&format!("binding is on port {shifted}")),
        "{}",
        error.message()
    );
    let [observation] = error.backend_open_observations() else {
        panic!("one capture open observation");
    };
    assert_eq!(observation.report.entry, BackendOpenEntry::Capture);
    assert_eq!(observation.report.status, BackendObservationStatus::Failed);
    assert_eq!(observation.report.requested, "nemu_ipc");
    assert_eq!(*reads.lock().expect("reads"), vec![INDEX_SHIFTED]);

    // The refused instance keeps its binding; the other instance is unchanged and reads nothing.
    assert_eq!(bound_port(&registry, ALIAS_SHIFTED), Some(shifted));
    let other_after = registry.resolve(ALIAS_OTHER).expect("other instance");
    assert_eq!(other_after.adb_endpoint(), other_before.adb_endpoint());
    assert_eq!(other_after.audit_endpoint(), other_before.audit_endpoint());
    let other_capture_after = registry
        .capture_for_open(ALIAS_OTHER)
        .expect("other capture");
    assert_eq!(
        other_capture_after.target.port,
        other_capture_before.target.port
    );
    assert_eq!(
        other_capture_after.nemu.instance_id,
        Some(i32::from(INDEX_OTHER))
    );
    assert_eq!(*reads.lock().expect("reads"), vec![INDEX_SHIFTED]);
}
