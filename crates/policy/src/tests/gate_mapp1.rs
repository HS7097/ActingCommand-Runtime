// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #437 MAPP-1 (register §1.323, §1.328; `contracts/device-group.md`, "One pick per
// device per round"): the evaluator takes the device each instance runs on as an argument of its
// own (`DeviceGroups`), and a round selects at most one candidate per device. The device's first
// candidate in the allocation order wins it; every later candidate of another member of the
// device is deferred `device_group_waiting` (`device_group_wait` `selected`, `alias` the winner,
// `task_id` its task) with no wake, where a lone instance's waits `instance_already_selected`.
// Members rank by today's scores, unchanged. The load refuses every group of several until the
// slice "order within one device", so the test hands the evaluator the map as the Runtime builds
// it from the assembly's member → device key map.
//
// Two members A and B of one device and an instance L alone: P (priority 900) and Q (100) on A,
// R (500) on B and Z (300) on L, every task hourly and due at NOW. Every expected value comes from
// the contract; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::codes::DeviceGroupWait;
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DeviceGroups, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactRefreshInputs, FactRefreshSettings, HostResourceSnapshot, InstanceHolds,
    InstancePoolLines, LeftOutFacts, NoProgressGuards, OwedReruns, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, RemedyPlan, RunSlots, SchedulingDecisionState, TaskDecision,
    compile_catalog, evaluate_with_device_groups, evaluate_with_left_out_facts,
};

const A: &str = "neutral.a";
const B: &str = "neutral.b";
const L: &str = "neutral.c";
const P: &str = "task.first";
const Q: &str = "task.second";
const R: &str = "task.other";
const Z: &str = "task.lone";
/// The device key the Runtime gives A and B (the text of the smallest member id).
const DEVICE: &str = "device.key";

#[test]
fn mapp1_one_pick_per_device_the_other_member_waits_device_group_waiting_selected() {
    let groups = device(&[A, B]);
    let plain = round(500, None);
    let grouped = round(500, Some(&groups));
    // Without the groups each instance is a device of its own: P, R and Z run.
    assert_eq!(dispatched(&plain), vec![(P, A), (R, B), (Z, L)]);
    // With them A and B share one device: P, first in the allocation order, wins it.
    assert_eq!(dispatched(&grouped), vec![(P, A), (Z, L)]);
    let waiting = decision(&grouped, R, B);
    assert_eq!(waiting.state, SchedulingDecisionState::Deferred);
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
            ("task_id", json!(P)),
        ],
    );
    // A later candidate of the winning member keeps the code a lone instance's gets; the waiting
    // member has no wake of its own, as that candidate has none.
    let behind = decision(&grouped, Q, A);
    assert_eq!(waiting.next_wake_unix_ms, behind.next_wake_unix_ms);
    assert!(
        behind
            .reasons
            .last()
            .is_some_and(|reason| reason.outcome.is(PolicyCode::InstanceAlreadySelected)),
        "{:?}",
        behind.reasons
    );
    // The instance alone on its device decides as without the groups.
    assert_eq!(decision(&grouped, Z, L), decision(&plain, Z, L));
}

#[test]
fn mapp1_the_device_goes_to_its_first_candidate_whichever_member_it_serves() {
    let groups = device(&[A, B]);
    // R outranks P now: it wins the device, and both of A's candidates wait for it.
    let grouped = round(1000, Some(&groups));
    assert_eq!(dispatched(&grouped), vec![(R, B), (Z, L)]);
    for task in [P, Q] {
        let waiting = decision(&grouped, task, A);
        assert_eq!(waiting.state, SchedulingDecisionState::Deferred, "{task}");
        let reason = waiting.reasons.last().expect("a reason");
        assert!(reason.outcome.is(PolicyCode::DeviceGroupWaiting), "{task}");
        assert_values(
            reason.outcome.values(),
            &[
                ("instance_id", json!(A)),
                ("alias", json!(B)),
                (
                    "device_group_wait",
                    json!(DeviceGroupWait::Selected.as_str()),
                ),
                ("task_id", json!(R)),
            ],
        );
    }
}

#[test]
fn mapp1_without_a_device_of_several_the_evaluation_is_byte_for_byte_the_one_before() {
    // No groups, and a map that names an instance alone on its device, give the evaluation of
    // the entry point before MAPP-1, byte for byte and with every decision's wake.
    let plain = round(500, None);
    for groups in [DeviceGroups::default(), device(&[L])] {
        let grouped = round(500, Some(&groups));
        assert_eq!(bytes(&grouped), bytes(&plain), "{groups:?}");
        assert_eq!(grouped, plain, "{groups:?}");
    }
}

// ---- The fixture

/// The devices of `members`, one device key for all of them.
fn device(members: &[&str]) -> DeviceGroups {
    DeviceGroups::new(
        members
            .iter()
            .map(|member| ((*member).to_owned(), DEVICE.to_owned()))
            .collect(),
    )
}

/// One round at [`NOW`] with R's priority `r_priority`; `None` evaluates through the entry
/// point before MAPP-1 (`evaluate_with_left_out_facts`).
fn round(r_priority: i16, groups: Option<&DeviceGroups>) -> PolicyEvaluation {
    let catalog = catalog(r_priority);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-mapp1".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[]), instance(B, &[]), instance(L, &[])],
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
    let refresh = refresh_inputs();
    let eligible = |_: &DispatchIntent| Ok(CandidateEligibility::Eligible);
    let evaluated = match groups {
        None => evaluate_with_left_out_facts::<PolicyEvaluationError>(
            &catalog,
            &facts,
            &resources,
            time,
            11,
            &BTreeSet::new(),
            &refresh,
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut CappedMemory::default(),
            &InstancePoolLines::default(),
            &LeftOutFacts::default(),
            |_, _, _| Ok(false),
            eligible,
        ),
        Some(groups) => evaluate_with_device_groups::<PolicyEvaluationError>(
            &catalog,
            &facts,
            &resources,
            time,
            11,
            &BTreeSet::new(),
            &refresh,
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut CappedMemory::default(),
            &InstancePoolLines::default(),
            &LeftOutFacts::default(),
            groups,
            |_, _, _| Ok(false),
            eligible,
        ),
    };
    evaluated.unwrap_or_else(|error| panic!("the round evaluates: {error}"))
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

/// The catalog: P and Q on A, R on B with `r_priority`, Z on L; every task hourly, due at
/// [`NOW`].
fn catalog(r_priority: i16) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.mapp1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-mapp1"]
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
        task(P, A, 900),
        task(Q, A, 100),
        task(R, B, r_priority),
        task(Z, L, 300),
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

/// One hourly task of `instance_id` with its priority; its feedback stop stays false.
fn task(task_id: &str, instance_id: &str, priority: i16) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "instance", "instance_id": instance_id},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": format!("procedure.{task_id}"),
        "priority": priority,
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

fn bytes(evaluation: &PolicyEvaluation) -> Vec<u8> {
    serde_json::to_vec(evaluation).expect("the evaluation serializes")
}

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
