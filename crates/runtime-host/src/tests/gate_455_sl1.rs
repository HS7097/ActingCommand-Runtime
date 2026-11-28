// SPDX-License-Identifier: AGPL-3.0-only

// #455 SL1, the host's half (0.12.97; register §1.540; the coordinator's ruling 5 of 11-28, no
// words of the owner's behind it; coordinator work order `rd-1123/brief-455-sl1.md`;
// contracts/scheduling/README.md, "A pool whose spending a line governs"): the goal page and the
// host's goal code are untouched, so a pool that writes `spend_line` is offered as no pool a goal
// can be set on by `{at_least}`, and a goal on it, by its `resource` entry or by its pool id, is
// refused `pool_unvalued` as on any pool without a valuation. Tested on what `EditGoal` runs: the
// page's pure check (`crate::goals::check_edit`) over the targetable pools the policy crate's own
// list gives for an active catalogue (`targetable_resources`). The catalogue: one task of the game
// producing the pool that writes `spend_line` and a pool whose valuation gives a step and a gap
// weight. Every name is made up; no clock is read.

use std::collections::BTreeMap;

use crate::goals::{EditRequest, GoalPage, check_edit};
use actingcommand_contract::{
    GoalProblem, RuntimeChannel, StandardPackGoals, TargetableResource, parse_standard_pack_goals,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationTime, GoalRecordEdit,
    InstanceSnapshot, SCHEDULING_SCHEMA_VERSION_V2, StoredGoalRecord, VariableSetBy,
    compile_catalog, targetable_resources,
};
use serde_json::{Value, json};

const GAME: &str = "neutral";
const INSTANCE: &str = "instance-a";
const OPERATION: &str = "operation.run";
const TASK: &str = "neutral.task";
/// A pool without a valuation that writes `spend_line`.
const CURRENCY: &str = "pool.neutral.currency";
/// A pool whose valuation gives the step 5 and the gap weight 700.
const POINTS: &str = "pool.neutral.points";

// The brief's test 5. The targetable list gives the pool that writes `spend_line` no step and no
// importance and requires both, as for any pool without a valuation, while the valued pool has its
// valuation's; so the page offers no goal on it by `{at_least}`. That value on its `resource` entry
// is refused `pool_unvalued` with its pool, and so is a target by its pool id; the same value on
// the valued pool's entry is taken, as today.
#[test]
fn sl1_host_a_pool_whose_spending_a_line_governs_takes_no_goal_by_at_least() {
    let offered = targetable();
    let terms = |pool: &str| {
        let row = offered
            .iter()
            .find(|row| row.resource == pool)
            .unwrap_or_else(|| panic!("{pool} is listed: {offered:?}"));
        (
            row.default_scale,
            row.default_importance_milli,
            row.scale_required,
            row.importance_required,
        )
    };
    assert_eq!(terms(CURRENCY), (None, None, true, true));
    assert_eq!(terms(POINTS), (Some(5), Some(700), false, false));
    let goals = goal_page(offered);

    let refusal = set(&goals, "currency", "{\"at_least\":40}").expect_err("its resource entry");
    assert_eq!(
        (refusal.problem, refusal.pool_id.as_deref()),
        (GoalProblem::PoolUnvalued, Some(CURRENCY))
    );
    let by_pool_value = format!("{{\"resource\":\"{CURRENCY}\",\"at_least\":40}}");
    let refusal = set(&goals, "by_pool", &by_pool_value).expect_err("its pool id");
    assert_eq!(
        (refusal.problem, refusal.pool_id.as_deref()),
        (GoalProblem::PoolUnvalued, Some(CURRENCY))
    );

    let taken = set(&goals, "points", "{\"at_least\":40}").expect("the valued pool's entry");
    assert_eq!(
        taken,
        GoalRecordEdit::SetTarget {
            id: "points".to_owned(),
            resource: POINTS.to_owned(),
            at_least: 40,
        }
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
        VariableSetBy {
            set_unix_ms: 5_000,
            channel: RuntimeChannel::Lab,
        },
    )
}

/// The page: the game's `goals.json` with a `resource` entry on [`CURRENCY`] and one on
/// [`POINTS`], no package bound, the pools `targetable`.
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

/// The game's `goals.json`, version 3: one `resource` entry on [`CURRENCY`] and one on [`POINTS`],
/// neither carrying a number.
fn declaration() -> StandardPackGoals {
    let document = json!({"schema_version": "actingcommand.goals.v3", "game": GAME, "entries": [
        {"name": "currency", "type": "resource", "governs": {"pool": CURRENCY},
         "title": {"en": "Currency"}},
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
        fact_snapshot_id: "snapshot:fixture-455-sl1".to_owned(),
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
        "catalog_id": "fixture.455-sl1-host",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-sl1-host"]
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
        "produces": [produce(CURRENCY), produce(POINTS)],
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
            CURRENCY,
            json!({"spend_line": {"price_milli": 33333, "scale": 6000,
                                  "income_floor_milli_per_day": 71429}})
        ),
        declared_pool(
            POINTS,
            json!({"valuation": {"name": "Points", "unit": "point", "scale": 5,
                                 "base_weight_milli": 100,
                                 "gap": {"rule": "shortfall_linear", "weight_milli": 700}}})
        )
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
            format!("memory://fixture-455-sl1-host/{name}.json"),
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
