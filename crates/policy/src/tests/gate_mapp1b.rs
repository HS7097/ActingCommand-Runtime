// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #437 MAPP-1b (register §1.328, point 8, and §1.330; `contracts/device-group.md`,
// "Never interrupted"): a chain handover's successor goes before the candidates of its device's
// other members too. The successor of a handover is allocated before every candidate that
// outranked it on its device, so the device's other members wait behind it as candidates ranked
// after it do: `device_group_waiting` `selected`, with the successor's member and task. Without
// a handover the device goes to its first candidate in the allocation order, as MAPP-1a built
// it; an instance alone on its device decides as before (`gate_411_b41`).
//
// Two members A and B of one device: on A a chain whose predecessor LEAD ran just before the
// round and ended on the page A rests on, and its successor CLOSE (priority 100), which starts on
// RESULT; on B the task OTHER (priority 500), due at NOW. The load refuses every group of several
// until the slice "order within one device", so the test hands the evaluator the map as the
// Runtime builds it. Every expected value comes from the contract; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::codes::DeviceGroupWait;
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, CompletedActivityWindow, DeviceGroups, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactRefreshInputs, FactRefreshSettings,
    HostResourceSnapshot, InstanceHolds, InstancePoolLines, LeftOutFacts, NoProgressGuards,
    OwedReruns, PolicyCode, PolicyEvaluation, PolicyEvaluationError, RemedyPlan, RestingPage,
    RunSlots, TaskDecision, TaskRuntimeSnapshot, TaskTerminalState, compile_catalog,
    evaluate_with_device_groups,
};

const A: &str = "neutral.a";
const B: &str = "neutral.b";
const LEAD: &str = "task.lead";
const CLOSE: &str = "task.close";
const OTHER: &str = "task.other";
/// The successor's entry page, and a page outside its entry page set.
const RESULT: &str = "neutral/result";
const HOME: &str = "neutral/home";
/// The device key the Runtime gives A and B (the text of the smallest member id).
const DEVICE: &str = "device.key";

#[test]
fn mapp1b_a_chain_successor_goes_before_the_other_members_of_its_device() {
    let groups = DeviceGroups::new(BTreeMap::from([
        (A.to_owned(), DEVICE.to_owned()),
        (B.to_owned(), DEVICE.to_owned()),
    ]));
    // Without the groups each instance is a device of its own: the successor and OTHER both run.
    let plain = round(RESULT, &DeviceGroups::default());
    assert_eq!(dispatched(&plain), vec![(CLOSE, A), (OTHER, B)]);

    // With them A and B share one device. OTHER outranks CLOSE, but CLOSE hands over on the page
    // A rests on: it goes first and wins the device, and OTHER waits behind it.
    let grouped = round(RESULT, &groups);
    assert_eq!(dispatched(&grouped), vec![(CLOSE, A)]);
    let waiting = decision(&grouped, OTHER, B);
    let reason = waiting.reasons.last().expect("a reason");
    assert!(
        reason.outcome.is(PolicyCode::DeviceGroupWaiting),
        "{:?}",
        waiting.reasons
    );
    assert_values(
        reason.outcome.values(),
        &[
            ("instance_id", json!(B)),
            ("alias", json!(A)),
            (
                "device_group_wait",
                json!(DeviceGroupWait::Selected.as_str()),
            ),
            ("task_id", json!(CLOSE)),
        ],
    );

    // Without a handover (A rests on a page outside CLOSE's entry page set) the device goes to
    // its first candidate in the allocation order, OTHER, and CLOSE waits for it.
    let ordered = round(HOME, &groups);
    assert_eq!(dispatched(&ordered), vec![(OTHER, B)]);
    let behind = decision(&ordered, CLOSE, A);
    let reason = behind.reasons.last().expect("a reason");
    assert!(
        reason.outcome.is(PolicyCode::DeviceGroupWaiting),
        "{:?}",
        behind.reasons
    );
    assert_values(
        reason.outcome.values(),
        &[
            ("instance_id", json!(A)),
            ("alias", json!(B)),
            (
                "device_group_wait",
                json!(DeviceGroupWait::Selected.as_str()),
            ),
            ("task_id", json!(OTHER)),
        ],
    );
}

// ---- The fixture

/// One round at [`NOW`], A resting on `page_id`, recorded during LEAD's run.
fn round(page_id: &str, groups: &DeviceGroups) -> PolicyEvaluation {
    let catalog = catalog();
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-mapp1b".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: vec![lead_state()],
        instances: vec![instance(A, &[]), instance(B, &[])],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: facts
            .instances
            .iter()
            .map(|instance| HostResourceSnapshot {
                host_id: instance.host_id.clone(),
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
    };
    let time = EvaluationTime {
        unix_ms: NOW,
        monotonic_ms: NOW,
    };
    let chain = ChainHandoverInputs {
        resting_pages: BTreeMap::from([(
            A.to_owned(),
            RestingPage {
                page_id: page_id.to_owned(),
                observed_at_unix_ms: NOW - 180_000,
            },
        )]),
        successor_wait_codes: Vec::new(),
    };
    evaluate_with_device_groups::<PolicyEvaluationError>(
        &catalog,
        &facts,
        &resources,
        time,
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &chain,
        &RunSlots::default(),
        &OwedReruns::default(),
        &InstanceHolds::default(),
        &NoProgressGuards::default(),
        &mut CappedMemory::default(),
        &InstancePoolLines::default(),
        &LeftOutFacts::default(),
        groups,
        |task, _, page_id| Ok(task.id == CLOSE && page_id == RESULT),
        |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

/// LEAD's latest run on A: admitted ten minutes before the round, succeeded two minutes before
/// it.
fn lead_state() -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: LEAD.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(NOW - 600_000),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: Some(CompletedActivityWindow {
            window_id: "window.fixture".to_owned(),
            completed_at_unix_ms: NOW - 120_000,
        }),
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

/// The fact refresh at its starting defaults, with no reader.
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: Vec::new(),
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    }
}

/// The catalog: LEAD (in its cooldown after its run) and its successor CLOSE on A, OTHER on B.
fn catalog() -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.mapp1b",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-mapp1b"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let tasks = json!([
        task(LEAD, A, 200, hourly(), 2 * HOUR_MS),
        task(
            CLOSE,
            A,
            100,
            json!({"kind": "dependency_completed", "task_id": LEAD, "terminal_states": ["succeeded"]}),
            HOUR_MS,
        ),
        task(OTHER, B, 500, hourly(), 1000),
    ]);
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile()]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

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

/// One task of `instance_id` with its priority, trigger and cooldown; its feedback stop stays
/// false.
fn task(
    task_id: &str,
    instance_id: &str,
    priority: i16,
    trigger: Value,
    cooldown_ms: u64,
) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "instance", "instance_id": instance_id},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": format!("procedure.{task_id}"),
        "priority": priority,
        "trigger": trigger,
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
        "cooldown_ms": cooldown_ms,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// One activity profile of the fixture's scope, open all day on every weekday.
fn profile() -> Value {
    json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": "game-a"},
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

// ---- Reading the round

fn decision<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
    instance_id: &str,
) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(instance_id)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {instance_id}: {evaluation:#?}"))
}

/// The (task, instance) of every dispatch, in instance order.
fn dispatched(evaluation: &PolicyEvaluation) -> Vec<(&str, &str)> {
    let mut dispatched = evaluation
        .dispatch_intents
        .iter()
        .map(|intent| (intent.task_id.as_str(), intent.instance_id.as_str()))
        .collect::<Vec<_>>();
    dispatched.sort_by(|left, right| left.1.cmp(right.1));
    dispatched
}

/// The reason's values are exactly these.
fn assert_values(values: &Map<String, Value>, expected: &[(&str, Value)]) {
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<Map<String, Value>>();
    assert_eq!(values, &expected);
}
