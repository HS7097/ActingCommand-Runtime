// SPDX-License-Identifier: AGPL-3.0-only

// `actingledger --state-root <root> views --instance-port <port>` (contracts/global-ledger-query.md,
// "Offline view entry"; an ADB port is the identity of an emulator instance, so every instance id
// ever bound to the same port through `runtime.instance_bound` is one instance). The port
// resolves at the page's snapshot to every instance id ever bound to it, and the page runs with
// that set as `instance_ids`. No bound instance is `instance_port_unknown`; a `--query` that
// already carries `instance_id` or `instance_ids` other than exactly that set is
// `instance_port_conflict`.

use crate::fixture::{OpenRoot, at, cli, report};
use actingcommand_contract::{
    EventQuery, FactValue, InstanceId, IssuedInstanceId, OriginModule, RuntimeFactRecord,
    RuntimeFactScope,
};
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::TempDir;

// Neutral, obviously fake ADB ports: one bound twice, one bound once, one never bound.
const SHARED_PORT: u16 = 40_001;
const OTHER_PORT: u16 = 40_002;
const UNBOUND_PORT: u16 = 40_009;

// A closed formal root in which two instance ids were bound to the shared port one after the
// other and a third to the other port, each with an event of its own, beside a Runtime-scope
// event that links no instance.
struct PortRoot {
    directory: TempDir,
    first: InstanceId,
    second: InstanceId,
    other: InstanceId,
    // Every event linked to an instance id ever bound to the shared port: its sequence and its
    // instance id.
    shared: BTreeMap<u64, String>,
}

// The instance id as the page prints it.
fn id_text(instance_id: &InstanceId) -> String {
    serde_json::to_value(instance_id)
        .expect("instance id JSON")
        .as_str()
        .expect("instance id text")
        .to_owned()
}

// The instance's `device.connected` record.
fn connected(instance: IssuedInstanceId, observed_at_unix_ms: u64) -> RuntimeFactRecord {
    RuntimeFactRecord {
        scope: RuntimeFactScope::Instance {
            instance_id: *instance.transport(),
        },
        key: "device.connected".to_owned(),
        value: FactValue::Boolean(true),
        observed_at_unix_ms,
        source: OriginModule::Runtime,
        ttl_ms: None,
    }
}

fn port_root() -> PortRoot {
    let directory = tempfile::tempdir().expect("root directory");
    let writer = OpenRoot::create(directory.path());
    let (first, other, second) = (writer.instance(), writer.instance(), writer.instance());
    let mut shared = BTreeMap::new();
    shared.insert(
        writer.bind(at(1), first, "node.a", SHARED_PORT).sequence(),
        id_text(first.transport()),
    );
    writer.bind(at(2), other, "node.b", OTHER_PORT);
    shared.insert(
        writer.record_fact(&connected(first, at(3))).sequence(),
        id_text(first.transport()),
    );
    writer.record_fact(&connected(other, at(4)));
    // A later start binds the shared port under a new instance id.
    shared.insert(
        writer.bind(at(5), second, "node.a", SHARED_PORT).sequence(),
        id_text(second.transport()),
    );
    shared.insert(
        writer.record_fact(&connected(second, at(6))).sequence(),
        id_text(second.transport()),
    );
    writer.record_fact(&RuntimeFactRecord {
        scope: RuntimeFactScope::Runtime,
        key: "host.scheduling_pause".to_owned(),
        value: FactValue::Boolean(false),
        observed_at_unix_ms: at(7),
        source: OriginModule::Runtime,
        ttl_ms: None,
    });
    writer.close();
    PortRoot {
        directory,
        first: *first.transport(),
        second: *second.transport(),
        other: *other.transport(),
        shared,
    }
}

// One `views --instance-port <port> [--query <query>]`: the page's events as their sequence and
// instance link, or the failure's code.
fn views(
    root: &Path,
    port: u16,
    query: Option<&EventQuery>,
) -> Result<BTreeMap<u64, String>, String> {
    let port = port.to_string();
    let query = query.map(|query| serde_json::to_string(query).expect("query JSON"));
    let mut command = vec!["views", "--instance-port", port.as_str()];
    if let Some(query) = &query {
        command.extend(["--query", query.as_str()]);
    }
    let (result, output) = cli(root, &command);
    result.map_err(|error| error.code().to_owned())?;
    let page = report(&output);
    assert_eq!(page["command"], "views", "{page}");
    let events = page["data"]["events"]
        .as_array()
        .unwrap_or_else(|| panic!("the page's events: {page}"));
    Ok(events
        .iter()
        .map(|event| {
            (
                event["sequence"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("an event sequence: {event}")),
                event["links"]["instance_id"]
                    .as_str()
                    .unwrap_or("<no instance link>")
                    .to_owned(),
            )
        })
        .collect())
}

// The page of the shared port holds the events of both instance ids ever bound to it, and none
// of the other port's instance or of no instance. A query that carries exactly that set, in
// another order, is no conflict and runs the same page.
#[test]
fn gate_views_instance_port_holds_every_instance_ever_bound_to_the_port() {
    let root = port_root();
    let path = root.directory.path();
    assert_eq!(
        views(path, SHARED_PORT, None),
        Ok(root.shared.clone()),
        "port {SHARED_PORT}: both instance ids ever bound to it, and only they"
    );
    let exact = EventQuery {
        instance_ids: vec![root.second, root.first],
        ..EventQuery::default()
    };
    assert_eq!(
        views(path, SHARED_PORT, Some(&exact)),
        Ok(root.shared),
        "a query that carries exactly the port's set"
    );
}

// A port no instance was ever bound to fails with `instance_port_unknown`.
#[test]
fn gate_views_instance_port_without_a_binding_is_unknown() {
    let root = port_root();
    assert_eq!(
        views(root.directory.path(), UNBOUND_PORT, None),
        Err("instance_port_unknown".to_owned())
    );
}

// A query whose instance condition is not exactly the port's set fails with
// `instance_port_conflict`: one of its two instance ids, the other port's instance id, or the
// set together with the other port's instance id.
#[test]
fn gate_views_instance_port_with_another_instance_condition_conflicts() {
    let root = port_root();
    let path = root.directory.path();
    for (query, case) in [
        (
            EventQuery {
                instance_id: Some(root.first),
                ..EventQuery::default()
            },
            "one of the port's two instance ids",
        ),
        (
            EventQuery {
                instance_id: Some(root.other),
                ..EventQuery::default()
            },
            "the other port's instance id",
        ),
        (
            EventQuery {
                instance_ids: vec![root.first, root.second, root.other],
                ..EventQuery::default()
            },
            "the port's set and the other port's instance id",
        ),
    ] {
        assert_eq!(
            views(path, SHARED_PORT, Some(&query)),
            Err("instance_port_conflict".to_owned()),
            "{case}"
        );
    }
}
