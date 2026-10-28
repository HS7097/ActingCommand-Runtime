// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #427-B (plan r2 point 5, #427 comment 6460013989; approved whole by Alice 10-28 12:4x;
// `contracts/scheduling/README.md`, "Shared run limits"): the evaluator deducts no virtual CPU,
// GPU or IO for the dispatches it chooses in a round. A task whose own load exceeds its host's
// capacity is still blocked at placement. The only limits the instances share are the
// heavy-scene slots of a host and the optional `performance.pacing.max_concurrent_runs`; each
// counts the dispatches admitted or running plus those chosen earlier in the round, each
// dispatch once, and its reason carries `used`, `limit_count`, `admitted_count` and
// `round_selected_count`. Every expected value comes from the work order; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;

use super::selection_catalog::{NOW, catalog, instance};
use crate::{
    CandidateEligibility, ChainHandoverInputs, DEFAULT_HEAVY_THRESHOLD_MILLI, DecisionReason,
    DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime, FactRefreshInputs,
    FactRefreshSettings, HostResourceSnapshot, InstanceSnapshot, LoadProfile, PolicyEvaluation,
    PolicyEvaluationError, RemedyPlan, RunSlots, SchedulingDecisionState, TaskDecision,
    evaluate_with_run_slots,
};

const TASK: &str = "task.routine";
const SHARED_HOST: &str = "host.shared";
/// The fixture task's weighted load sums to 200 + 100 + 300 = 600 milli: light under the default
/// threshold (1 500), heavy on a host whose threshold is this.
const HEAVY_AT_600: u64 = 600;
const HEAVY_DEFERRED: &str = "heavy_scene_budget_deferred";
const HEAVY_EXHAUSTED: &str = "heavy_scene_budget_exhausted";
const CONCURRENT_RUNS: &str = "policy_concurrent_runs_deferred";
const CAPACITY: &str = "host_capacity_insufficient";
/// The retired reason of the in-round CPU/GPU/IO deduction.
const RETIRED_HOST_BUDGET: &str = "host_budget_deferred";
/// An admission refusal the Runtime gives (registered by runtime-host).
const HELD: &str = "dispatch_instance_held";

/// `count` available instances of the fixture game, all on the shared host.
fn instances(count: usize) -> Vec<InstanceSnapshot> {
    (1..=count)
        .map(|number| InstanceSnapshot {
            host_id: SHARED_HOST.to_owned(),
            ..instance(&format!("instance-{number}"), &[])
        })
        .collect()
}

/// The shared host at full capacity: `heavy_limit` heavy-scene slots, `active_heavy` of them
/// counted taken by the performance controller (which caps its count at the limit), and the
/// heavy threshold `threshold`.
fn host(heavy_limit: u16, active_heavy: u16, threshold: u64) -> HostResourceSnapshot {
    HostResourceSnapshot {
        host_id: SHARED_HOST.to_owned(),
        cpu_available_milli: 1_000,
        gpu_available_milli: 1_000,
        io_available_milli: 1_000,
        host_responsiveness_basis_points: 10_000,
        third_party_pressure_basis_points: 0,
        heavy_dispatch_limit: heavy_limit,
        active_heavy_dispatches: active_heavy,
        heavy_threshold_milli: threshold,
    }
}

/// The load of one admitted dispatch of the fixture task.
fn routine_load() -> LoadProfile {
    LoadProfile::Weighted {
        cpu_milli: 200,
        gpu_milli: 100,
        io_milli: 300,
    }
}

/// `count` admitted dispatches of the fixture task and the limit of concurrent runs.
fn run_slots(count: usize, max_concurrent_runs: Option<u16>) -> RunSlots {
    RunSlots {
        admitted_load_profiles: vec![routine_load(); count],
        max_concurrent_runs,
    }
}

/// The fact refresh off: the fixture task reads no fact.
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: false,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: Vec::new(),
        instance_facts_loaded_at_unix_ms: None,
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    }
}

/// One round at [`NOW`] of the fixture task on `instances`, with `run_slots` and `eligibility`.
fn evaluate(
    instances: &[InstanceSnapshot],
    hosts: Vec<HostResourceSnapshot>,
    run_slots: &RunSlots,
    eligibility: impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, PolicyEvaluationError>,
) -> PolicyEvaluation {
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-427b".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: instances.to_vec(),
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts,
    };
    evaluate_with_run_slots::<PolicyEvaluationError>(
        &catalog(&[(TASK, 100)], None),
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        run_slots,
        |_, _, _| Ok(false),
        eligibility,
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

fn eligible(_: &DispatchIntent) -> Result<CandidateEligibility, PolicyEvaluationError> {
    Ok(CandidateEligibility::Eligible)
}

/// The instances the round dispatches on.
fn chosen(evaluation: &PolicyEvaluation) -> BTreeSet<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.instance_id.as_str())
        .collect()
}

/// The decisions of the instances the round does not dispatch on.
fn left(evaluation: &PolicyEvaluation) -> Vec<&TaskDecision> {
    let chosen = chosen(evaluation);
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision
                .instance_id
                .as_deref()
                .is_some_and(|instance_id| !chosen.contains(instance_id))
        })
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, instance_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.instance_id.as_deref() == Some(instance_id))
        .unwrap_or_else(|| panic!("a decision on {instance_id}: {evaluation:#?}"))
}

fn last(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason: {decision:#?}"))
}

/// The reason's code and its four numbers: `used`, `limit_count`, `admitted_count`,
/// `round_selected_count`.
fn slots(reason: &DecisionReason) -> (&str, [i64; 4]) {
    let values = reason.outcome.values();
    let number = |key: &str| {
        values
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("{key} in {values:?}"))
    };
    (
        reason.outcome.code().as_str(),
        [
            number("used"),
            number("limit_count"),
            number("admitted_count"),
            number("round_selected_count"),
        ],
    )
}

/// No decision of the round carries `code`.
fn nowhere(evaluation: &PolicyEvaluation, code: &str) -> bool {
    evaluation
        .decisions
        .iter()
        .flat_map(|decision| &decision.reasons)
        .all(|reason| reason.outcome.code().as_str() != code)
}

/// Five instances of one host, each with the ordinary load (200, 100, 300): together they need
/// 1 500 of the host's 1 000 io, which the round's deduction used to run out of after three
/// (the fourth `host_budget_deferred`). Now the round deducts nothing: all five are chosen.
#[test]
fn gate_427b_five_instances_with_ordinary_loads_are_all_chosen_in_one_round() {
    let evaluation = evaluate(
        &instances(5),
        vec![host(2, 0, DEFAULT_HEAVY_THRESHOLD_MILLI)],
        &RunSlots::default(),
        eligible,
    );
    assert_eq!(
        chosen(&evaluation),
        BTreeSet::from([
            "instance-1",
            "instance-2",
            "instance-3",
            "instance-4",
            "instance-5"
        ]),
        "{evaluation:#?}"
    );
    assert!(nowhere(&evaluation, RETIRED_HOST_BUDGET));
}

/// The placement check stays: on a host with 150 cpu the task's own 200 does not fit, so its
/// instance is blocked `host_capacity_insufficient`; the instance on the full host runs.
#[test]
fn gate_427b_a_task_whose_own_load_exceeds_its_host_is_still_blocked_at_placement() {
    let mut small = host(2, 0, DEFAULT_HEAVY_THRESHOLD_MILLI);
    small.host_id = "host.small".to_owned();
    small.cpu_available_milli = 150;
    let mut on_small = instances(2);
    on_small[0].host_id = "host.small".to_owned();
    let evaluation = evaluate(
        &on_small,
        vec![small, host(2, 0, DEFAULT_HEAVY_THRESHOLD_MILLI)],
        &RunSlots::default(),
        eligible,
    );
    assert_eq!(chosen(&evaluation), BTreeSet::from(["instance-2"]));
    let blocked = decision(&evaluation, "instance-1");
    assert_eq!(blocked.state, SchedulingDecisionState::Blocked);
    assert_eq!(last(blocked).outcome.code().as_str(), CAPACITY);
}

/// Two heavy-scene slots, three heavy candidates: two are chosen and the third waits
/// `heavy_scene_budget_deferred` with 2 of 2 used, 0 admitted, 2 chosen earlier in the round.
#[test]
fn gate_427b_two_heavy_are_chosen_and_the_third_waits_with_the_numbers() {
    let evaluation = evaluate(
        &instances(3),
        vec![host(2, 0, HEAVY_AT_600)],
        &RunSlots::default(),
        eligible,
    );
    assert_eq!(chosen(&evaluation).len(), 2, "{evaluation:#?}");
    let waiting = left(&evaluation);
    let [waiting] = waiting.as_slice() else {
        panic!("one instance left: {evaluation:#?}");
    };
    assert_eq!(slots(last(waiting)), (HEAVY_DEFERRED, [2, 2, 0, 2]));
}

/// A heavy dispatch already admitted takes a slot: the controller counts it in the snapshot
/// (`active_heavy_dispatches` 1) and the run slots list it, and it counts once. One more heavy
/// candidate is chosen; the other waits with 2 of 2 used, 1 admitted and 1 chosen earlier.
#[test]
fn gate_427b_an_admitted_heavy_dispatch_takes_a_slot_once() {
    let evaluation = evaluate(
        &instances(2),
        vec![host(2, 1, HEAVY_AT_600)],
        &run_slots(1, None),
        eligible,
    );
    assert_eq!(chosen(&evaluation).len(), 1, "{evaluation:#?}");
    let waiting = left(&evaluation);
    let [waiting] = waiting.as_slice() else {
        panic!("one instance left: {evaluation:#?}");
    };
    assert_eq!(slots(last(waiting)), (HEAVY_DEFERRED, [2, 2, 1, 1]));
}

/// A heavy candidate the admission refuses takes no slot: the first-ranked instance (its
/// affinity puts it first) is held, and the two others take the two slots.
#[test]
fn gate_427b_a_refused_heavy_candidate_frees_its_slot() {
    let mut three = instances(3);
    three[0].preferred_task_ids = vec![TASK.to_owned()];
    let mut asked = Vec::new();
    let evaluation = evaluate(
        &three,
        vec![host(2, 0, HEAVY_AT_600)],
        &RunSlots::default(),
        |intent: &DispatchIntent| {
            asked.push(intent.instance_id.clone());
            Ok(if intent.instance_id == "instance-1" {
                CandidateEligibility::Deferred {
                    reason: DecisionReason {
                        outcome: Outcome::relayed(HELD),
                        detail: "held by the fixture".to_owned(),
                    },
                    next_wake_unix_ms: None,
                }
            } else {
                CandidateEligibility::Eligible
            })
        },
    );
    assert_eq!(asked.first().map(String::as_str), Some("instance-1"));
    assert_eq!(
        chosen(&evaluation),
        BTreeSet::from(["instance-2", "instance-3"]),
        "{evaluation:#?}"
    );
    assert_eq!(
        last(decision(&evaluation, "instance-1"))
            .outcome
            .code()
            .as_str(),
        HELD
    );
}

/// Under pressure the limit falls to `.paused` (1) while three heavy dispatches still run; the
/// controller's snapshot caps its count at the limit, and the run slots carry the real number:
/// every heavy candidate is blocked at placement with 3 used of 1, 3 admitted, 0 chosen.
#[test]
fn gate_427b_under_pressure_the_reason_says_the_real_number_running() {
    let evaluation = evaluate(
        &instances(2),
        vec![host(1, 1, HEAVY_AT_600)],
        &run_slots(3, None),
        eligible,
    );
    assert!(chosen(&evaluation).is_empty(), "{evaluation:#?}");
    for blocked in left(&evaluation) {
        assert_eq!(blocked.state, SchedulingDecisionState::Blocked);
        assert_eq!(slots(last(blocked)), (HEAVY_EXHAUSTED, [3, 1, 3, 0]));
    }
}

/// Without `max_concurrent_runs` there is no limit: four light dispatches run and all five
/// instances are still chosen.
#[test]
fn gate_427b_without_the_max_key_there_is_no_limit_of_concurrent_runs() {
    let evaluation = evaluate(
        &instances(5),
        vec![host(2, 0, DEFAULT_HEAVY_THRESHOLD_MILLI)],
        &run_slots(4, None),
        eligible,
    );
    assert_eq!(chosen(&evaluation).len(), 5, "{evaluation:#?}");
    assert!(nowhere(&evaluation, CONCURRENT_RUNS));
}

/// `max_concurrent_runs` 3: with none running, three are chosen and the fourth and fifth wait
/// `policy_concurrent_runs_deferred` with 3 of 3 used, 0 admitted, 3 chosen earlier; with two
/// running, one is chosen and the rest wait with 3 of 3, 2 admitted, 1 chosen earlier.
#[test]
fn gate_427b_max_three_defers_the_fourth_with_the_numbers() {
    for (running, expected_chosen, numbers) in [(0, 3, [3, 3, 0, 3]), (2, 1, [3, 3, 2, 1])] {
        let evaluation = evaluate(
            &instances(5),
            vec![host(2, 0, DEFAULT_HEAVY_THRESHOLD_MILLI)],
            &run_slots(running, Some(3)),
            eligible,
        );
        assert_eq!(
            chosen(&evaluation).len(),
            expected_chosen,
            "{running} running: {evaluation:#?}"
        );
        let waiting = left(&evaluation);
        assert_eq!(waiting.len(), 5 - expected_chosen);
        for decision in waiting {
            assert_eq!(slots(last(decision)), (CONCURRENT_RUNS, numbers));
        }
    }
}

/// `max_concurrent_runs` 1 runs one policy dispatch at a time across every instance: one is
/// chosen while none runs; none while it runs (1 of 1, admitted); one again once it settled.
#[test]
fn gate_427b_max_one_runs_one_dispatch_at_a_time() {
    for (running, expected_chosen, numbers) in [
        (0, 1, [1, 1, 0, 1]),
        (1, 0, [1, 1, 1, 0]),
        (0, 1, [1, 1, 0, 1]),
    ] {
        let evaluation = evaluate(
            &instances(5),
            vec![host(2, 0, DEFAULT_HEAVY_THRESHOLD_MILLI)],
            &run_slots(running, Some(1)),
            eligible,
        );
        assert_eq!(
            chosen(&evaluation).len(),
            expected_chosen,
            "{running} running: {evaluation:#?}"
        );
        for decision in left(&evaluation) {
            assert_eq!(slots(last(decision)), (CONCURRENT_RUNS, numbers));
        }
    }
}
