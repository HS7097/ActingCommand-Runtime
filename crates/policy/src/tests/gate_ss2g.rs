// SPDX-License-Identifier: AGPL-3.0-only

// SS-2g (register §1.232; design `rd-1031/design-acm-price-addendum2.1.md` §3.5 and §7.8, tests 1
// to 7; coordinator work order `rd-1031/brief-ss2g.md`; the research channel's review of addendum
// 2, point 2, and the coordinator's choice: the expectation is the model's, not the package's last
// spend): a V2 `consumes` entry may declare `expected_amount_milli`, as a production may.
// - The compiler accepts it on a V2 consumption alone, at most a constant capacity × 1000; V1, a
//   second quantity and the pending marker beside it are refused as before.
// - The net benefit reads the consumption `C = Σ amount × 1000 + Σ expected_amount_milli` in
//   thousandths and costs it `C·W/(1000·Q)`, exactly.
// - Every reader of an integer consumption reads `⌈C/1000⌉`: the reserve view and gate, the
//   consumption a reading does not show yet, SS-2's tie key and the selection field
//   `task.consumes.<pool>`.
// - SS-6's `run_cap` is absent when the task reserves a pool it consumes by expectation;
//   `spend_allowance` is unchanged.
// - Forward projection answers `effect_evidence_insufficient` for an expected consumption, as for
//   any expected effect.
//
// The catalog is neutral and V2: tasks on one instance with an hourly clock due at NOW, and four
// pools of the instance read from their keys, none regenerating. Every candidate's host is idle.
// Every expected value comes from §7.8's formulas and examples.

use serde_json::{Value, json};

use super::selection_catalog::{
    DAY_MS, HOUR_MS, NOW, drop_candidate, fact, field_ref, identity_term, instance, integer_field,
    selection,
};
use crate::{
    CatalogCompileFailure, CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources,
    CompiledCatalog, DecisionReason, DecisionReasonChain, EvaluationFacts, EvaluationResources,
    EvaluationTime, ForwardEvidenceGap, ForwardProjectionCompleteness, ForwardProjectionConfig,
    HostResourceSnapshot, ObservedFact, PolicyCode, PolicyEvaluation, TaskDecision,
    TaskRuntimeSnapshot, TaskTerminalState, compile_catalog, evaluate, project_fact_pools,
    project_forward,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
/// Valued 1000 milli a unit (scale 1); capacity 1,000,000.
const VALUED: &str = "pool_valued";
/// Valued 2000 milli a unit (scale 1); capacity 1000, so at most 1,000,000 thousandths a run.
const DEAR: &str = "pool_dear";
/// No valuation; capacity 20,000. The reserves of the tests are on it.
const SPENT: &str = "pool_spent";
/// No valuation; capacity 20,000. No task reserves it.
const OTHER: &str = "pool_other";
const MINUTE_MS: u64 = 60_000;
const RESERVE_INSUFFICIENT: &str = "resource_reserve_insufficient";
const PARAMETERS: &str = "policy_dispatch_parameters";

// Test 1 (§7.8 item 1): a V2 consumption that writes `expected_amount_milli` compiles and keeps it
// as written (no `amount`); up to the pool's constant capacity × 1000 it passes, above it is
// `effect_incompatible`. Beside `amount`, or beside the pending marker, it is `limit_exceeded` at
// the entry; in a V1 document it is `limit_exceeded` at the entry, as an expected production is.
#[test]
fn ss2g_t1_a_v2_consumption_may_declare_its_expected_amount_and_bad_shapes_are_refused() {
    let accepted = catalog(
        vec![task(
            "task_spend",
            hourly(),
            vec![expected(DEAR, 26)],
            Vec::new(),
        )],
        None,
    );
    let spec = &accepted.catalog().tasks.tasks[0];
    assert_eq!(spec.consumes[0].expected_amount_milli, Some(26));
    assert_eq!(spec.consumes[0].units(), None);
    let written = serde_json::to_value(spec).expect("task");
    assert_eq!(written["consumes"][0]["expected_amount_milli"], json!(26));
    assert!(
        written["consumes"][0].get("amount").is_none(),
        "{written:#}"
    );
    compile(
        vec![task(
            "task_spend",
            hourly(),
            vec![expected(DEAR, 1_000_000)],
            Vec::new(),
        )],
        None,
    )
    .unwrap_or_else(|failure| panic!("capacity × 1000 compiles: {:?}", failure.diagnostics()));

    let refused = |consumption: Value, code: CatalogDiagnosticCode| {
        let failure = compile(
            vec![task(
                "task_spend",
                hourly(),
                vec![consumption.clone()],
                Vec::new(),
            )],
            None,
        )
        .expect_err("refused");
        assert!(
            refused_at(&failure, code, "/tasks/0/consumes/0"),
            "{consumption}: {code:?}: {:?}",
            failure.diagnostics()
        );
    };
    refused(
        expected(DEAR, 1_000_001),
        CatalogDiagnosticCode::EffectIncompatible,
    );
    refused(
        effect(
            DEAR,
            "consume",
            json!({"amount": 1, "expected_amount_milli": 26}),
        ),
        CatalogDiagnosticCode::LimitExceeded,
    );
    refused(
        effect(
            DEAR,
            "consume",
            json!({"amount": "pending", "expected_amount_milli": 26}),
        ),
        CatalogDiagnosticCode::LimitExceeded,
    );

    let v1 = compile_v1_example(|tasks| {
        tasks["tasks"][0]["consumes"] = json!([{
            "pool_id": "fixture-pool-a",
            "direction": "consume",
            "expected_amount_milli": 400,
            "observation_source": "self_reported",
            "confidence_milli": 1000
        }]);
    })
    .expect_err("V1 requires amount");
    assert!(
        refused_at(
            &v1,
            CatalogDiagnosticCode::LimitExceeded,
            "/tasks/0/consumes/0"
        ),
        "{:?}",
        v1.diagnostics()
    );
}

// Test 2 (§7.8 item 2): the net benefit costs an expected consumption of 26 milli of a pool with
// W = 2000 and Q = 1 exactly 26 × 2000 / 1000 = 52 milli: producing one unit of a pool with
// W = 1000 beside it gives B = 948 and runs; alone it gives B = −52 (never −2000, the cost of a
// whole unit, and never 0). An integer catalog is unchanged: `amount` 1 costs 2000 as before, and
// 1000 expected milli decide exactly as `amount` 1 does.
#[test]
fn ss2g_t2_the_net_benefit_reads_an_expected_consumption_exactly() {
    let gain = catalog(
        vec![judged(task(
            "task_gain",
            hourly(),
            vec![expected(DEAR, 26)],
            vec![produce(VALUED, 1)],
        ))],
        None,
    );
    let evaluation = round(&gain, Vec::new(), Vec::new());
    assert_eq!(dispatched(&evaluation), ["task_gain"], "{evaluation:#?}");
    let breakdown = reason(decision(&evaluation, "task_gain"), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(948)));

    let cost = |consumption: Value| -> DecisionReason {
        let catalog = catalog(
            vec![judged(task(
                "task_spend",
                hourly(),
                vec![consumption],
                Vec::new(),
            ))],
            None,
        );
        let evaluation = round(&catalog, Vec::new(), Vec::new());
        assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
        let held = decision(&evaluation, "task_spend");
        let last = last_reason(held);
        assert!(
            last.outcome.is(PolicyCode::PolicyBenefitNonpositive),
            "{held:#?}"
        );
        last.clone()
    };
    let fraction = cost(expected(DEAR, 26));
    assert_eq!(fraction.outcome.value("benefit_milli"), Some(&json!(-52)));
    assert!(
        fraction.detail.ends_with(" items=pool_dear:-52"),
        "{}",
        fraction.detail
    );
    let whole = cost(consume(DEAR, 1));
    assert_eq!(whole.outcome.value("benefit_milli"), Some(&json!(-2000)));
    assert!(
        whole.detail.ends_with(" items=pool_dear:-2000"),
        "{}",
        whole.detail
    );
    assert_eq!(cost(expected(DEAR, 1000)), whole);
}

// Test 3 (§7.8 item 3): the reserve gate reads ⌈C/1000⌉. The pool reads 100 and the task must
// leave 99. An expected 500 milli reads 1 (100 − 1 ≥ 99) and runs, and so do 1000 milli; 1001
// milli read 2 and the task is held with `resource_reserve_insufficient`, the pool left at 98.
#[test]
fn ss2g_t3_the_reserve_gate_reads_an_expected_consumption_rounded_up() {
    let gate = |milli: u64| {
        let catalog = catalog(
            vec![reserving(
                task(
                    "task_reserve",
                    hourly(),
                    vec![expected(SPENT, milli)],
                    Vec::new(),
                ),
                99,
            )],
            None,
        );
        round(&catalog, Vec::new(), vec![reading(SPENT, 100)])
    };
    for milli in [500, 1000] {
        let evaluation = gate(milli);
        assert_eq!(
            dispatched(&evaluation),
            ["task_reserve"],
            "{milli}: {evaluation:#?}"
        );
    }
    let evaluation = gate(1001);
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_reserve");
    let why = last_reason(held);
    assert_eq!(
        why.outcome.code().as_str(),
        RESERVE_INSUFFICIENT,
        "{held:#?}"
    );
    assert_eq!(why.outcome.value("pool_id"), Some(&json!(SPENT)), "{why:?}");
    assert_eq!(
        why.outcome.value("reserve_at_least"),
        Some(&json!(99)),
        "{why:?}"
    );
    assert_eq!(
        why.outcome.value("amount_after"),
        Some(&json!(98)),
        "{why:?}"
    );
    assert_eq!(why.outcome.value("task_id"), None, "{why:?}");
}

// Test 4 (§7.8 item 3): a dispatched task whose consumption the pool's reading does not show yet
// counts ⌈C/1000⌉ against another task's reserve (the gate's step 3). The earlier task consumes
// by expectation and ran half a minute before NOW, after the reading of 100 a minute before NOW;
// the reserving task consumes 1 and must leave 98. 1001 milli count 2: 100 − 1 − 2 = 97 < 98, held
// and naming the earlier task. 1000 milli count 1: 98, and it runs.
#[test]
fn ss2g_t4_an_unshown_expected_consumption_counts_rounded_up() {
    let unshown = |milli: u64| {
        let catalog = catalog(
            vec![
                task(
                    "task_earlier",
                    hourly(),
                    vec![expected(SPENT, milli)],
                    Vec::new(),
                ),
                reserving(
                    task(
                        "task_reserve",
                        hourly(),
                        vec![consume(SPENT, 1)],
                        Vec::new(),
                    ),
                    98,
                ),
            ],
            None,
        );
        round(
            &catalog,
            vec![ran("task_earlier", NOW - 30_000)],
            vec![reading(SPENT, 100)],
        )
    };
    let evaluation = unshown(1001);
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_reserve");
    let why = last_reason(held);
    assert_eq!(
        why.outcome.code().as_str(),
        RESERVE_INSUFFICIENT,
        "{held:#?}"
    );
    assert_eq!(
        why.outcome.value("reserve_at_least"),
        Some(&json!(98)),
        "{why:?}"
    );
    assert_eq!(
        why.outcome.value("amount_after"),
        Some(&json!(97)),
        "{why:?}"
    );
    assert_eq!(
        why.outcome.value("task_id"),
        Some(&json!("task_earlier")),
        "{why:?}"
    );

    let evaluation = unshown(1000);
    assert_eq!(dispatched(&evaluation), ["task_reserve"], "{evaluation:#?}");
}

// Test 5 (§7.8 item 4): a task that reserves a pool it consumes by expectation gets its
// `spend_allowance` (1000 − 100 = 900) and no `run_cap`; consuming the same pool by `amount` 30
// it gets the run cap of today, 900 / 30 = 30, and so it does when its expected consumption is
// of another pool that it does not reserve.
#[test]
fn ss2g_t5_a_reserved_expected_consumption_gives_no_run_cap() {
    let parameters_of = |consumes: Vec<Value>| {
        let catalog = catalog(
            vec![reserving(
                task("task_reserve", hourly(), consumes, Vec::new()),
                100,
            )],
            None,
        );
        let evaluation = round(&catalog, Vec::new(), vec![reading(SPENT, 1000)]);
        let reason = parameters(&evaluation, "task_reserve")
            .unwrap_or_else(|| panic!("{PARAMETERS}: {evaluation:#?}"));
        (
            reason.outcome.value("spend_allowances").cloned(),
            reason.outcome.value("run_cap").cloned(),
        )
    };
    let allowance = Some(json!([{"effect_index": 0, "spend_allowance": 900}]));
    assert_eq!(
        parameters_of(vec![expected(SPENT, 30_000)]),
        (allowance.clone(), None),
        "a reserved pool consumed by expectation"
    );
    assert_eq!(
        parameters_of(vec![consume(SPENT, 30)]),
        (allowance.clone(), Some(json!(30))),
        "an integer consumption"
    );
    assert_eq!(
        parameters_of(vec![consume(SPENT, 30), expected(OTHER, 5_000)]),
        (allowance, Some(json!(30))),
        "an expected consumption of a pool the task does not reserve"
    );
}

// Test 6 (§7.8 item 3): SS-2's tie key and the selection field `task.consumes.<pool>` read
// ⌈C/1000⌉.
// - The field: a selection document whose one term scores `task.consumes.<pool>` as it is ranks a
//   task consuming 1000 expected milli at 1 milli and one consuming 1001 at 2.
// - The tie key: two tied tasks need the pool at 9000 or more, which holds 10,000. B, declared
//   first, consumes 100 a run: eleven runs are left above its threshold. A consumes 500,001
//   expected milli, read 501: (10,000 − 9000) / 501 + 1 = 2 runs (500 would leave 3), so A runs.
#[test]
fn ss2g_t6_the_tie_key_and_the_selection_field_read_the_rounded_up_consumption() {
    let field = format!("task.consumes.{SPENT}");
    let document = selection(
        vec![integer_field(&field)],
        Vec::new(),
        Vec::new(),
        vec![identity_term(
            "term.consumes",
            field_ref(&field),
            drop_candidate(),
        )],
    );
    for (milli, effective) in [(1000, 1), (1001, 2)] {
        let catalog = catalog(
            vec![reserving(
                task(
                    "task_reserve",
                    hourly(),
                    vec![expected(SPENT, milli)],
                    Vec::new(),
                ),
                10,
            )],
            Some(&document),
        );
        let evaluation = round(&catalog, Vec::new(), vec![reading(SPENT, 100)]);
        let task = decision(&evaluation, "task_reserve");
        assert_eq!(
            task.rank.as_ref().map(|rank| rank.effective_milli),
            Some(effective),
            "{milli}: {task:#?}"
        );
        assert_eq!(dispatched(&evaluation), ["task_reserve"], "{milli}");
    }

    let threshold = all(vec![hourly(), at_least(&key_of(SPENT), 9_000)]);
    let tied = catalog(
        vec![
            task(
                "task_b",
                threshold.clone(),
                vec![consume(SPENT, 100)],
                Vec::new(),
            ),
            task(
                "task_a",
                threshold,
                vec![expected(SPENT, 500_001)],
                Vec::new(),
            ),
        ],
        None,
    );
    let evaluation = round(&tied, Vec::new(), vec![reading(SPENT, 10_000)]);
    assert_eq!(total(&evaluation, "task_a"), total(&evaluation, "task_b"));
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    let detail = breakdown(&evaluation, "task_a");
    assert!(detail.contains(" tie=bounded chances=2 "), "{detail}");
    assert!(detail.contains(" runs_left=2 "), "{detail}");
    let other = related(&evaluation, "task_a", "task_b");
    assert!(other.contains(" tie=bounded chances=11 "), "{other}");
}

// Test 7 (§7.8 item 5): forward projection is unchanged. A task whose only expected entry is its
// consumption (scan-verified, confidence 1000) is dispatched at NOW, and the projection stops there
// with `effect_evidence_insufficient` naming it, as before for any expected effect: no step is
// applied.
#[test]
fn ss2g_t7_forward_projection_stops_at_an_expected_consumption() {
    let catalog = catalog(
        vec![task(
            "task_spend",
            hourly(),
            vec![expected(SPENT, 500)],
            Vec::new(),
        )],
        None,
    );
    let (facts, resources) = inputs(&catalog, Vec::new(), vec![reading(SPENT, 100)]);
    let projection = project_forward(
        &catalog,
        &facts,
        &resources,
        time(NOW),
        11,
        ForwardProjectionConfig::for_hours(1, 32).expect("config"),
    )
    .unwrap_or_else(|error| panic!("the projection runs: {error:?}"));
    assert_eq!(
        projection.completeness,
        ForwardProjectionCompleteness::EvidenceInsufficient,
        "{projection:#?}"
    );
    assert!(projection.steps.is_empty(), "{projection:#?}");
    assert!(
        projection.evidence_gaps.contains(&ForwardEvidenceGap {
            code: "effect_evidence_insufficient".to_owned(),
            task_id: Some("task_spend".to_owned()),
            fact_key: None,
        }),
        "{projection:#?}"
    );
}

// ---- Fixtures ----

fn hourly() -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": HOUR_MS,
            "anchor_ms": 0
        }
    })
}

fn all(predicates: Vec<Value>) -> Value {
    json!({"kind": "all", "predicates": predicates})
}

/// True while the instance fact `fact_key` holds at least `threshold`.
fn at_least(fact_key: &str, threshold: i64) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": fact_key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": threshold},
        "max_age_ms": HOUR_MS
    })
}

/// One effect on `pool` in `direction` with these quantity members.
fn effect(pool: &str, direction: &str, quantity: Value) -> Value {
    let mut entry = json!({
        "pool_id": pool,
        "direction": direction,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    });
    for (key, value) in quantity.as_object().expect("quantity members") {
        entry[key] = value.clone();
    }
    entry
}

fn consume(pool: &str, amount: u64) -> Value {
    effect(pool, "consume", json!({"amount": amount}))
}

/// SS-2g: a consumption of `milli` thousandths of a unit a run.
fn expected(pool: &str, milli: u64) -> Value {
    effect(pool, "consume", json!({"expected_amount_milli": milli}))
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", json!({"amount": amount}))
}

/// A task of the game, priority 100, on `trigger`.
fn task(id: &str, trigger: Value, consumes: Vec<Value>, produces: Vec<Value>) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
        "trigger": trigger,
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
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// `task` marked `benefit_judged` (SS-2b).
fn judged(mut task: Value) -> Value {
    task["benefit_judged"] = json!(true);
    task
}

/// `task` with a reserve of `at_least` on [`SPENT`].
fn reserving(mut task: Value, at_least: u64) -> Value {
    task["reserves"] = json!([{"pool_id": SPENT, "at_least": at_least}]);
    task
}

/// The instance fact a pool is read from.
fn key_of(pool: &str) -> String {
    format!("resource.{pool}")
}

fn pool(id: &str, capacity: u64, base_weight_milli: Option<u32>) -> Value {
    let mut pool = json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": key_of(id)},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if let Some(base_weight_milli) = base_weight_milli {
        pool["valuation"] = json!({
            "name": "Fixture",
            "unit": "unit",
            "scale": 1,
            "base_weight_milli": base_weight_milli
        });
    }
    pool
}

/// The fixture catalog, `actingcommand.scheduling.v2`, with these tasks in this order, the four
/// pools and, when given, the selection document.
fn compile(
    tasks: Vec<Value>,
    selection: Option<&Value>,
) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.expected-consumption",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-expected-consumption"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let pools = json!([
        pool(VALUED, 1_000_000, Some(1000)),
        pool(DEAR, 1_000, Some(2000)),
        pool(SPENT, 20_000, None),
        pool(OTHER, 20_000, None),
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
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: selection.map(|selection| source("selection", selection)),
    })
}

fn catalog(tasks: Vec<Value>, selection: Option<&Value>) -> CompiledCatalog {
    compile(tasks, selection).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

/// The V1 example catalog `catalog-a`, its tasks document changed by `mutate`.
fn compile_v1_example(
    mutate: impl FnOnce(&mut Value),
) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let parse = |text: &str| serde_json::from_str::<Value>(text).expect("an example document");
    let mut tasks = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ));
    mutate(&mut tasks);
    compile_catalog(&CatalogSources {
        tasks: source("v1-tasks", &tasks),
        pools: source(
            "v1-pools",
            &parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/pools.json"
            )),
        ),
        activity: source(
            "v1-activity",
            &parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/activity.json"
            )),
        ),
        timeline: source(
            "v1-timeline",
            &parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
            )),
        ),
        selection: None,
    })
}

fn refused_at(failure: &CatalogCompileFailure, code: CatalogDiagnosticCode, path: &str) -> bool {
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-expected-consumption/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// A reading of `pool`'s key on the instance, a minute before NOW.
fn reading(pool: &str, value: i64) -> ObservedFact {
    fact(A, &key_of(pool), value, NOW - MINUTE_MS, None)
}

/// A run of `task_id` on the instance, dispatched at `at`, that succeeded.
fn ran(task_id: &str, at: u64) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

/// The facts of a round after the runs `history` records, and the pools' values the Runtime
/// derives from `readings`; the instance has its own idle host.
fn inputs(
    catalog: &CompiledCatalog,
    history: Vec<TaskRuntimeSnapshot>,
    readings: Vec<ObservedFact>,
) -> (EvaluationFacts, EvaluationResources) {
    let instance = instance(A, &[]);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-expected-consumption".to_owned(),
        facts: readings,
        outcomes: Vec::new(),
        tasks: history,
        instances: vec![instance.clone()],
        priority_offsets: Vec::new(),
    };
    let resources = project_fact_pools(
        catalog,
        &facts,
        &EvaluationResources {
            pools: Vec::new(),
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
        },
    );
    (facts, resources)
}

/// One round at NOW; every candidate is admitted.
fn round(
    catalog: &CompiledCatalog,
    history: Vec<TaskRuntimeSnapshot>,
    readings: Vec<ObservedFact>,
) -> PolicyEvaluation {
    let (facts, resources) = inputs(catalog, history, readings);
    evaluate(catalog, &facts, &resources, time(NOW), 11)
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn reason<'a>(decision: &'a TaskDecision, code: &str) -> &'a DecisionReason {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == code)
        .unwrap_or_else(|| panic!("a {code} reason on {decision:#?}"))
}

/// The reason that settled a decision: its last one.
fn last_reason(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}

fn total(evaluation: &PolicyEvaluation, task_id: &str) -> i64 {
    let decision = decision(evaluation, task_id);
    decision
        .rank
        .as_ref()
        .unwrap_or_else(|| panic!("a rank on {decision:#?}"))
        .total_score
}

/// The selected candidate's `rank_breakdown` detail.
fn breakdown<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a str {
    reason(decision(evaluation, task_id), "rank_breakdown")
        .detail
        .as_str()
}

/// The detail with which `winner`'s decision record lists `other`.
fn related<'a>(evaluation: &'a PolicyEvaluation, winner: &str, other: &str) -> &'a str {
    let decision = decision(evaluation, winner);
    decision
        .reasons
        .iter()
        .find(|reason| {
            reason.outcome.code().as_str() == "decision_candidate_not_selected"
                && reason.outcome.value("task_id") == Some(&json!(other))
        })
        .map(|reason| reason.detail.as_str())
        .unwrap_or_else(|| panic!("{other} listed on {decision:#?}"))
}

/// The reason chain of the evaluation's dispatch of `task_id`, if it has one.
fn dispatched_chain<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
) -> Option<&'a DecisionReasonChain> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == A)?;
    evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
}

/// The one `policy_dispatch_parameters` of `task_id`'s dispatch chain, if it has one.
fn parameters<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> Option<&'a DecisionReason> {
    let chain = dispatched_chain(evaluation, task_id)?;
    let reasons = chain
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == PARAMETERS)
        .collect::<Vec<_>>();
    match reasons.as_slice() {
        [] => None,
        [reason] => Some(reason),
        _ => panic!("one {PARAMETERS} on the chain: {chain:#?}"),
    }
}
