// SPDX-License-Identifier: AGPL-3.0-only

use super::{ForensicError, ForensicResult, query_view_page};
use crate::codes::{ForensicsCode, ForensicsLocation};
use actingcommand_contract::{
    EventPayload, EventQuery, EventType, ExecutionBackendProvenance, InstanceId,
    MAX_RUNTIME_EVENT_QUERY_EVENTS, ProjectionPayload, ProjectionProfile, RuntimeContractError,
    RuntimeEventQueryPageRequest, RuntimeInstanceDeviceBinding, RuntimePayload,
};
use actingcommand_ledger::GlobalLedgerMetadata;
use std::collections::{BTreeMap, BTreeSet};

/// The latest `runtime.instance_bound` fact of one instance through the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceBinding {
    pub instance_id: InstanceId,
    pub instance_alias: String,
    pub provenance: ExecutionBackendProvenance,
    pub adb_host: Option<String>,
    pub adb_port: Option<u16>,
    pub serial_configured: bool,
    /// Workflow #398 RD-1b: the fingerprint of a handset's device (`sha256:<hex>`), when bound.
    pub device_fingerprint: Option<String>,
    pub sequence: u64,
}

/// Instance identity by ADB port and, for a handset, by device fingerprint, derived from every
/// binding fact through the snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceBindings {
    pub through_sequence: u64,
    /// The latest binding per instance id.
    pub latest: BTreeMap<InstanceId, InstanceBinding>,
    /// Every instance id ever bound to the port as HOST:PORT, in first-binding order.
    /// A port is the identity of an emulator instance, so its members are one instance.
    pub ports: BTreeMap<u16, Vec<InstanceId>>,
    /// Workflow #398 RD-1b (model-398 v1.1 §3.4): every instance id ever bound to the device
    /// fingerprint, in first-binding order. A fingerprint is the identity of a handset, so its
    /// members are one instance.
    pub devices: BTreeMap<String, Vec<InstanceId>>,
    /// Instances outside the port map: serial-configured or without an ADB port.
    /// Their latest binding remains in `latest`.
    pub unported: BTreeSet<InstanceId>,
}

/// Reads every `runtime.instance_bound` fact with sequence <= `through_sequence` from the
/// opened metadata snapshot with its full payload, through the same page read as `views`.
pub fn instance_bindings(
    snapshot: &GlobalLedgerMetadata,
    through_sequence: u64,
) -> ForensicResult<InstanceBindings> {
    let mut bindings = InstanceBindings {
        through_sequence,
        ..InstanceBindings::default()
    };
    if through_sequence == 0 {
        return Ok(bindings);
    }
    let invalid_page = |error: RuntimeContractError| {
        ForensicError::new(
            error.code(),
            ForensicsLocation::ReadInstanceBindings.location(),
            "invalid binding page request",
        )
    };
    let query = EventQuery {
        event_type: Some(EventType::RuntimeInstanceBound),
        ..EventQuery::default()
    };
    let mut request = RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, None)
        .and_then(|request| request.at_snapshot(through_sequence))
        .map_err(invalid_page)?;
    let mut ported = BTreeSet::new();
    loop {
        let page = query_view_page(snapshot, &query, ProjectionProfile::Forensic, &request)?;
        if page.read_scope().is_none_or(|scope| !scope.read_complete) {
            return Err(ForensicError::new(
                ForensicsCode::InstanceBindingsIncomplete.code(),
                ForensicsLocation::ReadInstanceBindings.location(),
                "binding facts are not completely readable through the snapshot",
            ));
        }
        for event in page.events() {
            let malformed = |detail: &str| {
                ForensicError::new(
                    ForensicsCode::InstanceBindingMalformed.code(),
                    ForensicsLocation::ReadInstanceBindings.location(),
                    format!("sequence {}: {detail}", event.sequence),
                )
            };
            let ProjectionPayload::Full(payload) = &event.payload else {
                return Err(malformed("payload is not the full projection"));
            };
            let EventPayload::Runtime(RuntimePayload::InstanceBound(binding)) = payload.as_ref()
            else {
                return Err(malformed("payload is not runtime.instance_bound"));
            };
            let Some(instance_id) = event.links.instance_id().copied() else {
                return Err(malformed("links carry no instance_id"));
            };
            if let Some(port) = binding.adb_port().filter(|_| !binding.serial_configured()) {
                let members = bindings.ports.entry(port).or_default();
                if !members.contains(&instance_id) {
                    members.push(instance_id);
                }
                ported.insert(instance_id);
            }
            let device_fingerprint = binding
                .device()
                .and_then(RuntimeInstanceDeviceBinding::fingerprint)
                .map(|fingerprint| fingerprint.as_str().to_owned());
            if let Some(fingerprint) = &device_fingerprint {
                let members = bindings.devices.entry(fingerprint.clone()).or_default();
                if !members.contains(&instance_id) {
                    members.push(instance_id);
                }
            }
            bindings.latest.insert(
                instance_id,
                InstanceBinding {
                    instance_id,
                    instance_alias: binding.instance_alias().to_owned(),
                    provenance: binding.provenance(),
                    adb_host: binding.adb_host().map(str::to_owned),
                    adb_port: binding.adb_port(),
                    serial_configured: binding.serial_configured(),
                    device_fingerprint,
                    sequence: event.sequence,
                },
            );
        }
        match page.next_cursor() {
            Some(cursor) => {
                request = RuntimeEventQueryPageRequest::new(
                    MAX_RUNTIME_EVENT_QUERY_EVENTS,
                    Some(cursor.clone()),
                )
                .map_err(invalid_page)?;
            }
            None => break,
        }
    }
    bindings.unported = bindings
        .latest
        .keys()
        .filter(|instance_id| !ported.contains(*instance_id))
        .copied()
        .collect();
    Ok(bindings)
}

#[cfg(test)]
mod tests {
    // Workflow #398 RD-1b, test row T7 (model-398 v1.1 §3.4, §12): a device fingerprint is a
    // handset's identity, beside the port as an emulator's. Two instance ids bound to the same
    // fingerprint one after the other are one group; the port map is what the emulator's
    // binding alone gives.
    use super::*;
    use actingcommand_contract::{
        AuditInput, DeviceBoundVia, EventActor, EventDraft, EventLinksDraft, EventOrigin,
        EventSeverity, EventSource, IdentifierIssuer, InstanceBindingSource,
        InstanceDeviceBindingDraft, OriginModule, RuntimePayloadDraft, SecretField,
        SecretFingerprinter,
    };
    use actingcommand_ledger::{
        GlobalLedger, GlobalLedgerError, GlobalLedgerEvidenceConfig, LedgerMaintenance,
        Sha256SecretFingerprinter,
    };
    use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
    use std::path::Path;
    use std::sync::Arc;

    const SALT: &[u8] = b"neutral-forensics-salt";
    const EMULATOR_PORT: u16 = 16384;

    /// One instance's binding: an emulator on `127.0.0.1:<port>`, or a handset bound to the
    /// device `serial`.
    enum Bound<'a> {
        Emulator(u16),
        Handset(&'a str),
    }

    fn write_bindings(root: &Path, bindings: &[(InstanceId, &str, Bound<'_>)]) {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let fingerprinter = Sha256SecretFingerprinter::new(SALT).expect("fingerprinter");
        // A formal root, as acsetup leaves it: the Runtime database, the formal ledger marker
        // and the writer.
        let database = Arc::new(
            RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
                root,
                b"neutral-forensics-seed",
                |_| Ok(()),
                |_| Ok(()),
            )
            .expect("runtime database"),
        );
        let limits = MaintenanceLimits::default();
        let maintenance =
            LedgerMaintenance::acquire(root, true, limits, limits.deadline().expect("deadline"))
                .expect("maintenance owner");
        maintenance
            .initialize_empty(&database)
            .expect("formal ledger");
        let ledger = maintenance
            .open_writer(database, "neutral-device-bindings".into())
            .expect("formal writer");
        for (instance_id, alias, bound) in bindings {
            let (host, port, serial) = match bound {
                Bound::Emulator(port) => (Some("127.0.0.1".to_owned()), Some(*port), None),
                Bound::Handset(serial) => (None, None, Some(*serial)),
            };
            let payload = RuntimePayloadDraft::instance_bound(
                *alias,
                ExecutionBackendProvenance::PhysicalDevice,
                host,
                port,
                serial.is_some(),
                InstanceBindingSource::Explicit,
                None,
                None,
                None,
                serial.map(|serial| {
                    InstanceDeviceBindingDraft::handset(serial, 1_000, DeviceBoundVia::Cli)
                }),
                AuditInput::new(),
            );
            let links = EventLinksDraft::default()
                .with_request_id(ids.mint_request_id().expect("request id"))
                .with_correlation_id(ids.mint_correlation_id().expect("correlation id"))
                .with_action_id(ids.mint_action_id().expect("action id"))
                .with_instance_id(ids.issue_registered_instance(*instance_id));
            let draft = EventDraft::new(
                ids.mint_event_id().expect("event id"),
                1_000,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                links,
                payload.into(),
            )
            .sanitize(&fingerprinter)
            .expect("a sanitized binding");
            ledger.append(draft).expect("append the binding");
        }
        ledger.close().expect("close the ledger");
    }

    fn read_bindings(root: &Path) -> InstanceBindings {
        let snapshot = GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(root))
            .expect("ledger metadata");
        instance_bindings(&snapshot, snapshot.latest_sequence()).expect("instance bindings")
    }

    fn fingerprint(serial: &str) -> String {
        Sha256SecretFingerprinter::new(SALT)
            .expect("fingerprinter")
            .fingerprint(SecretField::DeviceSerial, serial)
            .expect("fingerprint")
            .as_str()
            .to_owned()
    }

    #[test]
    fn rd1b_t7_a_device_fingerprint_groups_its_instances_beside_the_port_map() {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let instance = || *ids.mint_instance_id().expect("instance id").transport();
        let (emulator, first, second, other) = (instance(), instance(), instance(), instance());
        let emulator_binding = (emulator, "neutral.emulator", Bound::Emulator(EMULATOR_PORT));

        let alone = tempfile::tempdir().expect("tempdir");
        write_bindings(
            alone.path(),
            &[(emulator, "neutral.emulator", Bound::Emulator(EMULATOR_PORT))],
        );
        let alone = read_bindings(alone.path());

        let root = tempfile::tempdir().expect("tempdir");
        write_bindings(
            root.path(),
            &[
                emulator_binding,
                (first, "neutral.handset", Bound::Handset("TESTSERIAL0001")),
                (second, "neutral.handset", Bound::Handset("TESTSERIAL0001")),
                (other, "neutral.other", Bound::Handset("TESTSERIAL0002")),
            ],
        );
        let bindings = read_bindings(root.path());

        assert_eq!(
            bindings.devices,
            BTreeMap::from([
                (fingerprint("TESTSERIAL0001"), vec![first, second]),
                (fingerprint("TESTSERIAL0002"), vec![other]),
            ])
        );
        // The port map and the emulator's binding are what its binding alone gives.
        assert_eq!(
            bindings.ports,
            BTreeMap::from([(EMULATOR_PORT, vec![emulator])])
        );
        assert_eq!(bindings.ports, alone.ports);
        assert_eq!(bindings.latest.get(&emulator), alone.latest.get(&emulator));
        assert_eq!(bindings.latest[&emulator].device_fingerprint, None);
        // A handset records no host or port, and its latest binding names its fingerprint.
        for (instance_id, serial) in [
            (first, "TESTSERIAL0001"),
            (second, "TESTSERIAL0001"),
            (other, "TESTSERIAL0002"),
        ] {
            let binding = &bindings.latest[&instance_id];
            assert_eq!(
                (binding.adb_host.as_deref(), binding.adb_port),
                (None, None)
            );
            assert!(binding.serial_configured);
            assert_eq!(binding.device_fingerprint, Some(fingerprint(serial)));
        }
        assert_eq!(
            bindings.unported,
            BTreeSet::from([first, second, other]),
            "handsets stay outside the port map"
        );
    }
}
