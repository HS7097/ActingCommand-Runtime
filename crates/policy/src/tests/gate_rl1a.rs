// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1a (frozen model v1.0 §3.1, §3.2, §3.6, §6.1 row RL-1a, §6.3, §7.1 A1a;
// contracts/standard-pack-schedule.md): the row's seven tests of `schedule.json` v1 and the pure
// composer `compose_standard_pack_rules`, with neutral fixtures only. Every expected value comes
// from the model row, not from what the code does today.
//
// T1 starts from the neutral catalog `contracts/scheduling/examples/catalog-a` brought into the
// as-is shape the model measured for today's catalogs (§2.2: V2, every scope the instance's
// own, the Runtime's package operation, a `package.` procedure, no instance override). The test
// converts it by the as-is rules of §6.3 into a `schedule.json` and composes it back with one
// bound instance.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{
    BundleIndex, STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes,
    read_standard_pack_root,
};
use serde_json::{Map, Value, json};

use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, ComposedProcedure, EligibilityState,
    EvaluationFacts, EvaluationResources, EvaluationTime, HostResourceSnapshot, InstanceSnapshot,
    ObservedFact, PoolValueSnapshot, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState,
    StandardPackBinding, StandardPackComposeFailure, StandardPackComposeInput,
    StandardPackComposition, StandardPackGameFiles, TaskRuntimeSnapshot, TaskTerminalState,
    compile_catalog, compose_standard_pack_rules, evaluate,
};

const GAME_A: &str = "game-a";
const GAME_B: &str = "game-b";
const SERVER_A: &str = "server-a";
const SERVER_B: &str = "server-b";
const INSTANCE_A: &str = "fixture-instance-a";
const INSTANCE_B: &str = "fixture-instance-b";
// catalog-a's task, pool, first profile and event ids.
const RULE: &str = "fixture.observe";
const POOL: &str = "fixture-pool-a";
const PROFILE: &str = "fixture-activity-a";
const GAME_PROFILE: &str = "fixture-activity-game";
const EVENT: &str = "fixture-reset-a";
// The packages of the fixture game roots.
const PACKAGE: &str = "fixture.observe";
const RETURN_PACKAGE: &str = "fixture.return";
const EXTRA_PACKAGE: &str = "fixture.extra";
const OTHER_PACKAGE: &str = "fixture.other";
const ABSENT_PACKAGE: &str = "fixture.absent";
const SECOND_RULE: &str = "fixture.second";
const HELD_REASON: &str = "fixture: kept out of the routine";
const REQUIRED_SUBSYSTEM: &str = "condition_edges";
const PRESENT_SUBSYSTEM: &str = "standard_pack_schedule";
const PACKAGE_OPERATION: &str = "operation.package.run";

const SEED: u64 = 17;
const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 86_400_000;
// The decision grid of T1: 7 days from midnight UTC of day 400, at 5-minute steps.
const GRID_START: u64 = 400 * DAY_MS;
const GRID_STEP_MS: u64 = 5 * MINUTE_MS;
const GRID_DAYS: u64 = 7;

// Every word the fixtures' strings may hold: a closed, neutral list (T7).
const NEUTRAL_WORDS: &[&str] = &[
    "a",
    "absent",
    "actingcommand",
    "active",
    "activity",
    "after",
    "any",
    "approval",
    "b",
    "boolean",
    "bundle",
    "catalog",
    "clock",
    "completed",
    "condition",
    "continue",
    "daily",
    "edges",
    "eq",
    "etc",
    "extra",
    "fact",
    "fixture",
    "game",
    "goal",
    "grid",
    "home",
    "host",
    "instance",
    "integer",
    "interval",
    "kept",
    "ledger",
    "less",
    "local",
    "observation",
    "observe",
    "of",
    "operation",
    "other",
    "out",
    "outcome",
    "pack",
    "package",
    "packs",
    "pool",
    "primary",
    "procedure",
    "produce",
    "reset",
    "resource",
    "return",
    "round",
    "routine",
    "run",
    "same",
    "scan",
    "schedule",
    "scheduling",
    "second",
    "server",
    "session",
    "snapshot",
    "stable",
    "standard",
    "succeeded",
    "task",
    "than",
    "the",
    "timeline",
    "utc",
    "v1",
    "v2",
    "v4",
    "verified",
    "weighted",
];

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// One fixture game root: its id, its `schedule.json` (none when the root holds none) and its
/// `bundle.json`.
struct GameRoot {
    game: &'static str,
    schedule: Option<Value>,
    bundle: Value,
}

/// A bundle index v4 of `packs` (package id, server) with `maintenance` (package id, server, use).
fn bundle(game: &str, packs: &[(&str, &str)], maintenance: &[(&str, &str, &str)]) -> Value {
    let mut bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": game,
        "packs": packs
            .iter()
            .map(|(package_id, server)| json!({
                "package_id": package_id,
                "server": server,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>()
    });
    if !maintenance.is_empty() {
        bundle["maintenance"] = Value::Array(
            maintenance
                .iter()
                .map(|(package_id, server, used)| {
                    json!({"package_id": package_id, "server": server, "uses": [used]})
                })
                .collect(),
        );
    }
    bundle
}

/// Game A's root: the rule's package, a return-home package and one package nothing names.
fn bundle_a() -> Value {
    bundle(
        GAME_A,
        &[
            (PACKAGE, SERVER_A),
            (RETURN_PACKAGE, SERVER_A),
            (EXTRA_PACKAGE, SERVER_A),
        ],
        &[(RETURN_PACKAGE, SERVER_A, "return_home")],
    )
}

/// Game A's root with its return-home package alone.
fn bundle_maintenance_only() -> Value {
    bundle(
        GAME_A,
        &[(RETURN_PACKAGE, SERVER_A)],
        &[(RETURN_PACKAGE, SERVER_A, "return_home")],
    )
}

/// Game B's root on server B, with `packs`.
fn bundle_b(packs: &[&str]) -> Value {
    let packs = packs
        .iter()
        .map(|package_id| (*package_id, SERVER_B))
        .collect::<Vec<_>>();
    bundle(GAME_B, &packs, &[])
}

fn game_a(schedule: Option<Value>) -> GameRoot {
    GameRoot {
        game: GAME_A,
        schedule,
        bundle: bundle_a(),
    }
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture JSON")
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("fixture bytes")
}

fn instance_scope(alias: &str) -> Value {
    json!({"kind": "instance", "instance_id": alias})
}

/// catalog-a's four documents, each with the member that lists its elements.
fn catalog_a() -> [(&'static str, Value); 4] {
    [
        (
            "tasks",
            parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
            )),
        ),
        (
            "pools",
            parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/pools.json"
            )),
        ),
        (
            "profiles",
            parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/activity.json"
            )),
        ),
        (
            "events",
            parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
            )),
        ),
    ]
}

/// Sets every `scope` member at any depth of `value` to instance `alias`.
fn instance_scopes(value: &mut Value, alias: &str) {
    match value {
        Value::Object(members) => {
            for (name, member) in members.iter_mut() {
                if name == "scope" {
                    *member = instance_scope(alias);
                } else {
                    instance_scopes(member, alias);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                instance_scopes(item, alias);
            }
        }
        _ => {}
    }
}

/// catalog-a in the as-is shape of today's catalogs (§2.2): V2, so each event declares its
/// validity (here from the epoch, open-ended); every scope instance A (the game-scoped profile
/// and the server-scoped event included); each task runs the Runtime's package operation through
/// the procedure `package.<package>` and has no instance override.
fn catalog_a_as_is() -> [(&'static str, Value); 4] {
    let mut documents = catalog_a();
    for (list, document) in &mut documents {
        document["schema_version"] = json!(SCHEDULING_SCHEMA_VERSION_V2);
        let items = document[*list].as_array_mut().expect("an element list");
        for item in items.iter_mut() {
            instance_scopes(item, INSTANCE_A);
            match *list {
                "tasks" => {
                    item["entrypoint"] = json!({"operation_id": PACKAGE_OPERATION});
                    item["procedure_ref"] = json!(format!("package.{PACKAGE}"));
                    item["instance_overrides"] = json!([]);
                }
                "events" => {
                    item["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
                }
                _ => {}
            }
        }
    }
    documents
}

/// Removes every `scope` member at any depth of `value`; each must be `own` (§6.3 rules 2, 6).
fn drop_scopes(value: &mut Value, own: &Value, pointer: &str) {
    match value {
        Value::Object(members) => {
            if let Some(scope) = members.shift_remove("scope") {
                assert_eq!(
                    &scope, own,
                    "§6.3 rule 2: the scope at {pointer} is the instance's own"
                );
            }
            for (name, member) in members.iter_mut() {
                drop_scopes(member, own, &format!("{pointer}/{name}"));
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                drop_scopes(item, own, &format!("{pointer}/{index}"));
            }
        }
        _ => {}
    }
}

/// The as-is rules of §6.3, applied to the V2 documents of the one instance `alias` bound to the
/// game root of `bundle`; each rule fails loud.
fn convert_as_is(documents: &[(&'static str, Value); 4], alias: &str, bundle: &Value) -> Value {
    let own = instance_scope(alias);
    let index: BundleIndex = serde_json::from_value(bundle.clone()).expect("a bundle index");
    let packages = index
        .packs
        .iter()
        .map(|pack| pack.package_id.clone())
        .collect::<BTreeSet<_>>();
    let maintenance = index
        .maintenance()
        .iter()
        .map(|entry| entry.package_id.clone())
        .collect::<BTreeSet<_>>();
    let mut schedule = Map::new();
    schedule.insert(
        "schema_version".to_owned(),
        json!("actingcommand.schedule.v1"),
    );
    schedule.insert("requires".to_owned(), json!([]));
    let mut named = BTreeSet::new();
    for (list, document) in documents {
        let member = match *list {
            "tasks" => "rules",
            "pools" => "pools",
            "profiles" => "activity",
            _ => "timeline",
        };
        let mut converted = Vec::new();
        let items = document[*list].as_array().expect("an element list");
        for (position, item) in items.iter().enumerate() {
            let pointer = format!("/{list}/{position}");
            let mut item = item.clone();
            // Rules 1, 2 and 6: the element goes to the game of its instance and loses every
            // scope, each the instance's own.
            drop_scopes(&mut item, &own, &pointer);
            if *list == "tasks" {
                let fields = item.as_object_mut().expect("a task object");
                // Rule 3: the entrypoint goes; it is the Runtime's package operation.
                assert_eq!(
                    fields.shift_remove("entrypoint"),
                    Some(json!({"operation_id": PACKAGE_OPERATION})),
                    "§6.3 rule 3 at {pointer}"
                );
                // Rule 4: the procedure becomes the package of the bundle it names.
                let package_id = fields
                    .shift_remove("procedure_ref")
                    .as_ref()
                    .and_then(Value::as_str)
                    .and_then(|procedure| procedure.strip_prefix("package."))
                    .filter(|package_id| packages.contains(*package_id))
                    .unwrap_or_else(|| panic!("§6.3 rule 4 at {pointer}"))
                    .to_owned();
                // Rule 5: the instance overrides go; they are empty.
                assert_eq!(
                    fields.shift_remove("instance_overrides"),
                    Some(json!([])),
                    "§6.3 rule 5 at {pointer}"
                );
                named.insert(package_id.clone());
                fields.insert("package_id".to_owned(), Value::String(package_id));
            }
            converted.push(item);
        }
        schedule.insert(member.to_owned(), Value::Array(converted));
    }
    // Rule 7: held are the packages neither maintenance-role nor named by a rule.
    let held = packages
        .iter()
        .filter(|package_id| !maintenance.contains(*package_id) && !named.contains(*package_id))
        .map(|package_id| json!({"package_id": package_id, "reason": HELD_REASON}))
        .collect::<Vec<_>>();
    schedule.insert("held".to_owned(), Value::Array(held));
    Value::Object(schedule)
}

/// catalog-a's `schedule.json`: T1's conversion for instance A on game A.
fn schedule_a() -> Value {
    convert_as_is(&catalog_a_as_is(), INSTANCE_A, &bundle_a())
}

/// [`schedule_a`] whose rule's trigger also reads the event (`timeline_active`), so that every
/// reference kind a copy rewrites appears: the outcome's task, the effect's and the goal's pool,
/// the event, and the fact's scope.
fn schedule_with_references() -> Value {
    let mut schedule = schedule_a();
    schedule["rules"][0]["trigger"]["predicates"]
        .as_array_mut()
        .expect("the trigger's predicates")
        .push(json!({"kind": "timeline_active", "event_id": EVENT}));
    schedule
}

/// Game B's `schedule.json`: one rule `id` with catalog-a's members, running `package_id`.
fn schedule_b(id: &str, package_id: &str) -> Value {
    let mut rule = schedule_a()["rules"][0].clone();
    rule["id"] = json!(id);
    rule["package_id"] = json!(package_id);
    json!({"schema_version": "actingcommand.schedule.v1", "rules": [rule]})
}

/// The evaluation facts of the grid: per instance of game A on server A, its snapshot (its own
/// host, the package operation) and one reading of `resource.primary`.
fn grid_facts(instances: &[&str]) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-grid".to_owned(),
        facts: instances
            .iter()
            .map(|alias| {
                serde_json::from_value::<ObservedFact>(json!({
                    "scope": instance_scope(alias),
                    "fact_key": "resource.primary",
                    "value": {"type": "integer", "value": 12},
                    "observed_at_unix_ms": GRID_START
                }))
                .expect("a fixture fact")
            })
            .collect(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: instances
            .iter()
            .map(|alias| InstanceSnapshot {
                instance_id: (*alias).to_owned(),
                server_id: SERVER_A.to_owned(),
                game_id: GAME_A.to_owned(),
                host_id: format!("host.{alias}"),
                available: true,
                capability_operation_ids: vec![PACKAGE_OPERATION.to_owned()],
                preferred_task_ids: Vec::new(),
            })
            .collect(),
        priority_offsets: Vec::new(),
    }
}

/// T1's facts snapshot: [`grid_facts`] of instance A, whose rule ran once and succeeded at the
/// grid's start. The grid then holds steps that do not run (the rule's cooldown at the start;
/// in its first hour, after the reading turns stale, neither its clock nor its fact is due) and
/// steps that run (its reading still fresh, and every hour's clock after the first).
fn grid_facts_after_a_run() -> EvaluationFacts {
    let mut facts = grid_facts(&[INSTANCE_A]);
    facts.tasks.push(TaskRuntimeSnapshot {
        task_id: RULE.to_owned(),
        instance_id: INSTANCE_A.to_owned(),
        last_dispatched_unix_ms: Some(GRID_START),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    });
    facts
}

fn grid_resources(instances: &[&str]) -> EvaluationResources {
    EvaluationResources {
        pools: vec![PoolValueSnapshot {
            pool_id: POOL.to_owned(),
            value: 40,
            observed_at_unix_ms: GRID_START,
        }],
        hosts: instances
            .iter()
            .map(|alias| HostResourceSnapshot {
                host_id: format!("host.{alias}"),
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
                heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
            })
            .collect(),
    }
}

/// Every fixture value the tests build (T7).
fn fixtures() -> Vec<(&'static str, Value)> {
    let mut fixtures = catalog_a()
        .into_iter()
        .map(|(_, document)| ("catalog-a", document))
        .collect::<Vec<_>>();
    fixtures.extend(
        catalog_a_as_is()
            .into_iter()
            .map(|(_, document)| ("catalog-a as-is", document)),
    );
    fixtures.extend([
        ("schedule A", schedule_a()),
        ("schedule with references", schedule_with_references()),
        ("schedule B", schedule_b(RULE, OTHER_PACKAGE)),
        ("schedule B second", schedule_b(SECOND_RULE, PACKAGE)),
        ("bundle A", bundle_a()),
        ("bundle with maintenance only", bundle_maintenance_only()),
        ("bundle B", bundle_b(&[OTHER_PACKAGE, PACKAGE])),
        (
            "grid facts",
            serde_json::to_value(grid_facts(&[INSTANCE_A, INSTANCE_B])).expect("facts JSON"),
        ),
        (
            "grid facts after a run",
            serde_json::to_value(grid_facts_after_a_run()).expect("facts JSON"),
        ),
        (
            "grid resources",
            serde_json::to_value(grid_resources(&[INSTANCE_A, INSTANCE_B]))
                .expect("resources JSON"),
        ),
        (
            "test strings",
            json!([
                GAME_A,
                GAME_B,
                SERVER_A,
                SERVER_B,
                INSTANCE_A,
                INSTANCE_B,
                RULE,
                POOL,
                PROFILE,
                GAME_PROFILE,
                EVENT,
                PACKAGE,
                RETURN_PACKAGE,
                EXTRA_PACKAGE,
                OTHER_PACKAGE,
                ABSENT_PACKAGE,
                SECOND_RULE,
                HELD_REASON,
                REQUIRED_SUBSYSTEM,
                PRESENT_SUBSYSTEM,
                PACKAGE_OPERATION,
                instance_scope(INSTANCE_B),
                {"kind": "server", "server_id": SERVER_A},
            ]),
        ),
    ]);
    fixtures
}

// ---------------------------------------------------------------------------------------------
// Composing
// ---------------------------------------------------------------------------------------------

/// Composes `roots` for the bound `instances` (alias, game, server), at catalog version 1.
fn compose(
    roots: &[GameRoot],
    instances: &[(&str, &str, &str)],
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    let read = roots
        .iter()
        .map(|fixture| {
            let bundle = bytes(&fixture.bundle);
            let root = read_standard_pack_root(
                StandardPackRootBytes {
                    bundle: Some(bundle.as_slice()),
                    ..StandardPackRootBytes::default()
                },
                fixture.game,
            )
            .unwrap_or_else(|issue| panic!("the root of {} reads: {issue}", fixture.game))
            .expect("the root holds bundle.json");
            let maintenance = serde_json::from_slice::<BundleIndex>(&bundle)
                .expect("a bundle index")
                .maintenance()
                .to_vec();
            (
                fixture.game,
                fixture.schedule.as_ref().map(bytes),
                root,
                maintenance,
            )
        })
        .collect::<Vec<_>>();
    let games = read
        .iter()
        .filter_map(|(game, schedule, root, maintenance)| {
            schedule.as_ref().map(|schedule| {
                (
                    (*game).to_owned(),
                    StandardPackGameFiles {
                        schedule: schedule.as_slice(),
                        root,
                        maintenance: maintenance.as_slice(),
                    },
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let instances = instances
        .iter()
        .map(|(alias, game, server)| {
            (
                (*alias).to_owned(),
                StandardPackBinding {
                    game_id: (*game).to_owned(),
                    server_id: (*server).to_owned(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems,
        catalog_version: 1,
    })
}

fn composed(
    roots: &[GameRoot],
    instances: &[(&str, &str, &str)],
    subsystems: Option<&BTreeSet<String>>,
) -> StandardPackComposition {
    compose(roots, instances, subsystems)
        .unwrap_or_else(|failure| panic!("the fixture composes: {failure}"))
}

/// The one refusal of a composition that must fail.
fn refused(
    roots: &[GameRoot],
    instances: &[(&str, &str, &str)],
    subsystems: Option<&BTreeSet<String>>,
) -> (Option<String>, StandardPackProblem, String) {
    let failure = match compose(roots, instances, subsystems) {
        Ok(_) => panic!("the fixture is refused"),
        Err(failure) => failure,
    };
    assert_eq!(failure.issues().len(), 1, "one refusal: {failure}");
    let issue = &failure.issues()[0];
    assert_eq!(issue.issue.file, STANDARD_PACK_SCHEDULE_FILE);
    (
        issue.game_id.clone(),
        issue.issue.problem,
        issue.issue.pointer.clone(),
    )
}

/// The composition's four documents, each with the member that lists its elements.
fn composed_documents(composition: &StandardPackComposition) -> [(&'static str, Value); 4] {
    let sources = composition.sources();
    let document = |source: &CatalogDocumentSource| {
        serde_json::from_slice::<Value>(&source.bytes).expect("a composed document")
    };
    [
        ("tasks", document(&sources.tasks)),
        ("pools", document(&sources.pools)),
        ("profiles", document(&sources.activity)),
        ("events", document(&sources.timeline)),
    ]
}

/// The ids of the elements of one composed document.
fn ids(document: &(&'static str, Value)) -> Vec<String> {
    document.1[document.0]
        .as_array()
        .expect("an element list")
        .iter()
        .map(|element| element["id"].as_str().expect("an id").to_owned())
        .collect()
}

/// The element `id` of one composed document.
fn element<'a>(document: &'a (&'static str, Value), id: &str) -> &'a Value {
    document.1[document.0]
        .as_array()
        .expect("an element list")
        .iter()
        .find(|element| element["id"] == id)
        .unwrap_or_else(|| panic!("{} holds {id}", document.0))
}

fn copy_id(id: &str, alias: &str) -> String {
    format!("{id}@{alias}")
}

fn compiled_as_is(documents: &[(&'static str, Value); 4]) -> CompiledCatalog {
    let source = |index: usize| {
        CatalogDocumentSource::new(
            format!("memory://fixture-rl1a/{}.json", documents[index].0),
            bytes(&documents[index].1),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source(0),
        pools: source(1),
        activity: source(2),
        timeline: source(3),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the as-is catalog compiles: {:?}", failure.diagnostics()))
}

// ---------------------------------------------------------------------------------------------
// Deciding
// ---------------------------------------------------------------------------------------------

/// One decision as the model row compares it: task, instance, state, reason codes, next wake.
#[derive(Debug, PartialEq)]
struct Decided {
    task_id: String,
    instance_id: Option<String>,
    eligibility: EligibilityState,
    state: SchedulingDecisionState,
    reasons: Vec<String>,
    next_wake_unix_ms: Option<u64>,
}

/// One evaluation as the model row compares it; the decision ids, which hash the catalog, are
/// left out.
#[derive(Debug, PartialEq)]
struct Evaluated {
    decisions: Vec<Decided>,
    next_wake_unix_ms: Option<u64>,
    dispatched: Vec<(String, String, String, String)>,
}

fn evaluated(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    now: u64,
) -> Result<Evaluated, String> {
    let time = EvaluationTime {
        unix_ms: now,
        monotonic_ms: now,
    };
    let evaluation = evaluate(catalog, facts, resources, time, SEED)
        .map_err(|error| error.code().as_str().to_owned())?;
    Ok(Evaluated {
        decisions: evaluation
            .decisions
            .iter()
            .map(|decision| Decided {
                task_id: decision.task_id.clone(),
                instance_id: decision.instance_id.clone(),
                eligibility: decision.eligibility,
                state: decision.state,
                reasons: decision
                    .reasons
                    .iter()
                    .map(|reason| reason.outcome.code().as_str().to_owned())
                    .collect(),
                next_wake_unix_ms: decision.next_wake_unix_ms,
            })
            .collect(),
        next_wake_unix_ms: evaluation.next_wake_unix_ms,
        dispatched: evaluation
            .dispatch_intents
            .iter()
            .map(|intent| {
                (
                    intent.task_id.clone(),
                    intent.instance_id.clone(),
                    intent.operation_id.clone(),
                    intent.procedure_ref.clone(),
                )
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

/// T1 (§7.1 A1a): catalog-a in the as-is shape, converted by the as-is rules, composes back to
/// its documents element by element apart from the descriptor, and both decide alike on one
/// facts snapshot over a 7-day grid at 5-minute steps.
#[test]
fn rl1a_t1_catalog_a_converted_by_the_as_is_rules_composes_back_and_decides_alike() {
    let as_is = catalog_a_as_is();
    let schedule = convert_as_is(&as_is, INSTANCE_A, &bundle_a());
    let composition = composed(
        &[game_a(Some(schedule))],
        &[(INSTANCE_A, GAME_A, SERVER_A)],
        Some(&BTreeSet::new()),
    );
    let documents = composed_documents(&composition);
    for ((list, original), (composed_list, document)) in as_is.iter().zip(&documents) {
        assert_eq!(list, composed_list);
        assert_eq!(
            document["catalog"],
            json!({"catalog_id": "standard-packs", "catalog_version": 1, "approval_refs": []}),
            "{list}: the descriptor of every composition (§3.2 step 4)"
        );
        let original = original[*list].as_array().expect("the original elements");
        let document = document[*list].as_array().expect("the composed elements");
        assert_eq!(document.len(), original.len(), "{list}: as many elements");
        for (index, (left, right)) in document.iter().zip(original).enumerate() {
            assert_eq!(left, right, "{list}/{index} composes back to the original");
        }
    }
    let original = compiled_as_is(&as_is);
    let (left, right) = (composition.compiled().catalog(), original.catalog());
    assert_eq!(left.tasks.tasks, right.tasks.tasks);
    assert_eq!(left.pools.pools, right.pools.pools);
    assert_eq!(left.activity.profiles, right.activity.profiles);
    assert_eq!(left.timeline.events, right.timeline.events);
    assert_eq!(
        composition.manifest(),
        [ComposedProcedure {
            procedure_ref: format!("package.{PACKAGE}"),
            game_id: GAME_A.to_owned(),
            package_id: PACKAGE.to_owned(),
            directory: PACKAGE.to_owned(),
        }]
    );

    let facts = grid_facts_after_a_run();
    let resources = grid_resources(&[INSTANCE_A]);
    let (mut runs, mut waits) = (0, 0);
    for step in 0..GRID_DAYS * DAY_MS / GRID_STEP_MS {
        let now = GRID_START + step * GRID_STEP_MS;
        let from_packs = evaluated(composition.compiled(), &facts, &resources, now);
        let from_documents = evaluated(&original, &facts, &resources, now);
        assert_eq!(
            from_packs, from_documents,
            "at {now} both catalogs decide alike"
        );
        match from_packs {
            Ok(evaluation) if !evaluation.dispatched.is_empty() => runs += 1,
            Ok(_) => waits += 1,
            Err(code) => panic!("at {now} the grid evaluates: {code}"),
        }
    }
    assert!(
        runs > 0 && waits > 0,
        "the grid holds steps that run and steps that do not ({runs}, {waits})"
    );
}

/// T2 (§3.2 step 3): with one instance per (game, server) the ids are the file's; with two, each
/// copy's ids are `<id>@<alias>`, its references are rewritten, and its rule continues `<id>`.
#[test]
fn rl1a_t2_one_instance_keeps_the_ids_and_two_suffix_them_and_continue_the_id() {
    let one = composed(
        &[game_a(Some(schedule_with_references()))],
        &[(INSTANCE_A, GAME_A, SERVER_A)],
        None,
    );
    let documents = composed_documents(&one);
    assert_eq!(ids(&documents[0]), [RULE]);
    assert_eq!(ids(&documents[1]), [POOL]);
    assert_eq!(ids(&documents[2]), [PROFILE, GAME_PROFILE]);
    assert_eq!(ids(&documents[3]), [EVENT]);
    let task = element(&documents[0], RULE);
    assert!(
        task.get("continues_from").is_none(),
        "one copy continues nothing"
    );
    assert_eq!(task["feedback_stop"]["task_id"], RULE);
    assert_eq!(task["produces"][0]["pool_id"], POOL);
    assert_eq!(task["trigger"]["predicates"][2]["event_id"], EVENT);

    let two = composed(
        &[game_a(Some(schedule_with_references()))],
        &[
            (INSTANCE_A, GAME_A, SERVER_A),
            (INSTANCE_B, GAME_A, SERVER_A),
        ],
        None,
    );
    let documents = composed_documents(&two);
    let copies = |id: &str| [copy_id(id, INSTANCE_A), copy_id(id, INSTANCE_B)];
    assert_eq!(ids(&documents[0]), copies(RULE));
    assert_eq!(ids(&documents[1]), copies(POOL));
    assert_eq!(
        ids(&documents[2]),
        [
            copy_id(PROFILE, INSTANCE_A),
            copy_id(GAME_PROFILE, INSTANCE_A),
            copy_id(PROFILE, INSTANCE_B),
            copy_id(GAME_PROFILE, INSTANCE_B),
        ]
    );
    assert_eq!(ids(&documents[3]), copies(EVENT));
    for alias in [INSTANCE_A, INSTANCE_B] {
        let task = element(&documents[0], &copy_id(RULE, alias));
        assert_eq!(task["scope"], instance_scope(alias));
        assert_eq!(task["continues_from"], json!([RULE]));
        assert_eq!(task["feedback_stop"]["task_id"], copy_id(RULE, alias));
        assert_eq!(task["produces"][0]["pool_id"], copy_id(POOL, alias));
        assert_eq!(
            task["trigger"]["predicates"][1]["scope"],
            instance_scope(alias)
        );
        assert_eq!(
            task["trigger"]["predicates"][2]["event_id"],
            copy_id(EVENT, alias)
        );
        let pool = element(&documents[1], &copy_id(POOL, alias));
        assert_eq!(pool["scope"], instance_scope(alias));
        let profile = element(&documents[2], &copy_id(PROFILE, alias));
        assert_eq!(profile["scope"], instance_scope(alias));
        assert_eq!(
            profile["goals"][0]["metric"]["pool_id"],
            copy_id(POOL, alias)
        );
        let event = element(&documents[3], &copy_id(EVENT, alias));
        assert_eq!(event["scope"], instance_scope(alias));
        assert_eq!(
            two.compiled().accounting_ids(&copy_id(RULE, alias)),
            [copy_id(RULE, alias).as_str(), RULE],
            "each copy accounts as the file's id too"
        );
    }
    assert_eq!(two.manifest().len(), 1, "both copies run the one package");
}

/// T3 (§3.1, "No instance names"): a scope at any depth of a rule or an item, or an instance
/// override, is refused as `schedule_invalid` with its pointer.
#[test]
fn rl1a_t3_a_scope_or_an_instance_override_in_the_file_is_refused_with_its_pointer() {
    let mut rule_scope = schedule_a();
    rule_scope["rules"][0]["scope"] = instance_scope(INSTANCE_A);
    let mut predicate_scope = schedule_a();
    predicate_scope["rules"][0]["trigger"]["predicates"][1]["scope"] = instance_scope(INSTANCE_A);
    let mut event_scope = schedule_a();
    event_scope["timeline"][0]["scope"] = json!({"kind": "server", "server_id": SERVER_A});
    let mut overrides = schedule_a();
    overrides["rules"][0]["instance_overrides"] = json!([{
        "instance_id": INSTANCE_B,
        "enabled": true,
        "priority": 120,
        "strategic_weight_milli": 1100,
        "load_profile": null
    }]);
    for (schedule, pointer) in [
        (rule_scope, "/rules/0/scope"),
        (predicate_scope, "/rules/0/trigger/predicates/1/scope"),
        (event_scope, "/timeline/0/scope"),
        (overrides, "/rules/0/instance_overrides"),
    ] {
        assert_eq!(
            refused(
                &[game_a(Some(schedule))],
                &[(INSTANCE_A, GAME_A, SERVER_A)],
                None
            ),
            (
                Some(GAME_A.to_owned()),
                StandardPackProblem::ScheduleInvalid,
                pointer.to_owned()
            ),
            "{pointer}"
        );
    }
}

/// T4 (§3.6): a package the registry does not hold, an id two games compose, a package two
/// games' rules name, and a `requires` the Runtime does not name are refused with their problem.
#[test]
fn rl1a_t4_an_unregistered_package_an_id_clash_and_a_missing_requirement_are_refused() {
    let both = [
        (INSTANCE_A, GAME_A, SERVER_A),
        (INSTANCE_B, GAME_B, SERVER_B),
    ];
    let mut unregistered = schedule_a();
    unregistered["rules"][0]["package_id"] = json!(ABSENT_PACKAGE);
    assert_eq!(
        refused(
            &[game_a(Some(unregistered))],
            &[(INSTANCE_A, GAME_A, SERVER_A)],
            None
        ),
        (
            Some(GAME_A.to_owned()),
            StandardPackProblem::RulePackageUnregistered,
            "/rules/0/package_id".to_owned()
        )
    );
    let id_clash = GameRoot {
        game: GAME_B,
        schedule: Some(schedule_b(RULE, OTHER_PACKAGE)),
        bundle: bundle_b(&[OTHER_PACKAGE]),
    };
    assert_eq!(
        refused(&[game_a(Some(schedule_a())), id_clash], &both, None),
        (
            Some(GAME_B.to_owned()),
            StandardPackProblem::RuleIdConflict,
            "/rules/0/id".to_owned()
        ),
        "the later game in name order is refused"
    );
    let package_clash = GameRoot {
        game: GAME_B,
        schedule: Some(schedule_b(SECOND_RULE, PACKAGE)),
        bundle: bundle_b(&[PACKAGE]),
    };
    assert_eq!(
        refused(&[game_a(Some(schedule_a())), package_clash], &both, None),
        (
            Some(GAME_B.to_owned()),
            StandardPackProblem::RuleIdConflict,
            "/rules/0/package_id".to_owned()
        )
    );

    let mut requiring = schedule_a();
    requiring["requires"] = json!([REQUIRED_SUBSYSTEM]);
    let present = BTreeSet::from([PRESENT_SUBSYSTEM.to_owned()]);
    assert_eq!(
        refused(
            &[game_a(Some(requiring.clone()))],
            &[(INSTANCE_A, GAME_A, SERVER_A)],
            Some(&present)
        ),
        (
            Some(GAME_A.to_owned()),
            StandardPackProblem::ScheduleRequiresMissing,
            "/requires/0".to_owned()
        )
    );
    let named = BTreeSet::from([PRESENT_SUBSYSTEM.to_owned(), REQUIRED_SUBSYSTEM.to_owned()]);
    let composition = composed(
        &[game_a(Some(requiring.clone()))],
        &[(INSTANCE_A, GAME_A, SERVER_A)],
        Some(&named),
    );
    assert_eq!(composition.games()[0].requires, [REQUIRED_SUBSYSTEM]);
    let unchecked = composed(
        &[game_a(Some(requiring))],
        &[(INSTANCE_A, GAME_A, SERVER_A)],
        None,
    );
    assert_eq!(
        unchecked.games()[0].requires,
        [REQUIRED_SUBSYSTEM],
        "without a manifest the requirement is reported, for the Runtime to check"
    );
}

/// T5 (§3.1, "Classification"): a package neither maintenance-role, named by a rule nor held is
/// one `standard_pack_package_unclassified` warning and the rest of the game runs; a held
/// package is not scheduled.
#[test]
fn rl1a_t5_an_unclassified_package_warns_and_a_held_one_is_not_scheduled() {
    let mut unclassified = schedule_a();
    unclassified["held"] = json!([]);
    let composition = composed(
        &[game_a(Some(unclassified))],
        &[(INSTANCE_A, GAME_A, SERVER_A)],
        None,
    );
    let game = &composition.games()[0];
    assert_eq!(game.unclassified, [EXTRA_PACKAGE]);
    assert_eq!(game.maintenance, [RETURN_PACKAGE]);
    assert_eq!(game.scheduled, [PACKAGE]);
    assert_eq!(game.tasks, 1, "the rest of the game runs");
    let warnings = composition.warnings();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].is(SharedCode::StandardPackPackageUnclassified));
    assert_eq!(warnings[0].category(), Category::Warning);
    assert_eq!(warnings[0].value("game"), Some(&json!(GAME_A)));
    assert_eq!(warnings[0].value("package_id"), Some(&json!(EXTRA_PACKAGE)));

    let mut held = schedule_a();
    held["held"]
        .as_array_mut()
        .expect("the held list")
        .push(json!({"package_id": PACKAGE, "reason": HELD_REASON}));
    let composition = composed(
        &[game_a(Some(held))],
        &[(INSTANCE_A, GAME_A, SERVER_A)],
        None,
    );
    let game = &composition.games()[0];
    assert_eq!(game.held, [EXTRA_PACKAGE, PACKAGE]);
    assert!(game.scheduled.is_empty());
    assert!(game.unclassified.is_empty());
    assert!(composition.warnings().is_empty());
    assert!(composition.compiled().catalog().tasks.tasks.is_empty());
    assert!(composition.manifest().is_empty());
    let evaluation = evaluated(
        composition.compiled(),
        &grid_facts(&[INSTANCE_A]),
        &grid_resources(&[INSTANCE_A]),
        GRID_START + 12 * 60 * MINUTE_MS,
    )
    .expect("the composition evaluates");
    assert!(
        evaluation
            .decisions
            .iter()
            .all(|decision| decision.task_id != RULE),
        "the held package's rule is not scheduled"
    );
    assert!(evaluation.dispatched.is_empty());
}

/// SS-2 test 5 (register §1.222; Alice's ruling 104 point 1: two priority tiers, maintenance high
/// and business normal): a game whose composed rules use three distinct priority values is one
/// `standard_pack_priority_tiers_exceeded` warning (keys `game`, `count` 3, `limit_count` 2), and
/// its rules are composed all the same; one whose rules use two values is no warning. The root is
/// never refused.
#[test]
fn ss2_t5_three_priority_values_warn_and_two_do_not() {
    let rule = schedule_a()["rules"][0].clone();
    let with_priorities = |priorities: &[i64]| {
        let mut schedule = schedule_a();
        schedule["rules"] = Value::Array(
            priorities
                .iter()
                .enumerate()
                .map(|(index, priority)| {
                    let mut copy = rule.clone();
                    if index > 0 {
                        let id = format!("{RULE}.{index}");
                        copy["feedback_stop"]["task_id"] = json!(id);
                        copy["id"] = json!(id);
                    }
                    copy["priority"] = json!(priority);
                    copy
                })
                .collect(),
        );
        schedule
    };

    let three = composed(
        &[game_a(Some(with_priorities(&[300, 200, 100])))],
        &[(INSTANCE_A, GAME_A, SERVER_A)],
        None,
    );
    assert_eq!(
        three.compiled().catalog().tasks.tasks.len(),
        3,
        "the root is composed"
    );
    let warnings = three.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].is(SharedCode::StandardPackPriorityTiersExceeded));
    assert_eq!(warnings[0].category(), Category::Warning);
    assert_eq!(warnings[0].value("game"), Some(&json!(GAME_A)));
    assert_eq!(warnings[0].value("count"), Some(&json!(3)));
    assert_eq!(warnings[0].value("limit_count"), Some(&json!(2)));

    let two = composed(
        &[game_a(Some(with_priorities(&[300, 100, 100, 300])))],
        &[(INSTANCE_A, GAME_A, SERVER_A)],
        None,
    );
    assert_eq!(two.compiled().catalog().tasks.tasks.len(), 4);
    assert!(two.warnings().is_empty(), "{:?}", two.warnings());
}

/// T6 (§3.6, "No rules at all is a valid state"): a file without rules, a bound game whose root
/// holds no `schedule.json`, and no bound instance each compose a valid empty catalog.
#[test]
fn rl1a_t6_no_rules_compose_a_valid_empty_catalog() {
    let empty = json!({"schema_version": "actingcommand.schedule.v1"});
    let without_rules = GameRoot {
        game: GAME_A,
        schedule: Some(empty),
        bundle: bundle_maintenance_only(),
    };
    let without_file = GameRoot {
        game: GAME_A,
        schedule: None,
        bundle: bundle_maintenance_only(),
    };
    let empty_catalog = |case: &str, composition: StandardPackComposition| {
        for (list, document) in composed_documents(&composition) {
            assert_eq!(document[list], json!([]), "{case}: {list} is empty");
            assert_eq!(document["catalog"]["approval_refs"], json!([]), "{case}");
        }
        assert!(composition.manifest().is_empty(), "{case}");
        assert!(composition.warnings().is_empty(), "{case}");
        // The empty catalog declares no pool, so its inputs hold no pool reading: a reading of a
        // pool the catalog does not declare is an invalid input, with rules or without.
        let resources = EvaluationResources {
            pools: Vec::new(),
            ..grid_resources(&[INSTANCE_A])
        };
        let evaluation = evaluated(
            composition.compiled(),
            &grid_facts(&[INSTANCE_A]),
            &resources,
            GRID_START + 12 * 60 * MINUTE_MS,
        )
        .unwrap_or_else(|code| panic!("{case}: the empty catalog evaluates: {code}"));
        assert!(evaluation.decisions.is_empty(), "{case}");
        assert!(evaluation.dispatched.is_empty(), "{case}");
    };
    let bound = [(INSTANCE_A, GAME_A, SERVER_A)];
    let composition = composed(&[without_rules], &bound, None);
    assert_eq!(composition.games()[0].tasks, 0);
    assert!(composition.games()[0].schedule_sha256.is_some());
    empty_catalog("a file without rules", composition);
    let composition = composed(&[without_file], &bound, None);
    assert_eq!(
        composition.games()[0].schedule_sha256,
        None,
        "the bound game has no rules"
    );
    empty_catalog("a root without schedule.json", composition);
    let composition = composed(&[game_a(Some(schedule_a()))], &[], None);
    assert!(composition.games().is_empty(), "no game is in use");
    empty_catalog("no bound instance", composition);
}

/// T7 (the neutral core): every word of every fixture string is in the closed neutral list.
#[test]
fn rl1a_t7_no_fixture_names_a_game() {
    fn strings(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::String(text) => found.push(text.clone()),
            Value::Array(items) => items.iter().for_each(|item| strings(item, found)),
            Value::Object(members) => members.values().for_each(|member| strings(member, found)),
            _ => {}
        }
    }
    let mut unknown = BTreeSet::new();
    for (name, fixture) in fixtures() {
        let mut found = Vec::new();
        strings(&fixture, &mut found);
        for text in found {
            for word in text.split(|c: char| c.is_ascii() && !c.is_ascii_alphanumeric()) {
                let word = word.to_ascii_lowercase();
                if !word.is_empty()
                    && !word.bytes().all(|byte| byte.is_ascii_digit())
                    && !NEUTRAL_WORDS.contains(&word.as_str())
                {
                    unknown.insert(format!("{name}: {word}"));
                }
            }
        }
    }
    assert!(
        unknown.is_empty(),
        "words outside the closed neutral list: {unknown:?}"
    );
}
