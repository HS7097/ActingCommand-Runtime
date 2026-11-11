// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-LB1a (since 0.12.55; register §1.309 row LB1-4, the slice's section §1.311;
// `contracts/scheduling/README.md`, "Record lists and the projection"): the catalog compile
// refuses a member that names a key under `inventory.list.` to read, `type_mismatch` at its key:
// a `fact` and a `record_deadline` predicate, a fact reference, a pool's ledger key, a pool's fact
// reference, a consumption read from a fact and a fact the selection document declares. The same
// member naming another key is not refused at that pointer, and a timeline event's prefixes, which
// name keys to invalidate, are not refused. On the neutral example catalog `catalog-a`, declared
// V2 (a reference and a consumption read from a fact are V2 members). Every expected value is the
// contract's; the keys are made up.

use serde_json::{Value, json};

use super::selection_catalog::{drop_candidate, fact_ref, gate, integer_fact, selection};
use crate::{
    CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources, SCHEDULING_SCHEMA_VERSION_V2,
    compile_catalog,
};

const LIST_KEY: &str = "inventory.list.example_kind_a";
const OTHER_KEY: &str = "inventory.example_rows";

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-s168-lb1a/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

// The four documents of `catalog-a` declared V2 (a timeline event gets validity {from 0, until
// null}), each edited by `edit` (tasks, pools, timeline), with `selection` as the fifth document.
fn example(
    edit: impl FnOnce(&mut Value, &mut Value, &mut Value),
    selection: Option<&Value>,
) -> CatalogSources {
    let parse = |bytes: &[u8]| -> Value { serde_json::from_slice(bytes).expect("example JSON") };
    let mut documents = [
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/pools.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/activity.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
        )),
    ];
    for declared in &mut documents {
        declared["schema_version"] = json!(SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = declared.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
    }
    let [mut tasks, mut pools, activity, mut timeline] = documents;
    edit(&mut tasks, &mut pools, &mut timeline);
    CatalogSources {
        tasks: source("tasks", &tasks),
        pools: source("pools", &pools),
        activity: source("activity", &activity),
        timeline: source("timeline", &timeline),
        selection: selection.map(|document| source("selection", document)),
    }
}

// Whether compiling `sources` is refused with `type_mismatch` at `path`.
fn refused_at(sources: &CatalogSources, path: &str) -> bool {
    compile_catalog(sources).err().is_some_and(|failure| {
        failure.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == CatalogDiagnosticCode::TypeMismatch && diagnostic.json_path == path
        })
    })
}

// The diagnostics of compiling `sources`, for a failed assertion's message.
fn diagnostics(sources: &CatalogSources) -> Vec<(CatalogDiagnosticCode, String)> {
    match compile_catalog(sources) {
        Ok(_) => Vec::new(),
        Err(failure) => failure
            .diagnostics()
            .iter()
            .map(|diagnostic| (diagnostic.code, diagnostic.json_path.clone()))
            .collect(),
    }
}

// The example task's second trigger predicate (a `fact` predicate of the instance's scope).
const PREDICATE: &str = "/tasks/0/trigger/predicates/1";

fn record_deadline(key: &str) -> Value {
    json!({
        "kind": "record_deadline",
        "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
        "fact_key": key,
        "timestamp_field": "due_at_unix_ms",
        "within_ms": 3_600_000,
        "max_age_ms": 900_000
    })
}

fn consumption(key: &str) -> Value {
    json!([{
        "pool_id": "fixture-pool-a",
        "direction": "consume",
        "amount": {"type": "fact", "fact_key": key, "fallback": 1},
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    }])
}

// One case per place the row lists: each member naming a key under `inventory.list.` is refused
// `type_mismatch` at its key, and the same member naming another key is not refused there.
#[test]
fn lb1a_a_catalog_that_names_a_list_key_to_read_is_refused_at_its_key() {
    type Edit = fn(&mut Value, &mut Value, &mut Value, &str);
    let cases: [(&str, Edit, String); 6] = [
        (
            "a fact predicate",
            |tasks, _, _, key| {
                tasks["tasks"][0]["trigger"]["predicates"][1]["fact_key"] = json!(key)
            },
            format!("{PREDICATE}/fact_key"),
        ),
        (
            "a record deadline predicate",
            |tasks, _, _, key| {
                tasks["tasks"][0]["trigger"]["predicates"][1] = record_deadline(key);
            },
            format!("{PREDICATE}/fact_key"),
        ),
        (
            "a fact reference",
            |tasks, _, _, key| {
                tasks["tasks"][0]["trigger"]["predicates"][1]["value"] =
                    json!({"type": "fact", "fact_key": key, "offset": 0});
            },
            format!("{PREDICATE}/value/fact_key"),
        ),
        (
            "a pool's ledger key",
            |_, pools, _, key| pools["pools"][0]["observation"]["fact_key"] = json!(key),
            "/pools/0/observation/fact_key".to_owned(),
        ),
        (
            "a pool's fact reference",
            |_, pools, _, key| {
                pools["pools"][0]["capacity"] =
                    json!({"type": "fact", "fact_key": key, "offset": 0});
            },
            "/pools/0/capacity/fact_key".to_owned(),
        ),
        (
            "a consumption read from a fact",
            |tasks, _, _, key| tasks["tasks"][0]["consumes"] = consumption(key),
            "/tasks/0/consumes/0/amount/fact_key".to_owned(),
        ),
    ];
    for (case, edit, path) in cases {
        let listed = example(
            |tasks, pools, timeline| edit(tasks, pools, timeline, LIST_KEY),
            None,
        );
        assert!(
            refused_at(&listed, &path),
            "{case}: {:?}",
            diagnostics(&listed)
        );
        let other = example(
            |tasks, pools, timeline| edit(tasks, pools, timeline, OTHER_KEY),
            None,
        );
        assert!(
            !refused_at(&other, &path),
            "{case} of another key: {:?}",
            diagnostics(&other)
        );
    }

    // A fact the selection document declares: refused at its `fact_key` in the document; the same
    // document of another key is not refused there.
    let document = |key: &str| {
        selection(
            Vec::new(),
            vec![integer_fact(key, 86_400_000)],
            vec![gate("gate-held", at_least_one(key), drop_candidate())],
            Vec::new(),
        )
    };
    let listed = example(|_, _, _| {}, Some(&document(LIST_KEY)));
    assert!(
        refused_at(&listed, "/facts/0/fact_key"),
        "a fact the selection document declares: {:?}",
        diagnostics(&listed)
    );
    let other = example(|_, _, _| {}, Some(&document(OTHER_KEY)));
    assert!(
        !refused_at(&other, "/facts/0/fact_key"),
        "another key: {:?}",
        diagnostics(&other)
    );
}

// `integer_at_least` over the fact source of `key`, threshold 1.
fn at_least_one(key: &str) -> Value {
    json!({"kind": "integer_at_least", "value": fact_ref(key), "threshold": 1})
}

// A timeline event's prefixes name keys to invalidate, not to read: an event that invalidates
// `inventory.list.` compiles, and the example compiles without any edit.
#[test]
fn lb1a_a_timeline_prefix_under_the_list_prefix_is_not_refused() {
    let plain = example(|_, _, _| {}, None);
    compile_catalog(&plain)
        .unwrap_or_else(|failure| panic!("the example: {:?}", failure.diagnostics()));
    let invalidating = example(
        |_, _, timeline| {
            timeline["events"][0]["invalidates_fact_prefixes"] =
                json!(["session.", "timeline.", "inventory.list."]);
        },
        None,
    );
    compile_catalog(&invalidating).unwrap_or_else(|failure| {
        panic!("a list prefix to invalidate: {:?}", failure.diagnostics())
    });
}
