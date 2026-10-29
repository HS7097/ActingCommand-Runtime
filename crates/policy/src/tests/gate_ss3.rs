// SPDX-License-Identifier: AGPL-3.0-only

// SS-3 (Alice's ruling 100 on Workflow #386, comment 6490879952; rd-1029 design-sch-state §5
// SS-3, test 6; register §1.212): run-count quotas become optional, and a catalog that writes all
// three `loop_budget` items and the three activity bounds evaluates identically to before. Its
// identity (the catalog hash pinned before SS-3), its compiled documents and the loop budget its
// dispatch intent pins are byte for byte those of before.

use serde_json::{Value, json};

use super::selection_catalog::{NOW, catalog, evaluate_fixture, instance};
use crate::{
    CatalogDocumentSource, CatalogSources, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2,
    compile_catalog,
};

/// The example catalog `catalog-a`, which writes every bound, declared as `version`.
fn example(version: &str) -> CatalogSources {
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
        declared["schema_version"] = json!(version);
        if version == SCHEDULING_SCHEMA_VERSION_V2
            && let Some(events) = declared.get_mut("events").and_then(Value::as_array_mut)
        {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
    }
    let [tasks, pools, activity, timeline] = documents;
    let source = |name: &str, value: &Value| {
        CatalogDocumentSource::new(
            format!("memory://catalog-a/{name}.json"),
            serde_json::to_vec_pretty(value).expect("example bytes"),
        )
    };
    CatalogSources {
        tasks: source("tasks", &tasks),
        pools: source("pools", &pools),
        activity: source("activity", &activity),
        timeline: source("timeline", &timeline),
        selection: None,
    }
}

#[test]
fn ss3_t6_a_catalog_that_writes_every_bound_evaluates_identically() {
    // The identity pinned before SS-3 (`compiler::tests::neutral_catalog_compiles_to_stable_hash_
    // and_dry_run`): the V1 example's hash is unchanged.
    let v1 = compile_catalog(&example(SCHEDULING_SCHEMA_VERSION)).expect("the V1 example");
    assert_eq!(
        v1.catalog_hash(),
        "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c"
    );

    // In V1 and V2 alike, every written bound serializes as it is written, and nothing else.
    for version in [SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2] {
        let compiled = compile_catalog(&example(version)).expect("the example compiles");
        let tasks = serde_json::to_value(&compiled.catalog().tasks).expect("tasks JSON");
        assert_eq!(
            tasks["tasks"][0]["loop_budget"],
            json!({"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000}),
            "{version}"
        );
        let activity = serde_json::to_value(&compiled.catalog().activity).expect("activity JSON");
        for profile in activity["profiles"].as_array().expect("profiles") {
            assert_eq!(profile["daily_budget"], json!(24), "{version}");
            assert_eq!(profile["max_window_iterations"], json!(4), "{version}");
            assert_eq!(profile["session_max_ms"], json!(7_200_000), "{version}");
        }
    }

    // The dispatch intent of a catalog that writes all three items pins them with the bytes of
    // before: three plain integers between the profile id and the urgency.
    let compiled = catalog(&[("task-a", 1)], None);
    let evaluation = evaluate_fixture(&compiled, &[instance("instance-a", &[])], Vec::new())
        .expect("the fixture evaluates");
    assert_eq!(
        evaluation.dispatch_intents.len(),
        1,
        "the fixture task is due"
    );
    let prerequisites = &evaluation.dispatch_intents[0].prerequisites;
    assert_eq!(prerequisites.evaluated_at_unix_ms, NOW);
    assert_eq!(
        serde_json::to_string(prerequisites).expect("prerequisites JSON"),
        format!(
            concat!(
                r#"{{"fencing_required":true,"evaluated_at_unix_ms":{},"#,
                r#""facts_fresh_until_unix_ms":{},"activity_profile_id":"fixture-activity","#,
                r#""daily_limit":24,"window_iteration_limit":4,"max_runtime_ms":300000,"#,
                r#""urgency_milli":{}}}"#
            ),
            NOW,
            serde_json::to_string(&prerequisites.facts_fresh_until_unix_ms)
                .expect("freshness JSON"),
            prerequisites.urgency_milli,
        )
    );
}
