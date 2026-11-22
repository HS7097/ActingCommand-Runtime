// SPDX-License-Identifier: AGPL-3.0-only

// #455 P1 (0.12.78; register §1.504; the coordinator's rulings 1 to 9 of 11-21 in its work order,
// not the owner's words; contracts/scheduling/README.md, "A pool read from rows of a list" and
// "Record lists and the projection"; contracts/scheduling/v2/README.md;
// contracts/standard-pack-schedule.md, "`requires`"):
// - a V2 pool's `observation` may be of the kind `list_rows` beside `ledger_fact`: one row, four
//   rows with factors, a round trip, the compiled pool holding its rows in the order written (1);
// - each refusal at its pointer with a code the compile already has (2);
// - every other member that names a key under `inventory.list.` is still refused, and a
//   `list_rows` observation names no other key (3);
// - the composer carries the member and requires `pool_list_rows`, and a Runtime that does not
//   name the subsystem refuses the file at that entry (4);
// - no host gives such a pool a reading yet: its cost term is `benefit_unknown` and its capped
//   state `capped_cap_unknown`, under the list's key with `fact_missing`, never 0, though the list
//   is in the input and a caller hands the pool a snapshot of 0 (5, 6);
// - a catalog without the kind keeps its golden identity (7).
//
// The keys, columns and ids are made up; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources,
    CompiledCatalog, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FactScalar, FactValue, HostResourceSnapshot, ObservationRef, ObservedFact, POOL_LIST_ROWS_KIND,
    POOL_LIST_ROWS_SUBSYSTEM, PolicyCode, PolicyEvaluation, PolicyEvaluationError, PoolListRow,
    PoolValueSnapshot, PoolsDocument, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2,
    ScopeSelector, StandardPackBinding, StandardPackComposeInput, StandardPackComposition,
    StandardPackGameFiles, TaskDecision, compile_catalog, compose_standard_pack_rules,
    evaluate_with_eligibility, project_fact_pools,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{StandardPackProblem, StandardPackRootBytes, read_standard_pack_root};

/// The list reading the fixtures' pools are read from, and its two columns.
const LIST_KEY: &str = "inventory.list.example_kind_a";
const KEY_COLUMN: &str = "id";
const VALUE_COLUMN: &str = "count";
/// One row of the fixtures' list pools.
const ROW: &[(&str, u64)] = &[("item_a", 1)];
/// The neutral example catalog's golden identities (as in `gate_ord1`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the accepted shape. One row, and four rows with factors written out of key order: each
// compiles, the compiled pool holds the observation with its rows in the order written, it
// serializes as written under the kind `list_rows`, reads back equal, and the document it
// serializes to compiles again to the same catalog.
#[test]
fn p1_t1_a_list_rows_pool_compiles_round_trips_and_keeps_its_rows_in_written_order() {
    let four: [(&str, u64); 4] = [
        ("item_d", 200),
        ("item_a", 400),
        ("item_c", 1_000),
        ("item_b", 2_000),
    ];
    for rows in [ROW, &four[..]] {
        let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |_, pools| listed(pools, rows));
        let compiled = compile(&sources);
        let pools = &compiled.catalog().pools;
        assert_eq!(
            pools.pools[0].observation,
            ObservationRef::ListRows {
                fact_key: LIST_KEY.to_owned(),
                key_column: KEY_COLUMN.to_owned(),
                value_column: VALUE_COLUMN.to_owned(),
                rows: rows
                    .iter()
                    .map(|(key, factor)| PoolListRow {
                        key: (*key).to_owned(),
                        factor: *factor,
                    })
                    .collect(),
            },
            "{rows:?}"
        );
        let document = serde_json::to_value(pools).expect("the pools document serializes");
        assert_eq!(
            document["pools"][0]["observation"],
            list_rows(rows),
            "{rows:?}"
        );
        assert_eq!(
            document["pools"][0]["observation"]["kind"],
            json!(POOL_LIST_ROWS_KIND)
        );
        assert_eq!(POOL_LIST_ROWS_KIND, "list_rows");
        let read = serde_json::from_value::<PoolsDocument>(document.clone())
            .expect("the pools document reads back");
        assert_eq!(&read, pools, "{rows:?}");
        let mut again = sources.clone();
        again.pools.bytes = serde_json::to_vec(&document).expect("the document's bytes");
        let recompiled = compile(&again);
        assert_eq!(
            recompiled.catalog_hash(),
            compiled.catalog_hash(),
            "{rows:?}"
        );
        assert_eq!(&recompiled.catalog().pools, pools, "{rows:?}");
    }
}

// Test 2, the refusals, one case each, each at its pointer: in a V1 document the observation's
// kind is `unknown_field`; in V2 a key under another prefix, a column against a list column's
// rule, the two columns the same and a pool of a game's scope are `type_mismatch`; no row, 65
// rows, a factor of 0 or of 1 000 001 are `limit_exceeded`; a row's key twice is `duplicate_id`;
// a key that is empty or holds a control character is `type_mismatch` and one of 1025 bytes
// `limit_exceeded` (the bound of a fact's text); beside `static_snapshot` the pool is
// `limit_exceeded`, as a `ledger_fact` pool without a fact observation is.
#[test]
fn p1_t2_each_refusal_is_its_code_at_its_pointer() {
    use crate::CatalogDiagnosticCode::{DuplicateId, LimitExceeded, TypeMismatch, UnknownField};
    let v1 = example(SCHEDULING_SCHEMA_VERSION, |_, pools| listed(pools, ROW));
    assert!(
        refused_at(&v1, UnknownField, "/pools/0/observation/kind"),
        "a V1 document: {:?}",
        diagnostics(&v1)
    );

    type Edit = fn(&mut Value);
    let at = |member: &str| format!("/pools/0/observation/{member}");
    let cases: [(&str, Edit, CatalogDiagnosticCode, String); 14] = [
        (
            "a key under another prefix",
            |pool| pool["observation"]["fact_key"] = json!("inventory.example_rows"),
            TypeMismatch,
            at("fact_key"),
        ),
        (
            "a key column against the rule",
            |pool| pool["observation"]["key_column"] = json!("Id"),
            TypeMismatch,
            at("key_column"),
        ),
        (
            "a value column against the rule",
            |pool| pool["observation"]["value_column"] = json!("count-a"),
            TypeMismatch,
            at("value_column"),
        ),
        (
            "the two columns the same",
            |pool| pool["observation"]["value_column"] = json!(KEY_COLUMN),
            TypeMismatch,
            at("value_column"),
        ),
        (
            "a pool of a game's scope",
            |pool| pool["scope"] = json!({"kind": "game", "game_id": "fixture-game-a"}),
            TypeMismatch,
            at("kind"),
        ),
        (
            "no row",
            |pool| pool["observation"]["rows"] = json!([]),
            LimitExceeded,
            at("rows"),
        ),
        (
            "65 rows",
            |pool| pool["observation"]["rows"] = numbered_rows(65),
            LimitExceeded,
            at("rows"),
        ),
        (
            "a factor of 0",
            |pool| pool["observation"]["rows"][0]["factor"] = json!(0),
            LimitExceeded,
            at("rows/0/factor"),
        ),
        (
            "a factor of 1 000 001",
            |pool| pool["observation"]["rows"][0]["factor"] = json!(1_000_001),
            LimitExceeded,
            at("rows/0/factor"),
        ),
        (
            "a row's key twice",
            |pool| {
                pool["observation"]["rows"] = json!([
                    {"key": "item_a", "factor": 1},
                    {"key": "item_a", "factor": 2}
                ]);
            },
            DuplicateId,
            at("rows/1/key"),
        ),
        (
            "an empty key",
            |pool| pool["observation"]["rows"][0]["key"] = json!(""),
            TypeMismatch,
            at("rows/0/key"),
        ),
        (
            "a key with a control character",
            |pool| pool["observation"]["rows"][0]["key"] = json!("item\u{7}a"),
            TypeMismatch,
            at("rows/0/key"),
        ),
        (
            "a key of 1025 bytes",
            |pool| pool["observation"]["rows"][0]["key"] = json!("a".repeat(1_025)),
            LimitExceeded,
            at("rows/0/key"),
        ),
        (
            "beside static_snapshot",
            |pool| pool["value_source"] = json!({"kind": "static_snapshot"}),
            LimitExceeded,
            "/pools/0".to_owned(),
        ),
    ];
    for (case, edit, code, path) in cases {
        let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |_, pools| {
            listed(pools, ROW);
            edit(&mut pools["pools"][0]);
        });
        assert!(
            refused_at(&sources, code, &path),
            "{case}: {code:?} at {path}: {:?}",
            diagnostics(&sources)
        );
    }
}

// Test 3, the one exception. A `fact` observation, a capacity reference and a line reference
// under `inventory.list.`, and a `fact` predicate on such a key, are refused `type_mismatch` at
// their key as before (the last three on a pool that is read from rows of a list itself); a
// `list_rows` observation of a key under another prefix is refused there too.
#[test]
fn p1_t3_only_a_list_rows_observation_names_the_list_prefix() {
    type Edit = fn(&mut Value, &mut Value);
    let cases: [(&str, Edit, &str); 5] = [
        (
            "a fact observation",
            |_, pools| {
                pools["pools"][0]["observation"] = json!({"kind": "fact", "fact_key": LIST_KEY});
            },
            "/pools/0/observation/fact_key",
        ),
        (
            "a capacity reference",
            |_, pools| {
                listed(pools, ROW);
                pools["pools"][0]["capacity"] =
                    json!({"type": "fact", "fact_key": LIST_KEY, "offset": 0});
            },
            "/pools/0/capacity/fact_key",
        ),
        (
            "a line reference",
            |_, pools| {
                listed(pools, ROW);
                pools["pools"][0]["stop_line"] =
                    json!({"type": "fact", "fact_key": LIST_KEY, "offset": 0});
                pools["pools"][0]["hard_line"] = json!(0);
            },
            "/pools/0/stop_line/fact_key",
        ),
        (
            "a fact predicate",
            |tasks, pools| {
                listed(pools, ROW);
                tasks["tasks"][0]["trigger"]["predicates"][1]["fact_key"] = json!(LIST_KEY);
            },
            "/tasks/0/trigger/predicates/1/fact_key",
        ),
        (
            "a list_rows observation of another prefix",
            |_, pools| {
                listed(pools, ROW);
                pools["pools"][0]["observation"]["fact_key"] = json!("resource.primary");
            },
            "/pools/0/observation/fact_key",
        ),
    ];
    for (case, edit, path) in cases {
        let sources = example(SCHEDULING_SCHEMA_VERSION_V2, edit);
        assert!(
            refused_at(&sources, CatalogDiagnosticCode::TypeMismatch, path),
            "{case}: {:?}",
            diagnostics(&sources)
        );
    }
}

// Test 4, the composer. A `schedule.json` pool read from rows of a list, with `pool_list_rows` in
// `requires`, composes: the composed pools document carries the observation as written and the
// compiled pool its rows in that order. Without the entry the file is refused, `schedule_invalid`
// at the pool's `observation`. On a Runtime whose manifest does not name the subsystem the file
// that lists it is refused at that entry (`schedule_requires_missing`).
#[test]
fn p1_t4_the_composer_requires_pool_list_rows_and_a_runtime_without_it_refuses_the_file() {
    let rows: [(&str, u64); 2] = [("item_b", 2), ("item_a", 1)];
    let composed = compose(&schedule(&rows, &[POOL_LIST_ROWS_SUBSYSTEM]), None)
        .unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let pools = serde_json::from_slice::<Value>(&composed.sources().pools.bytes)
        .expect("the composed pools document");
    assert_eq!(
        pools["pools"][0]["observation"],
        list_rows(&rows),
        "{pools}"
    );
    assert_eq!(
        pools["pools"][0]["scope"],
        json!({"kind": "instance", "instance_id": COMPOSED_INSTANCE})
    );
    let ObservationRef::ListRows { rows: compiled, .. } =
        &composed.compiled().catalog().pools.pools[0].observation
    else {
        panic!(
            "a list_rows pool: {:?}",
            composed.compiled().catalog().pools
        );
    };
    assert_eq!(
        compiled
            .iter()
            .map(|row| (row.key.as_str(), row.factor))
            .collect::<Vec<_>>(),
        rows
    );

    let failure = compose(&schedule(&rows, &[]), None).expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, "/pools/0/observation");

    let older = BTreeSet::from(["standard_pack_schedule".to_owned()]);
    let failure = compose(&schedule(&rows, &[POOL_LIST_ROWS_SUBSYSTEM]), Some(&older))
        .expect_err("a Runtime that does not name the subsystem");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleRequiresMissing);
    assert_eq!(issue.pointer, "/requires/0");
}

// Test 5, the cost term (ruling 6). A judged task consumes 10 of a valued regenerating pool read
// from rows of a list and produces 5 of a valued pool: its net benefit is unknown,
// `benefit_unknown` `reading_unknown` naming the pool, the list's key and `fact_missing`, and it is
// not dispatched; the same when the input holds the list with a row of each key and the caller
// hands the pool a snapshot of 0, which the fact projection drops and the evaluation does not read.
#[test]
fn p1_t5_the_cost_term_of_a_list_pool_is_unknown_and_never_zero() {
    let catalog = evaluated(
        list_pool(100, json!({"amount": 1, "per_ms": 6 * MINUTE_MS}), false),
        10,
    );
    for (case, facts, pools) in inputs() {
        let evaluation = round(&catalog, facts, pools);
        assert!(
            evaluation.dispatch_intents.is_empty(),
            "{case}: {evaluation:#?}"
        );
        let last = last(decision(&evaluation));
        assert!(last.is(PolicyCode::BenefitUnknown), "{case}: {last:?}");
        assert_eq!(
            last.value("benefit_gap"),
            Some(&json!("reading_unknown")),
            "{case}"
        );
        assert_eq!(last.value("pool_id"), Some(&json!(LISTED)), "{case}");
        assert_eq!(last.value("fact_key"), Some(&json!(LIST_KEY)), "{case}");
        assert_eq!(
            last.value("unknown_reason"),
            Some(&json!("fact_missing")),
            "{case}"
        );
        assert_eq!(
            last.value("benefit_milli"),
            None,
            "{case}: never a known part"
        );
    }
}

// Test 6, the capped state (ruling 6). A judged task consumes 1 of a valued pool without
// regeneration read from rows of a list, which writes a stop line of 800 and a hard line of 100,
// and produces 5 of a valued pool: its benefit is known and positive, and it is held with
// `pool_line_held` `capped_cap_unknown` naming the pool, the list's key and `fact_missing`, not
// dispatched; the same with the list in the input and a caller's snapshot of 0.
#[test]
fn p1_t6_the_capped_state_of_a_list_pool_is_unknown_and_never_zero() {
    let catalog = evaluated(list_pool(1_000, json!({"kind": "none"}), true), 1);
    for (case, facts, pools) in inputs() {
        let evaluation = round(&catalog, facts, pools);
        assert!(
            evaluation.dispatch_intents.is_empty(),
            "{case}: {evaluation:#?}"
        );
        let last = last(decision(&evaluation));
        assert!(last.is(PolicyCode::PoolLineHeld), "{case}: {last:?}");
        assert_eq!(
            last.value("pool_line_hold"),
            Some(&json!("capped_cap_unknown")),
            "{case}"
        );
        assert_eq!(last.value("pool_id"), Some(&json!(LISTED)), "{case}");
        assert_eq!(last.value("fact_key"), Some(&json!(LIST_KEY)), "{case}");
        assert_eq!(
            last.value("unknown_reason"),
            Some(&json!("fact_missing")),
            "{case}"
        );
    }
}

// Test 7, identity (ruling 7). The neutral example catalog, which writes no `list_rows`, compiles
// to its V1 and V2 golden identities, and its pools serialize without the kind.
#[test]
fn p1_t7_a_catalog_without_the_kind_keeps_its_golden_identity() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let compiled = compile(&example(version, |_, _| {}));
        assert_eq!(compiled.catalog_hash(), golden, "{version}");
        let text = serde_json::to_string(&compiled.catalog().pools).expect("the pools document");
        assert!(!text.contains(POOL_LIST_ROWS_KIND), "{version}: {text}");
    }
}

// ---- The compile fixture: the neutral example catalog `catalog-a`

/// A `list_rows` observation of [`LIST_KEY`] over `rows` (key, factor), in this order.
fn list_rows(rows: &[(&str, u64)]) -> Value {
    json!({
        "kind": "list_rows",
        "fact_key": LIST_KEY,
        "key_column": KEY_COLUMN,
        "value_column": VALUE_COLUMN,
        "rows": rows
            .iter()
            .map(|(key, factor)| json!({"key": key, "factor": factor}))
            .collect::<Vec<_>>()
    })
}

/// `count` rows of distinct keys, each of factor 1.
fn numbered_rows(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("item_{index}"), "factor": 1}))
            .collect(),
    )
}

/// Pool 0 of the example read from rows of [`LIST_KEY`], beside `ledger_fact`.
fn listed(pools: &mut Value, rows: &[(&str, u64)]) {
    let pool = &mut pools["pools"][0];
    pool["observation"] = list_rows(rows);
    pool["value_source"] = json!({"kind": "ledger_fact", "minimum_confidence_milli": 500});
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-p1/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed), its tasks and pools documents edited by `edit`.
fn example(version: &str, edit: impl FnOnce(&mut Value, &mut Value)) -> CatalogSources {
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
    let [mut tasks, mut pools, activity, timeline] = documents;
    edit(&mut tasks, &mut pools);
    CatalogSources {
        tasks: source("tasks", &tasks),
        pools: source("pools", &pools),
        activity: source("activity", &activity),
        timeline: source("timeline", &timeline),
        selection: None,
    }
}

fn compile(sources: &CatalogSources) -> CompiledCatalog {
    compile_catalog(sources)
        .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}

/// Whether compiling `sources` is refused with `code` at `path`.
fn refused_at(sources: &CatalogSources, code: CatalogDiagnosticCode, path: &str) -> bool {
    compile_catalog(sources).err().is_some_and(|failure| {
        failure
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
    })
}

/// The diagnostics of compiling `sources`, for a failed assertion's message.
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

// ---- The composer fixture of test 4

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
/// The one instance the composer binds.
const COMPOSED_INSTANCE: &str = "instance-a";
const RULE: &str = "rule.plain";

/// A `schedule.json` rule running the package of its own id, on an hourly clock.
fn schedule_rule() -> Value {
    let mut rule = task(RULE, Vec::new(), Vec::new());
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(RULE));
    rule
}

/// `schedule.json` with one rule and one pool read from rows of [`LIST_KEY`] (no scope: the
/// composer fills it), naming `requires`.
fn schedule(rows: &[(&str, u64)], requires: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": [schedule_rule()],
        "pools": [{
            "id": "pool.listed",
            "capacity": 1_000_000,
            "projection": {"kind": "none"},
            "observation": list_rows(rows),
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": null
        }],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for [`COMPOSED_INSTANCE`], whose game root registers the rule's package;
/// with `subsystems`, a Runtime's manifest checks `requires`.
fn compose(
    schedule: &Value,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": RULE,
            "server": SERVER,
            "entry_task_id": "task",
            "path": format!("packs/{RULE}"),
            "file_count": 1,
            "byte_count": 64
        }]
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
    let schedule = serde_json::to_vec(schedule).expect("schedule bytes");
    // Workflow #455 G-R2s: every package of the root reads, none irreversible.
    let irreversible = super::readable_root(&root);
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
        },
    )]);
    let instances = BTreeMap::from([(
        COMPOSED_INSTANCE.to_owned(),
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

// ---- The evaluation fixture of tests 5 and 6: one instance, a judged task on an hourly clock due
// at NOW, a valued pool it produces and a valued pool read from rows of a list it consumes

const A: &str = "instance-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// Valued 1000 milli a unit, no regeneration, read from its own fact, which no input holds.
const VALUED: &str = "pool.valued";
/// Valued 1000 milli a unit, read from rows of [`LIST_KEY`].
const LISTED: &str = "pool.listed";
const TASK: &str = "task_judged";

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A task of the game on an hourly clock with these effects.
fn task(id: &str, produces: Vec<Value>, consumes: Vec<Value>) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
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
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": consumes,
        "produces": produces,
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
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

fn valuation() -> Value {
    json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000})
}

fn valued_pool() -> Value {
    json!({
        "id": VALUED,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": "resource.valued"},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": valuation()
    })
}

/// [`LISTED`] of `capacity` and `projection`, read from the rows `item_a` (factor 200) and `item_b`
/// (factor 400) of [`LIST_KEY`]; with `lines`, a stop line of 800 and a hard line of 100.
fn list_pool(capacity: u64, projection: Value, lines: bool) -> Value {
    let mut pool = json!({
        "id": LISTED,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": projection,
        "observation": list_rows(&[("item_a", 200), ("item_b", 400)]),
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": valuation()
    });
    if lines {
        pool["stop_line"] = json!(800);
        pool["hard_line"] = json!(100);
    }
    pool
}

/// The V2 catalog of [`valued_pool`], `listed` and one judged task that produces 5 of [`VALUED`]
/// and consumes `consumed` of [`LISTED`].
fn evaluated(listed: Value, consumed: u64) -> CompiledCatalog {
    let mut judged = task(
        TASK,
        vec![effect(VALUED, "produce", 5)],
        vec![effect(LISTED, "consume", consumed)],
    );
    judged["benefit_judged"] = json!(true);
    let header = json!({
        "catalog_id": "fixture.455-p1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-p1"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let profile = json!({
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
    });
    compile(&CatalogSources {
        tasks: source("tasks", &document("tasks", json!([judged]))),
        pools: source("pools", &document("pools", json!([valued_pool(), listed]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
}

/// One input of tests 5 and 6: its name, the facts and the caller's pool snapshots.
type Input = (&'static str, Vec<ObservedFact>, Vec<PoolValueSnapshot>);

/// The two inputs of tests 5 and 6: nothing, and the list itself, read a minute ago with a row
/// of each key, beside a caller's snapshot of 0 for [`LISTED`].
fn inputs() -> [Input; 2] {
    let row = |key: &str, count: i64| {
        BTreeMap::from([
            (KEY_COLUMN.to_owned(), FactScalar::String(key.to_owned())),
            (VALUE_COLUMN.to_owned(), FactScalar::Integer(count)),
        ])
    };
    let list = ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: LIST_KEY.to_owned(),
        value: FactValue::RecordList(vec![row("item_a", 3), row("item_b", 2)]),
        observed_at_unix_ms: NOW - MINUTE_MS,
        expires_at_unix_ms: Some(NOW + HOUR_MS),
        confidence_milli: 1_000,
    };
    let zero = PoolValueSnapshot {
        pool_id: LISTED.to_owned(),
        value: 0,
        observed_at_unix_ms: NOW - MINUTE_MS,
    };
    [
        ("no list", Vec::new(), Vec::new()),
        ("the list and a snapshot of 0", vec![list], vec![zero]),
    ]
}

/// One round at NOW over `facts`, handing the evaluation `pools` as the caller's snapshots, every
/// candidate admitted. The fact projection drops every snapshot of [`LISTED`].
fn round(
    catalog: &CompiledCatalog,
    facts: Vec<ObservedFact>,
    pools: Vec<PoolValueSnapshot>,
) -> PolicyEvaluation {
    let instance = instance(A, &[]);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-p1".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance.clone()],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools,
        hosts: vec![HostResourceSnapshot {
            host_id: instance.host_id,
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
            heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
        }],
    };
    assert!(
        project_fact_pools(catalog, &facts, &resources)
            .pools
            .iter()
            .all(|snapshot| snapshot.pool_id != LISTED),
        "the fact projection gives the list pool no value"
    );
    evaluate_with_eligibility(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
        11,
        |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
            Ok(CandidateEligibility::Eligible)
        },
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

fn decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == TASK && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {TASK} on {A}: {evaluation:#?}"))
}

fn last(decision: &TaskDecision) -> &Outcome {
    &decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
        .outcome
}
