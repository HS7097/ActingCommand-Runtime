// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #437 ORD-1 (register §1.348; the owner's rulings on board #437 as the coordinator
// recorded them, not the owner's words; contracts/scheduling/v2/README.md;
// contracts/standard-pack-schedule.md, "`requires`"):
// - ORD-1a: a V2 task may declare `max_queue_wait_ms`, the longest time its candidate may wait in
//   its device's queue, from one minute to one day; a V1 document that names it, a value of
//   another type and a value outside the range are refused at the member. A `schedule.json` rule
//   writes it as any V2 member, it passes through to the composed task, and the file names
//   `queue_wait` in `requires`. A catalog without it keeps its golden hashes.
//
// Every catalog is neutral; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS};
use crate::{
    CatalogCompileFailure, CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources,
    QUEUE_WAIT_SUBSYSTEM, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2,
    StandardPackBinding, StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    compile_catalog, compose_standard_pack_rules,
};
use actingcommand_contract::{
    BundleIndex, StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
};

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
/// The task member ORD-1a adds.
const MEMBER: &str = "max_queue_wait_ms";
/// The neutral example catalog's golden identities (as in `gate_sched_c1a`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// ORD-1a test 1, the compiler. In a V1 document the member is `unknown_field`; a string, `null`
// and a fraction are `type_mismatch`; 0, 59 999, 86 400 001 and 2^53 − 1 are `limit_exceeded`,
// each at `/tasks/0/max_queue_wait_ms`. The bounds 60 000 and 86 400 000 compile, and the compiled
// task carries the value written.
#[test]
fn ord1a_t1_the_compiler_holds_max_queue_wait_ms_to_v2_and_its_range() {
    let pointer = format!("/tasks/0/{MEMBER}");
    let failure = compile_catalog(&sources(
        SCHEDULING_SCHEMA_VERSION,
        vec![with(
            task_body("task.v1", "instance-a"),
            MEMBER,
            json!(600_000),
        )],
    ))
    .expect_err("V1 names no max_queue_wait_ms");
    assert!(
        refused_at(&failure, CatalogDiagnosticCode::UnknownField, &pointer),
        "{:?}",
        failure.diagnostics()
    );

    let refused = |case: &str, value: Value, code: CatalogDiagnosticCode| {
        let failure = compile_catalog(&sources(
            SCHEDULING_SCHEMA_VERSION_V2,
            vec![with(task_body("task.v2", "instance-a"), MEMBER, value)],
        ))
        .expect_err(case);
        assert!(
            refused_at(&failure, code, &pointer),
            "{case}: {:?}",
            failure.diagnostics()
        );
    };
    for (case, value) in [
        ("a string", json!("600000")),
        ("null", Value::Null),
        ("a fraction", json!(1.5)),
    ] {
        refused(case, value, CatalogDiagnosticCode::TypeMismatch);
    }
    for value in [0_u64, 59_999, 86_400_001, 9_007_199_254_740_991] {
        refused(
            &format!("{value}"),
            json!(value),
            CatalogDiagnosticCode::LimitExceeded,
        );
    }

    let compiled = compile_catalog(&sources(
        SCHEDULING_SCHEMA_VERSION_V2,
        vec![
            with(task_body("task.least", "instance-a"), MEMBER, json!(60_000)),
            with(
                task_body("task.most", "instance-a"),
                MEMBER,
                json!(86_400_000),
            ),
            task_body("task.none", "instance-a"),
        ],
    ))
    .unwrap_or_else(|failure| panic!("the bounds compile: {:?}", failure.diagnostics()));
    let limits = compiled
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task.max_queue_wait_ms))
        .collect::<Vec<_>>();
    assert_eq!(
        limits,
        [
            ("task.least", Some(60_000)),
            ("task.most", Some(86_400_000)),
            ("task.none", None),
        ]
    );
}

// ORD-1a test 2, the composer. A rule's `max_queue_wait_ms` passes through to its composed task
// as written, and a rule without it composes none. Refused: a file that writes it without naming
// `queue_wait` in `requires`, `schedule_invalid` at the first such rule's member; a value the
// compiler refuses, mapped back to the rule's member; and, on a Runtime whose manifest does not
// name `queue_wait`, the file at `requires` (`schedule_requires_missing`).
#[test]
fn ord1a_t2_the_composer_passes_it_through_and_refuses_it_without_requires() {
    let requires = [QUEUE_WAIT_SUBSYSTEM];
    let rules = || {
        vec![
            schedule_rule("rule.plain"),
            with(schedule_rule("rule.waits"), MEMBER, json!(600_000)),
        ]
    };
    let composed = compose(&schedule(rules(), &requires), None)
        .unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let limits = composed
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task.max_queue_wait_ms))
        .collect::<Vec<_>>();
    assert_eq!(
        limits,
        [
            ("rule.plain".to_owned(), None),
            ("rule.waits".to_owned(), Some(600_000)),
        ]
    );
    let tasks = serde_json::from_slice::<Value>(&composed.sources().tasks.bytes)
        .expect("the composed tasks document");
    assert_eq!(tasks["tasks"][1][MEMBER], json!(600_000), "{tasks}");
    assert!(tasks["tasks"][0].get(MEMBER).is_none(), "{tasks}");

    // `requires` does not name the subsystem: the first rule that writes the member.
    let failure = compose(&schedule(rules(), &[]), None).expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, format!("/rules/1/{MEMBER}"));

    // A value the compiler refuses, mapped back to the rule.
    let mut short = rules();
    short[1][MEMBER] = json!(59_999);
    let failure = compose(&schedule(short, &requires), None).expect_err("out of range");
    assert!(
        failure.issues().iter().any(|refused| {
            refused.issue.problem == StandardPackProblem::ScheduleInvalid
                && refused.issue.pointer == format!("/rules/1/{MEMBER}")
        }),
        "{failure}"
    );

    // A Runtime whose manifest does not name the subsystem refuses the file at `requires`.
    let older = BTreeSet::from(["standard_pack_schedule".to_owned()]);
    let failure = compose(&schedule(rules(), &requires), Some(&older)).expect_err("older");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleRequiresMissing);
    assert_eq!(issue.pointer, "/requires/0");
}

// ORD-1a test 3, identity. The neutral example catalog keeps its V1 and V2 golden identities, and
// a V2 catalog whose tasks do not write the member serializes none.
#[test]
fn ord1a_t3_without_the_member_every_catalog_hash_is_unchanged() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example(version))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
        let text = serde_json::to_string(&example.catalog().tasks.tasks).expect("tasks");
        assert!(!text.contains(MEMBER), "{version}: {text}");
    }
    let plain = compile_catalog(&sources(
        SCHEDULING_SCHEMA_VERSION_V2,
        vec![task_body("task.plain", "instance-a")],
    ))
    .unwrap_or_else(|failure| panic!("the plain catalog compiles: {:?}", failure.diagnostics()));
    let text = serde_json::to_string(&plain.catalog().tasks.tasks).expect("tasks");
    assert!(!text.contains(MEMBER), "{text}");
}

// ---- The ORD-1a fixture

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

fn refused_at(failure: &CatalogCompileFailure, code: CatalogDiagnosticCode, path: &str) -> bool {
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
}

/// One hourly task of `instance_id`, priority 100, a minute long, written with every run-count
/// bound so that it compiles as V1 too; its feedback stop stays false.
fn task_body(task_id: &str, instance_id: &str) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "instance", "instance_id": instance_id},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": format!("procedure.{task_id}"),
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": HOUR_MS,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": task_id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// One activity profile of the game, open all day on every weekday.
fn profile() -> Value {
    json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7200000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300000,
            "expected_duration_ms": 60000
        },
        "minimum_interval_ms": 60000,
        "maximum_interval_ms": 300000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1000,
        "goals": []
    })
}

/// The four documents of a catalog of `version` with `tasks`, no pool and no event.
fn sources(version: &str, tasks: Vec<Value>) -> CatalogSources {
    let header = json!({
        "catalog_id": "fixture.ord1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-ord1"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({"schema_version": version, "catalog": header});
        document[member] = entries;
        document
    };
    CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile()]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    }
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-ord1/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed).
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
    CatalogSources {
        tasks: source("example-tasks", &tasks),
        pools: source("example-pools", &pools),
        activity: source("example-activity", &activity),
        timeline: source("example-timeline", &timeline),
        selection: None,
    }
}

/// A `schedule.json` rule running the package of its own id: [`task_body`] without scope,
/// entrypoint, procedure and instance overrides, with `package_id`.
fn schedule_rule(id: &str) -> Value {
    let mut rule = task_body(id, "instance-a");
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
    rule
}

/// `schedule.json` with these rules, naming `requires`.
fn schedule(rules: Vec<Value>, requires: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": [],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for one instance of the game, whose registry holds a package per rule; with
/// `subsystems`, a Runtime manifest checks `requires`.
fn compose(
    schedule: &Value,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = schedule["rules"]
        .as_array()
        .expect("the rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package").to_owned())
        .collect::<BTreeSet<_>>();
    let bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packages
            .iter()
            .map(|package_id| json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>()
    });
    let bundle = serde_json::to_vec(&bundle).expect("bundle bytes");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let maintenance = serde_json::from_slice::<BundleIndex>(&bundle)
        .expect("a bundle index")
        .maintenance()
        .to_vec();
    let schedule = serde_json::to_vec(schedule).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &maintenance,
        },
    )]);
    let instances = BTreeMap::from([(
        "instance-a".to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems,
        catalog_version: 1,
    })
}
