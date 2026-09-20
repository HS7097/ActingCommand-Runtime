// SPDX-License-Identifier: AGPL-3.0-only

use super::{ForensicError, ForensicResult, query_view_page};
use actingcommand_contract::{
    EventPayload, EventQuery, EventType, ExecutionBackendProvenance, InstanceId,
    MAX_RUNTIME_EVENT_QUERY_EVENTS, ProjectionPayload, ProjectionProfile, RuntimeContractError,
    RuntimeEventQueryPageRequest, RuntimePayload,
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
    pub sequence: u64,
}

/// Instance identity by ADB port, derived from every binding fact through the snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceBindings {
    pub through_sequence: u64,
    /// The latest binding per instance id.
    pub latest: BTreeMap<InstanceId, InstanceBinding>,
    /// Every instance id ever bound to the port as HOST:PORT, in first-binding order.
    /// A port is the identity of an emulator instance, so its members are one instance.
    pub ports: BTreeMap<u16, Vec<InstanceId>>,
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
            "read_instance_bindings",
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
                "instance_bindings_incomplete",
                "read_instance_bindings",
                "binding facts are not completely readable through the snapshot",
            ));
        }
        for event in page.events() {
            let malformed = |detail: &str| {
                ForensicError::new(
                    "instance_binding_malformed",
                    "read_instance_bindings",
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
            bindings.latest.insert(
                instance_id,
                InstanceBinding {
                    instance_id,
                    instance_alias: binding.instance_alias().to_owned(),
                    provenance: binding.provenance(),
                    adb_host: binding.adb_host().map(str::to_owned),
                    adb_port: binding.adb_port(),
                    serial_configured: binding.serial_configured(),
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
