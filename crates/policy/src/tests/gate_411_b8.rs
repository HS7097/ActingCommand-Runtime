// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B8 gate tests, policy half (plan `rd-1021/train-0129/b-plan/plan.md` §2.8 and
// §5.3 BT-B8 T1, T3; `contracts/remedies.md` section 4; policy gate key 58; register §1.146): a
// V2 catalog task that opts in with `dispatch_parameters: true` hands its run the schedule values
// of its dispatch on `policy_dispatch_parameters`, with or without remedies; a catalog without the
// opt-in keeps its hash and its chains. Every expected value comes from §2.8 and the draft it
// quotes ("是不是本时段最后一轮、离下一轮多久、今天还剩几次"; "「最后一轮」两种算法都给": the last clock
// occurrence in the activity window, and the last of today's budget).
//
// The catalog is neutral: one task on one instance, triggered by a UTC `daily` clock at 08:00,
// 12:00 and 20:00, in an activity window of 07:00-21:00 UTC on every weekday. The Runtime's
// admission preview is the eligibility callback, which answers how many runs the daily budgets
// still allow after the dispatch.

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, instance};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, CompiledCatalog,
    DecisionReasonChain, EvaluationFacts, EvaluationResources, EvaluationTime,
    HostResourceSnapshot, PolicyEvaluation, PolicyEvaluationError, compile_catalog,
    evaluate_with_eligibility,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const TASK: &str = "t_rounds";
const PARAMETERS: &str = "policy_dispatch_parameters";
/// A day of the fixture at 00:00 UTC.
const DAY: u64 = 20_000 * DAY_MS;
/// The clock's occurrences of that day.
const NOON: u64 = DAY + 12 * HOUR_MS;
const EVENING: u64 = DAY + 20 * HOUR_MS;
/// The next day's first occurrence.
const NEXT_MORNING: u64 = DAY + DAY_MS + 8 * HOUR_MS;
/// The end of that day's activity window, 21:00 UTC.
const WINDOW_END: u64 = DAY + 21 * HOUR_MS;

// §2.8 and T1: a task that opts in, evaluated without remedies, carries on its winning dispatch
// the five values: the next occurrence of its clock, the end of its activity window, whether no
// occurrence is left before that end (the last round of the window), how many runs the daily
// budgets still allow after this one, and whether that is none (the last of today's budget). The
// two "last" definitions are independent: the evening round is the window's last with budget
// left, and a noon round with the budget spent is the day's last with rounds left in the window.
#[test]
fn b8_t1_an_opted_in_dispatch_carries_the_five_schedule_values_without_remedies() {
    let catalog = catalog(Some(true));
    for (case, at, daily_remaining, expected) in [
        (
            "noon, budget left",
            NOON + 30_000,
            3,
            json!({
                "next_occurrence_unix_ms": EVENING,
                "window_end_unix_ms": WINDOW_END,
                "last_in_window": false,
                "last_daily_slot": false,
                "daily_remaining": 3
            }),
        ),
        (
            "evening: the last round of the window",
            EVENING + 30_000,
            2,
            json!({
                "next_occurrence_unix_ms": NEXT_MORNING,
                "window_end_unix_ms": WINDOW_END,
                "last_in_window": true,
                "last_daily_slot": false,
                "daily_remaining": 2
            }),
        ),
        (
            "noon: the last of today's budget",
            NOON + 30_000,
            0,
            json!({
                "next_occurrence_unix_ms": EVENING,
                "window_end_unix_ms": WINDOW_END,
                "last_in_window": false,
                "last_daily_slot": true,
                "daily_remaining": 0
            }),
        ),
    ] {
        let evaluation = evaluate(&catalog, at, daily_remaining);
        let chain = dispatched_chain(&evaluation)
            .unwrap_or_else(|| panic!("{case}: a dispatch of {TASK}: {evaluation:#?}"));
        let reasons = chain
            .reasons
            .iter()
            .filter(|reason| reason.outcome.code().as_str() == PARAMETERS)
            .collect::<Vec<_>>();
        let [reason] = reasons.as_slice() else {
            panic!("{case}: one {PARAMETERS} on the dispatch's chain: {chain:#?}");
        };
        let carried = [
            "next_occurrence_unix_ms",
            "window_end_unix_ms",
            "last_in_window",
            "last_daily_slot",
            "daily_remaining",
        ]
        .into_iter()
        .map(|key| {
            (
                key.to_owned(),
                reason.outcome.value(key).cloned().unwrap_or(Value::Null),
            )
        })
        .collect::<serde_json::Map<_, _>>();
        assert_eq!(Value::Object(carried), expected, "{case}: {reason:?}");
        assert!(
            reason.outcome.value("run_parameters").is_none(),
            "{case}: no remedy plan, no package parameters: {reason:?}"
        );
    }
}

// §2.8 and T3: a catalog without the opt-in keeps its hash and its chains. The neutral example
// catalog declared as V2 keeps the golden hash R7-T6 and K5-T7 computed for it; the fixture task
// without the member, evaluated as the opted-in one is, carries no `policy_dispatch_parameters`.
#[test]
fn b8_t3_a_catalog_without_the_opt_in_keeps_its_hash_and_its_chains() {
    let neutral = compile_catalog(&neutral_v2_sources())
        .unwrap_or_else(|failure| panic!("the V2 catalog compiles: {:?}", failure.diagnostics()));
    assert_eq!(
        neutral.catalog_hash(),
        "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047"
    );

    let catalog = catalog(None);
    let evaluation = evaluate(&catalog, NOON + 30_000, 3);
    let chain = dispatched_chain(&evaluation)
        .unwrap_or_else(|| panic!("a dispatch of {TASK}: {evaluation:#?}"));
    assert!(
        chain
            .reasons
            .iter()
            .all(|reason| reason.outcome.code().as_str() != PARAMETERS),
        "{chain:#?}"
    );
}

// ---- Fixture.

/// The fixture catalog, `actingcommand.scheduling.v2`, its one task with `dispatch_parameters`
/// set to `opt_in` when given.
fn catalog(opt_in: Option<bool>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.dispatch-schedule",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-dispatch-schedule"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let mut task = json!({
        "id": TASK,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{TASK}"),
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "daily",
                "clock_source": {
                    "kind": "server",
                    "timezone_id": "etc/utc",
                    "utc_offset_minutes": 0,
                    "dst_offset_minutes": 0,
                    "maintenance_drift_ms": 0
                },
                "minutes_of_day": [480, 720, 1200]
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
    });
    if let Some(opt_in) = opt_in {
        task["dispatch_parameters"] = json!(opt_in);
    }
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 420,
            "end_minute_of_day": 1260
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
        tasks: source("tasks", &document("tasks", json!([task]))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// The neutral example catalog declared as `actingcommand.scheduling.v2`, each timeline event
/// with its explicit unbounded validity, as R7-T6 declares it.
fn neutral_v2_sources() -> CatalogSources {
    let mut sources = CatalogSources {
        tasks: CatalogDocumentSource::new(
            "memory://catalog-a/tasks.json",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/tasks.json")
                .to_vec(),
        ),
        pools: CatalogDocumentSource::new(
            "memory://catalog-a/pools.json",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/pools.json")
                .to_vec(),
        ),
        activity: CatalogDocumentSource::new(
            "memory://catalog-a/activity.json",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/activity.json")
                .to_vec(),
        ),
        timeline: CatalogDocumentSource::new(
            "memory://catalog-a/timeline.json",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/timeline.json")
                .to_vec(),
        ),
        selection: None,
    };
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value = serde_json::from_slice(&source.bytes).expect("neutral document");
        document["schema_version"] = json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        source.bytes = serde_json::to_vec(&document).expect("document bytes");
    }
    sources
}

/// Evaluates the fixture at `at` without remedies; the admission preview admits every candidate
/// with `daily_remaining` runs left.
fn evaluate(catalog: &CompiledCatalog, at: u64, daily_remaining: u32) -> PolicyEvaluation {
    let instance = instance(A, &[]);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-dispatch-schedule".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance.clone()],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
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
    };
    evaluate_with_eligibility::<PolicyEvaluationError>(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: at,
            monotonic_ms: at,
        },
        11,
        |_| Ok(CandidateEligibility::EligibleWithDailyRemaining { daily_remaining }),
    )
    .unwrap_or_else(|error| panic!("the evaluation of valid inputs: {error}"))
}

/// The reason chain of the evaluation's dispatch of the fixture task, if it has one.
fn dispatched_chain(evaluation: &PolicyEvaluation) -> Option<&DecisionReasonChain> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == A)?;
    evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
}
