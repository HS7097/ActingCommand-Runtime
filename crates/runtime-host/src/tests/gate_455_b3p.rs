// SPDX-License-Identifier: AGPL-3.0-only

// #455 B3-P, the host's half (0.12.82; register §1.522; the coordinator's rulings of 11-23, no
// words of the owner's behind them; coordinator work order `rd-1123/brief-455-b3p.md`;
// contracts/resource-targets.md, "One value by name"): the goal page sets a goal by `{at_least}`
// alone on a pool that writes `target_defaults` in place of a valuation, as on a valued pool, and
// the host's goal code is the one of before. Tested on what `EditGoal` runs: the page's pure check
// (`crate::goals::check_edit`) over the targetable pools the policy crate's own list gives for an
// active catalogue (`targetable_resources`, the list `ResourceTargetView` and the goal page hold),
// and the policy crate's one-row edit of the record and its reader. The catalogue: one task of the
// game producing a pool with `target_defaults` 10 and 1000, a pool whose valuation gives a step
// and a gap weight, and a pool with neither. Every name is made up; no clock is read.

use std::collections::BTreeMap;

use crate::goals::{EditRequest, GoalPage, check_edit};
use actingcommand_contract::{
    FactScalar, GoalProblem, Initiator, RuntimeChannel, StandardPackGoals, TargetableResource,
    parse_standard_pack_goals,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationTime, GoalRecordEdit,
    InstanceSnapshot, SCHEDULING_SCHEMA_VERSION_V2, StoredGoalRecord, TargetSetter, VariableSetBy,
    compile_catalog, edit_goal_record, stored_goal_record, targetable_resources,
};
use serde_json::{Value, json};

const GAME: &str = "neutral";
const INSTANCE: &str = "instance-a";
const OPERATION: &str = "operation.run";
const TASK: &str = "neutral.task";
/// A pool without a valuation whose `target_defaults` are 10 and 1000.
const LACKING: &str = "pool.neutral.lacking";
/// A pool whose valuation gives the step 5 and the gap weight 700.
const POINTS: &str = "pool.neutral.points";
/// A pool with neither a valuation nor `target_defaults`.
const BARE: &str = "pool.neutral.bare";

type Rows = Vec<BTreeMap<String, FactScalar>>;

// The brief's test 5. The page lists the pool with `target_defaults` as one a goal can be set on,
// its defaults 10 and 1000 given and no number required, as the valued pool with its valuation's;
// the pool with neither still requires both. A value `{at_least: 40}` set on the `resource` entry
// of that pool is the one row `SetTarget` with no number: the target row the record holds is the
// row the same request writes for the valued pool's entry, its id and pool aside, with no `scale`
// and no `importance_milli`; a target by pool id (`{resource, at_least}` under a name no entry
// has) is the same edit and the same row under its own name. The pool with neither is still
// refused `pool_unvalued` with its pool. Clearing the entry removes its target, and the record
// left with none is the withdrawal.
#[test]
fn b3p_host_a_goal_on_a_pool_with_target_defaults_is_set_by_at_least_alone_and_cleared() {
    let offered = targetable();
    let terms = |pool: &str| {
        let row = offered
            .iter()
            .find(|row| row.resource == pool)
            .unwrap_or_else(|| panic!("{pool} is targetable: {offered:?}"));
        (
            row.default_scale,
            row.default_importance_milli,
            row.scale_required,
            row.importance_required,
        )
    };
    assert_eq!(terms(LACKING), (Some(10), Some(1000), false, false));
    assert_eq!(terms(POINTS), (Some(5), Some(700), false, false));
    assert_eq!(terms(BARE), (None, None, true, true));
    let goals = goal_page(offered);

    let on_entry = set(&goals, "lacking", "{\"at_least\":40}").expect("the value is taken");
    assert_eq!(
        on_entry,
        GoalRecordEdit::SetTarget {
            id: "lacking".to_owned(),
            resource: LACKING.to_owned(),
            at_least: 40,
        }
    );
    let rows = written(None, &on_entry);
    let lacking_row = target_row(&rows, "lacking");
    assert!(
        !lacking_row.contains_key("scale") && !lacking_row.contains_key("importance_milli"),
        "{lacking_row:?}"
    );
    let on_valued = set(&goals, "points", "{\"at_least\":40}").expect("the value is taken");
    let valued_rows = written(None, &on_valued);
    assert_eq!(
        lacking_row,
        &renamed(target_row(&valued_rows, "points"), "lacking", LACKING),
        "the row of a valued pool's entry"
    );

    let by_pool_value = format!("{{\"resource\":\"{LACKING}\",\"at_least\":40}}");
    let by_pool = set(&goals, "by_pool", &by_pool_value).expect("the value is taken");
    assert_eq!(
        by_pool,
        GoalRecordEdit::SetTarget {
            id: "by_pool".to_owned(),
            resource: LACKING.to_owned(),
            at_least: 40,
        }
    );
    let by_pool_rows = written(None, &by_pool);
    assert_eq!(
        target_row(&by_pool_rows, "by_pool"),
        &renamed(lacking_row, "by_pool", LACKING)
    );

    let bare_value = format!("{{\"resource\":\"{BARE}\",\"at_least\":40}}");
    let refusal = set(&goals, "spare", &bare_value).expect_err("a pool with neither");
    assert_eq!(
        (refusal.problem, refusal.pool_id.as_deref()),
        (GoalProblem::PoolUnvalued, Some(BARE))
    );

    let recorded = stored_goal_record(Some(rows.as_slice()), None, INSTANCE);
    let clear = check_edit(
        &goals,
        &recorded,
        EditRequest::Clear { name: "lacking" },
        judged_by(),
    )
    .expect("the clear is taken");
    assert_eq!(
        clear,
        GoalRecordEdit::ClearTarget {
            id: "lacking".to_owned()
        }
    );
    let cleared = written(Some(&rows), &clear);
    assert_eq!(
        stored_goal_record(Some(cleared.as_slice()), None, INSTANCE),
        StoredGoalRecord::Empty
    );
}

/// What the check of `EditGoal` answers on `goals` with no stored record for a `set` of `name` to
/// `value_json`.
fn set(
    goals: &GoalPage,
    name: &str,
    value_json: &str,
) -> Result<GoalRecordEdit, crate::goals::GoalRefusal> {
    check_edit(
        goals,
        &StoredGoalRecord::Empty,
        EditRequest::Set { name, value_json },
        judged_by(),
    )
}

/// The setter a value is judged with: the Lab at 5000.
fn judged_by() -> VariableSetBy {
    VariableSetBy {
        set_unix_ms: 5_000,
        channel: RuntimeChannel::Lab,
    }
}

/// The rows `edit` writes over `stored`, set by the Lab's command line at 5000; the edit must be
/// taken.
fn written(stored: Option<&Rows>, edit: &GoalRecordEdit) -> Rows {
    edit_goal_record(
        stored.map(Vec::as_slice),
        None,
        INSTANCE,
        edit,
        TargetSetter {
            set_unix_ms: 5_000,
            channel: RuntimeChannel::Lab,
            initiator: Some(Initiator::LabCli),
        },
    )
    .unwrap_or_else(|refusal| panic!("{edit:?} is taken: {refusal:?}"))
    .composed
    .rows
}

/// The target row of `id` among `rows`.
fn target_row<'a>(rows: &'a Rows, id: &str) -> &'a BTreeMap<String, FactScalar> {
    rows.iter()
        .find(|row| {
            row.get("row") == Some(&FactScalar::String("target".to_owned()))
                && row.get("id") == Some(&FactScalar::String(id.to_owned()))
        })
        .unwrap_or_else(|| panic!("no target row {id}: {rows:?}"))
}

/// `row` with the id `id` and the pool `pool`.
fn renamed(
    row: &BTreeMap<String, FactScalar>,
    id: &str,
    pool: &str,
) -> BTreeMap<String, FactScalar> {
    let mut copy = row.clone();
    copy.insert("id".to_owned(), FactScalar::String(id.to_owned()));
    copy.insert("resource".to_owned(), FactScalar::String(pool.to_owned()));
    copy
}

/// The page: the game's `goals.json` with a `resource` entry on [`LACKING`] and one on [`POINTS`],
/// no package bound, the pools `targetable`.
fn goal_page(targetable: Vec<TargetableResource>) -> GoalPage {
    GoalPage {
        game_id: GAME.to_owned(),
        unchecked: None,
        declaration: Some(declaration()),
        packages: Vec::new(),
        targetable,
        capacities: BTreeMap::new(),
        not_applying: BTreeMap::new(),
        root: BTreeMap::new(),
    }
}

/// The game's `goals.json`, version 3 since Workflow #455 RR1 (version 2 before it): one
/// `resource` entry on [`LACKING`] and one on [`POINTS`], neither carrying a number.
fn declaration() -> StandardPackGoals {
    let document = json!({"schema_version": "actingcommand.goals.v3", "game": GAME, "entries": [
        {"name": "lacking", "type": "resource", "governs": {"pool": LACKING},
         "title": {"en": "Material"}},
        {"name": "points", "type": "resource", "governs": {"pool": POINTS},
         "title": {"en": "Points"}}
    ]});
    parse_standard_pack_goals(&serde_json::to_vec(&document).expect("goals bytes"), GAME)
        .unwrap_or_else(|issues| panic!("the fixture goals.json reads: {issues:?}"))
}

/// The pools the application can target under the active catalogue, as the policy crate lists
/// them for the host's views.
fn targetable() -> Vec<TargetableResource> {
    let instance = InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: "server-a".to_owned(),
        game_id: GAME.to_owned(),
        host_id: format!("host.{INSTANCE}"),
        available: true,
        capability_operation_ids: vec![OPERATION.to_owned()],
        preferred_task_ids: Vec::new(),
    };
    let facts = EvaluationFacts {
        ledger_position: 1,
        fact_snapshot_id: "snapshot:fixture-455-b3p".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance.clone()],
        priority_offsets: Vec::new(),
    };
    let compiled = compile_catalog(&sources())
        .unwrap_or_else(|failure| panic!("the catalogue compiles: {:?}", failure.diagnostics()));
    let at = EvaluationTime {
        unix_ms: 1_000_000,
        monotonic_ms: 1_000_000,
    };
    targetable_resources(&compiled, &facts, &instance, at)
        .unwrap_or_else(|error| panic!("the targetable list: {error:?}"))
        .0
}

/// A pool of the instance of constant capacity 1000 that does not regenerate, read from
/// `resource.<its id>`, with `extra` members.
fn declared_pool(id: &str, extra: Value) -> Value {
    let mut declared = json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": INSTANCE},
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{id}")},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": null
    });
    if let (Some(members), Value::Object(extra)) = (declared.as_object_mut(), extra) {
        members.extend(extra);
    }
    declared
}

/// The V2 catalogue: [`TASK`] on an hourly clock producing 2 of each pool a run.
fn sources() -> CatalogSources {
    let header = json!({
        "catalog_id": "fixture.455-b3p-host",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-b3p-host"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let produce = |pool: &str| {
        json!({"pool_id": pool, "direction": "produce", "amount": 2,
               "observation_source": "scan_verified", "confidence_milli": 1000})
    };
    let task = json!({
        "id": TASK,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{TASK}"),
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3_600_000,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": TASK,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [produce(LACKING), produce(POINTS), produce(BARE)],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86_400_000,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    });
    let pool_list = json!([
        declared_pool(
            LACKING,
            json!({"target_defaults": {"scale": 10, "importance_milli": 1000}})
        ),
        declared_pool(
            POINTS,
            json!({"valuation": {"name": "Points", "unit": "point", "scale": 5,
                                 "base_weight_milli": 100,
                                 "gap": {"rule": "shortfall_linear", "weight_milli": 700}}})
        ),
        declared_pool(BARE, json!({}))
    ]);
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
    let source = |name: &str, declared: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-455-b3p-host/{name}.json"),
            serde_json::to_vec(declared).expect("fixture document bytes"),
        )
    };
    CatalogSources {
        tasks: source("tasks", &document("tasks", json!([task]))),
        pools: source("pools", &document("pools", pool_list)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    }
}
