// SPDX-License-Identifier: AGPL-3.0-only

// `actingledger --state-root <root> facts --at <sequence>` (contracts/runtime-fact-store.md,
// "Offline read"). The result is printed as one JSON line under `command: facts`. `available`
// is the store once every event through the position was applied: the `RuntimeFactSnapshot`
// the online read returns, records in scope-then-key order, `ledger_position` = `snapshot_id` =
// the position, part 1 of 1, `taken_at_unix_ms` the ledger timestamp of the event at the
// position, and `source` the final page's read scope (offline, complete, scanned through the
// position). `not_available` names `position_beyond_snapshot` for a position past the snapshot.
// The tool exits 0 only for `available`; otherwise the result is printed first and the tool
// exits nonzero with `runtime_facts_not_available` or the failure code.

use crate::fixture::{OpenRoot, at, cli, latest_sequence, report};
use actingcommand_contract::{
    FactValue, LedgerReadScope, LedgerReadSource, OriginModule, RUNTIME_FACT_SCHEMA_VERSION,
    RuntimeFactInvalidation, RuntimeFactInvalidationReason, RuntimeFactRecord, RuntimeFactScope,
    RuntimeFactSnapshot,
};
use actingcommand_ledger::PersistedEvent;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

// A neutral, obviously fake ADB port.
const PORT: u16 = 40_001;

// One record of `scope` produced by the Runtime, without a lifetime.
fn record(
    scope: &RuntimeFactScope,
    key: &str,
    value: FactValue,
    observed_at_unix_ms: u64,
) -> RuntimeFactRecord {
    RuntimeFactRecord {
        scope: scope.clone(),
        key: key.to_owned(),
        value,
        observed_at_unix_ms,
        source: OriginModule::Runtime,
        ttl_ms: None,
    }
}

// The records in scope-then-key order.
fn store_order(records: &[&RuntimeFactRecord]) -> Vec<RuntimeFactRecord> {
    records
        .iter()
        .map(|record| {
            (
                (record.scope.clone(), record.key.clone()),
                RuntimeFactRecord::clone(record),
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

// The one JSON line the command printed.
fn one_line(output: &[u8]) -> Value {
    let lines = output.iter().filter(|byte| **byte == b'\n').count();
    assert!(
        lines == 1 && output.ends_with(b"\n"),
        "one JSON line: {}",
        String::from_utf8_lossy(output)
    );
    report(output)
}

// `facts --at <the event's sequence>` exits 0 and prints the store holding exactly `records`,
// available at that position.
fn assert_available(root: &Path, event: &PersistedEvent, records: &[&RuntimeFactRecord]) {
    let position = event.sequence();
    let (result, output) = cli(root, &["facts", "--at", &position.to_string()]);
    assert!(
        result.is_ok(),
        "facts --at {position} exits 0 for available: {result:?}\n{}",
        String::from_utf8_lossy(&output)
    );
    let report = one_line(&output);
    assert_eq!(report["command"], "facts", "{report}");
    let data = &report["data"];
    assert_eq!(data["state"], "available", "{report}");
    assert_eq!(data["position"], position, "{report}");
    let expected = RuntimeFactSnapshot {
        schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
        ledger_position: position,
        taken_at_unix_ms: event.timestamp_unix_ms(),
        snapshot_id: position,
        part: 1,
        parts: 1,
        records: store_order(records),
    };
    assert_eq!(
        data["facts"],
        serde_json::to_value(&expected).expect("expected facts JSON"),
        "the facts available at {position}"
    );
    let source: LedgerReadScope =
        serde_json::from_value(data["source"].clone()).expect("the final page's read scope");
    assert_eq!(
        (
            source.source,
            source.read_complete,
            source.scanned_through_position
        ),
        (LedgerReadSource::Offline, true, position),
        "{report}"
    );
}

// The ledger of one Runtime: an instance bound, three records, the store sealed after them as
// the periodic snapshot seals it, a later change of one key, the instance's key dropped when its
// device closed, and the instance bound again by a later start. Every position shows the store
// through it: a key changed later shows its earlier value at an earlier position, and an
// invalidated key is gone from its invalidation on.
#[test]
fn gate_facts_at_a_position_are_the_store_replayed_through_that_position() {
    let directory = tempfile::tempdir().expect("root directory");
    let root = directory.path();
    let writer = OpenRoot::create(root);
    let device = writer.instance();
    let runtime = RuntimeFactScope::Runtime;
    let instance = RuntimeFactScope::Instance {
        instance_id: *device.transport(),
    };
    let paused = record(
        &runtime,
        "host.scheduling_pause",
        FactValue::Boolean(false),
        at(2),
    );
    let connected = record(
        &instance,
        "device.connected",
        FactValue::Boolean(true),
        at(3),
    );
    let marker = record(
        &runtime,
        "config.neutral_marker",
        FactValue::String("fixture".to_owned()),
        at(4),
    );
    let resumed = record(
        &runtime,
        "host.scheduling_pause",
        FactValue::Boolean(true),
        at(6),
    );

    writer.bind(at(1), device, "node.a", PORT);
    let paused_event = writer.record_fact(&paused);
    let connected_event = writer.record_fact(&connected);
    let marker_event = writer.record_fact(&marker);
    // The store sealed at the ledger's latest sequence, one part.
    let sealed_event = writer.seal_facts(RuntimeFactSnapshot {
        schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
        ledger_position: marker_event.sequence(),
        taken_at_unix_ms: at(5),
        snapshot_id: marker_event.sequence(),
        part: 1,
        parts: 1,
        records: store_order(&[&paused, &connected, &marker]),
    });
    let resumed_event = writer.record_fact(&resumed);
    let closed_event = writer.invalidate_fact(&RuntimeFactInvalidation {
        scope: instance,
        key: connected.key.clone(),
        reason: RuntimeFactInvalidationReason::DeviceClosed,
        at_unix_ms: at(7),
    });
    // The event at this position is no runtime fact event.
    let rebound_event = writer.bind(at(8), device, "node.a", PORT);
    writer.close();

    assert_available(root, &paused_event, &[&paused]);
    assert_available(root, &connected_event, &[&connected, &paused]);
    assert_available(root, &marker_event, &[&connected, &marker, &paused]);
    assert_available(root, &sealed_event, &[&marker, &connected, &paused]);
    assert_available(root, &resumed_event, &[&connected, &resumed, &marker]);
    assert_available(root, &closed_event, &[&resumed, &marker]);
    assert_available(root, &rebound_event, &[&resumed, &marker]);
}

// A position past the ledger's last sequence is `not_available` with
// `position_beyond_snapshot`; the result is printed first and the tool exits nonzero with
// `runtime_facts_not_available`.
#[test]
fn gate_facts_beyond_the_snapshot_are_not_available_and_exit_nonzero() {
    let directory = tempfile::tempdir().expect("root directory");
    let root = directory.path();
    let writer = OpenRoot::create(root);
    let device = writer.instance();
    writer.bind(at(1), device, "node.a", PORT);
    writer.record_fact(&record(
        &RuntimeFactScope::Instance {
            instance_id: *device.transport(),
        },
        "device.connected",
        FactValue::Boolean(true),
        at(2),
    ));
    writer.close();
    let latest = latest_sequence(root);
    let beyond = latest + 1;

    let (result, output) = cli(root, &["facts", "--at", &beyond.to_string()]);
    let report = one_line(&output);
    let error = result.expect_err("a position beyond the snapshot exits nonzero");
    assert_eq!(error.code(), "runtime_facts_not_available", "{report}");
    assert_eq!(report["command"], "facts", "{report}");
    assert_eq!(
        report["data"],
        json!({
            "state": "not_available",
            "position": beyond,
            "latest_sequence": latest,
            "reason": "position_beyond_snapshot"
        }),
        "{report}"
    );
}
