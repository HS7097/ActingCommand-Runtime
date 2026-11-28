// SPDX-License-Identifier: AGPL-3.0-only

// #455 SL2, the host's half (0.12.101; register §1.547; the coordinator's rulings of 11-28, after
// the owner's words that a goal never changes the weights of the business layer and that a line on
// a saved currency has no date; coordinator work order `rd-1123/brief-455-sl2.md`;
// contracts/resource-targets.md, "A pool's line"; contracts/runtime-interface.md, "Goal request
// kinds"): the goal page sets a pool's line by `{at_least}` alone on a pool that writes
// `spend_line`, with no task producing it, and shows it as it shows any resource entry. The host's
// goal code is the one of before: the targetable list the policy crate gives names such a pool
// with neither term required, its defaults the pool's `spend_line` numbers. Tested on what
// `EditGoal` and `GoalView` run: the page's pure check (`crate::goals::check_edit`) and view
// (`crate::goals::view_entries`) over the policy crate's targetable list (`targetable_resources`),
// its one-row edit of the record (`edit_goal_record`) and its reading of the stored targets'
// conditions (`active_resource_targets`). Every name is made up; no clock is read.

use std::collections::BTreeMap;

use crate::goals::{EditRequest, GoalPage, check_edit, view_entries};
use actingcommand_contract::{
    FactScalar, GoalProblem, GoalResourceState, Initiator, RESOURCE_TARGETS_FACT_KEY,
    ResourceTargetObservation, RuntimeChannel, StandardPackGoals, TargetableResource,
    parse_standard_pack_goals, variable_value_json,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, EvaluationFacts, EvaluationTime,
    GivenPoolReadings, GoalRecordEdit, InstanceSnapshot, SCHEDULING_SCHEMA_VERSION_V2,
    StoredGoalRecord, StoredResourceTargets, TargetSetter, VariableSetBy, active_resource_targets,
    compile_catalog, edit_goal_record, stored_goal_record, targetable_resources,
};
use serde_json::{Value, json};

const GAME: &str = "neutral";
const INSTANCE: &str = "instance-a";
const OPERATION: &str = "operation.run";
const TASK: &str = "neutral.task";
/// A pool without a valuation that writes `spend_line`; no task produces it.
const CURRENCY: &str = "pool.neutral.currency";
/// A pool whose valuation gives the step 5 and the gap weight 700; [`TASK`] produces it.
const POINTS: &str = "pool.neutral.points";
/// The time of the page.
const AT: u64 = 1_000_000;
/// When the pools were read: a minute before [`AT`].
const READ_AT: u64 = AT - 60_000;

type Rows = Vec<BTreeMap<String, FactScalar>>;

// The brief's test 1. The targetable list names the currency, which no task produces, with its
// `spend_line`'s 6000 and 33 333 as its defaults and neither required, its reading 25 as for any
// pool. `{at_least: 40}` on its `resource` entry is the one row `SetTarget`, and the target row the
// record holds is the row the same request writes for the valued pool's entry, its id and pool
// aside, with no `scale`, `importance_milli` or `tasks`; by its pool id under a name no entry has it
// is the same edit and row under that name. A deadline is still `deadline_unsupported` at
// `/by_unix_ms`, and an `at_least` of 0 or above the pool's capacity of 1000 `out_of_range` at
// `/at_least` with the range 1 to 1000 and the pool, as on any pool. The page lists the entry with
// its value, who set it, the pool's reading and the gap 15 (40 less 25).
#[test]
fn sl2_host_a_line_is_set_by_at_least_alone_and_shown_as_a_goal_is() {
    let compiled = catalogue();
    let offered = listed(&compiled, &readings());
    let currency = offered
        .iter()
        .find(|row| row.resource == CURRENCY)
        .unwrap_or_else(|| panic!("the currency is targetable: {offered:?}"));
    assert_eq!(
        currency,
        &TargetableResource {
            resource: CURRENCY.to_owned(),
            fact_key: format!("resource.{CURRENCY}"),
            producing_tasks: Vec::new(),
            default_scale: Some(6000),
            default_importance_milli: Some(33_333),
            scale_required: false,
            importance_required: false,
            observation: observed(25),
        }
    );
    let goals = goal_page(offered.clone());

    let on_entry = set(&goals, "currency", "{\"at_least\":40}").expect("the line is taken");
    assert_eq!(
        on_entry,
        GoalRecordEdit::SetTarget {
            id: "currency".to_owned(),
            resource: CURRENCY.to_owned(),
            at_least: 40,
        }
    );
    let rows = written(None, &on_entry);
    let line_row = target_row(&rows, "currency");
    for member in ["scale", "importance_milli", "tasks"] {
        assert!(!line_row.contains_key(member), "{member}: {line_row:?}");
    }
    let on_valued = set(&goals, "points", "{\"at_least\":40}").expect("the goal is taken");
    let valued_rows = written(None, &on_valued);
    assert_eq!(
        line_row,
        &renamed(target_row(&valued_rows, "points"), "currency", CURRENCY),
        "the row of a valued pool's entry"
    );
    let by_pool_value = format!("{{\"resource\":\"{CURRENCY}\",\"at_least\":40}}");
    let by_pool = set(&goals, "by_pool", &by_pool_value).expect("the line is taken");
    assert_eq!(
        by_pool,
        GoalRecordEdit::SetTarget {
            id: "by_pool".to_owned(),
            resource: CURRENCY.to_owned(),
            at_least: 40,
        }
    );
    let by_pool_rows = written(None, &by_pool);
    assert_eq!(
        target_row(&by_pool_rows, "by_pool"),
        &renamed(line_row, "by_pool", CURRENCY)
    );

    let deadline =
        set(&goals, "currency", "{\"at_least\":40,\"by_unix_ms\":5000}").expect_err("a deadline");
    assert_eq!(
        (deadline.problem, deadline.pointer.as_deref()),
        (GoalProblem::DeadlineUnsupported, Some("/by_unix_ms"))
    );
    let mut bounded = goal_page(offered);
    bounded.capacities.insert(CURRENCY.to_owned(), 1000);
    for at_least in [0, 1001] {
        let refusal = set(
            &bounded,
            "currency",
            &format!("{{\"at_least\":{at_least}}}"),
        )
        .expect_err("out of range");
        assert_eq!(
            (
                refusal.problem,
                refusal.pointer.as_deref(),
                refusal.range,
                refusal.pool_id.as_deref()
            ),
            (
                GoalProblem::OutOfRange,
                Some("/at_least"),
                Some((1, 1000)),
                Some(CURRENCY)
            ),
            "{at_least}"
        );
    }

    let stored = stored_goal_record(Some(rows.as_slice()), None, INSTANCE);
    let conditions = match active_resource_targets(
        Some(&compiled),
        &with_record(readings(), &rows),
        &GivenPoolReadings::default(),
        &instance(),
        time(),
    ) {
        Ok(StoredResourceTargets::Policy(policy)) => policy.conditions,
        other => panic!("the stored line is read: {other:?}"),
    };
    let viewed = view_entries(&goals, &stored, &[], &conditions);
    let entry = viewed
        .entries
        .iter()
        .find(|entry| entry.name == "currency")
        .unwrap_or_else(|| panic!("the currency's entry: {viewed:?}"));
    assert_eq!(
        entry.value_json,
        Some(variable_value_json(&json!({"at_least": 40})))
    );
    assert_eq!(
        entry.set.as_ref().map(|set_by| set_by.unix_ms),
        Some(5_000),
        "{entry:?}"
    );
    assert_eq!(
        entry.resource,
        Some(GoalResourceState {
            pool: CURRENCY.to_owned(),
            target_id: Some("currency".to_owned()),
            observation: Some(observed(25)),
            gap: Some(15),
        })
    );
}

// The brief's test 7, the host's half: what the page of a build before this one does, fed the row
// its targetable list gave the currency (none without a producer; with one, both terms required
// and no default). The set of the line is `pool_not_targetable`, or `pool_unvalued`, with the
// pool; the clear of the line stored by this build is taken on either page (`ClearTarget` of its
// id), as a clear never reads the targetable list: a person clears the lines before going back,
// or after.
#[test]
fn sl2_host_an_older_page_refuses_to_set_a_line_and_clears_a_stored_one() {
    let offered = listed(&catalogue(), &readings());
    let points = || {
        offered
            .iter()
            .find(|row| row.resource == POINTS)
            .cloned()
            .unwrap_or_else(|| panic!("the valued pool is targetable: {offered:?}"))
    };
    let mut both_required = offered
        .iter()
        .find(|row| row.resource == CURRENCY)
        .cloned()
        .unwrap_or_else(|| panic!("the currency is targetable: {offered:?}"));
    both_required.producing_tasks = vec![TASK.to_owned()];
    both_required.default_scale = None;
    both_required.default_importance_milli = None;
    both_required.scale_required = true;
    both_required.importance_required = true;
    let rows = written(
        None,
        &set(&goal_page(offered.clone()), "currency", "{\"at_least\":40}")
            .expect("the line is taken"),
    );
    let stored = stored_goal_record(Some(rows.as_slice()), None, INSTANCE);
    for (page, problem) in [
        (goal_page(vec![points()]), GoalProblem::PoolNotTargetable),
        (
            goal_page(vec![both_required, points()]),
            GoalProblem::PoolUnvalued,
        ),
    ] {
        let refusal = set(&page, "currency", "{\"at_least\":40}").expect_err("the older page");
        assert_eq!(
            (refusal.problem, refusal.pool_id.as_deref()),
            (problem, Some(CURRENCY))
        );
        let clear = check_edit(
            &page,
            &stored,
            EditRequest::Clear { name: "currency" },
            judged_by(),
        )
        .expect("the clear is taken");
        assert_eq!(
            clear,
            GoalRecordEdit::ClearTarget {
                id: "currency".to_owned()
            }
        );
    }
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

/// The page: the game's `goals.json` with a `resource` entry on [`CURRENCY`] and one on
/// [`POINTS`], no package bound, the pools `targetable`, no capacity.
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

fn instance() -> InstanceSnapshot {
    InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: "server-a".to_owned(),
        game_id: GAME.to_owned(),
        host_id: format!("host.{INSTANCE}"),
        available: true,
        capability_operation_ids: vec![OPERATION.to_owned()],
        preferred_task_ids: Vec::new(),
    }
}

fn time() -> EvaluationTime {
    EvaluationTime {
        unix_ms: AT,
        monotonic_ms: AT,
    }
}

/// The instance with the currency read at 25 and the valued pool at 7, at [`READ_AT`].
fn readings() -> EvaluationFacts {
    let reading = |pool: &str, value: i64| actingcommand_policy::ObservedFact {
        scope: actingcommand_policy::ScopeSelector::Instance {
            instance_id: INSTANCE.to_owned(),
        },
        fact_key: format!("resource.{pool}"),
        value: actingcommand_policy::FactValue::Integer(value),
        observed_at_unix_ms: READ_AT,
        expires_at_unix_ms: None,
        confidence_milli: 1000,
    };
    EvaluationFacts {
        ledger_position: 1,
        fact_snapshot_id: "snapshot:fixture-455-sl2".to_owned(),
        facts: vec![reading(CURRENCY, 25), reading(POINTS, 7)],
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance()],
        priority_offsets: Vec::new(),
    }
}

/// `facts` with the instance's goal record of `rows`, stored at 5000 without an expiry, as the
/// goal edit stores a v3 record.
fn with_record(mut facts: EvaluationFacts, rows: &Rows) -> EvaluationFacts {
    facts.facts.push(actingcommand_policy::ObservedFact {
        scope: actingcommand_policy::ScopeSelector::Instance {
            instance_id: INSTANCE.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: actingcommand_policy::FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(rows).expect("rows")).expect("record list"),
        ),
        observed_at_unix_ms: 5_000,
        expires_at_unix_ms: None,
        confidence_milli: 1000,
    });
    facts
}

/// The reading `current` at [`READ_AT`], as the targetable list shows it.
fn observed(current: i64) -> ResourceTargetObservation {
    ResourceTargetObservation {
        current: Some(current),
        observed_at_unix_ms: Some(READ_AT),
        pending: None,
    }
}

/// The pools the application can target under `compiled` over `facts`, as the policy crate lists
/// them for the host's views.
fn listed(compiled: &CompiledCatalog, facts: &EvaluationFacts) -> Vec<TargetableResource> {
    targetable_resources(compiled, facts, &instance(), time())
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

/// The V2 catalogue: [`TASK`] on an hourly clock producing 2 of [`POINTS`] a run; nothing
/// produces [`CURRENCY`].
fn catalogue() -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-sl2-host",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-sl2-host"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
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
        "produces": [{"pool_id": POINTS, "direction": "produce", "amount": 2,
                      "observation_source": "scan_verified", "confidence_milli": 1000}],
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
            format!("memory://fixture-455-sl2-host/{name}.json"),
            serde_json::to_vec(declared).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", json!([task]))),
        pools: source("pools", &document("pools", pool_list)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalogue compiles: {:?}", failure.diagnostics()))
}
