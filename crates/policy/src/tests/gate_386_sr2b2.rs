// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-2b-2 (design `rd-1028/design-p22.md` §6.2 and §8 row SR-2b-2, test ①;
// `contracts/policy-suspension.md`, "The rerun"): an owed rerun the Runtime hands the evaluation
// is treated as triggered and past its task cooldown and carries the Runtime's reason
// `policy_rerun_owed`. With `promote` it ranks ahead of its instance's other candidates and is
// the instance's dispatch; without it the pair still triggers and is a candidate, but keeps its
// own rank, so a higher-ranked candidate of the instance wins. Every expected value comes from
// the work order; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Code, Outcome};

use super::selection_catalog::{NOW, catalog, instance};
use crate::{
    CandidateEligibility, ChainHandoverInputs, DecisionReason, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactRefreshInputs, FactRefreshSettings,
    HostResourceSnapshot, OwedRerun, OwedReruns, PolicyEvaluation, PolicyEvaluationError,
    RemedyPlan, RunSlots, TaskDecision, TaskRuntimeSnapshot, evaluate_with_owed_reruns,
};

const INSTANCE: &str = "instance-1";
/// The higher-ranked task of the instance.
const HIGH: &str = "task.high";
/// The owed task: lower ranked, dispatched 500 ms ago (its cooldown is 1 000 ms), so without the
/// owed rerun it waits for its next hourly occurrence and its cooldown.
const OWED: &str = "task.owed";
const OWED_CODE: &str = "policy_rerun_owed";
const ALREADY_SELECTED: &str = "instance_already_selected";

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

/// The reason the Runtime writes for an owed rerun (a code runtime-host registers).
fn owed_reason() -> DecisionReason {
    DecisionReason {
        outcome: Outcome::new(Code::relayed(OWED_CODE).expect("the code is registered"))
            .task_id(OWED)
            .instance_id(INSTANCE)
            .count(1_u32),
        detail: "a rerun is owed".to_owned(),
    }
}

/// One round at [`NOW`] of both tasks on the instance, the owed task dispatched 500 ms ago, with
/// `owed` as the owed reruns and every candidate eligible.
fn evaluate(owed: &OwedReruns) -> PolicyEvaluation {
    let instance = instance(INSTANCE, &[]);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-sr2b2".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: vec![TaskRuntimeSnapshot {
            task_id: OWED.to_owned(),
            instance_id: INSTANCE.to_owned(),
            last_dispatched_unix_ms: Some(NOW - 500),
            eligible_since_unix_ms: None,
            terminal_state: None,
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
            expected_runtime_ms: None,
            expected_runtime_runs: None,
        }],
        instances: vec![instance.clone()],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: instance.host_id.clone(),
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
    evaluate_with_owed_reruns::<PolicyEvaluationError>(
        &catalog(&[(HIGH, 900), (OWED, 100)], None),
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
        &RunSlots::default(),
        owed,
        |_, _, _| Ok(false),
        |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

fn owed(promote: bool) -> OwedReruns {
    OwedReruns {
        pairs: BTreeMap::from([(
            (OWED.to_owned(), INSTANCE.to_owned()),
            OwedRerun {
                reason: owed_reason(),
                promote,
            },
        )]),
    }
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id)
        .unwrap_or_else(|| panic!("a decision of {task_id}: {evaluation:#?}"))
}

fn has(decision: &TaskDecision, code: &str) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| reason.outcome.code().as_str() == code)
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

// ---- ①

// Without an owed rerun the higher-ranked task is the instance's dispatch and the owed task is
// no candidate (its occurrence was served and its cooldown runs). With an owed rerun that
// promotes, the owed task is triggered past its cooldown, ranks first with `policy_rerun_owed`
// and is dispatched; the higher-ranked task waits as `instance_already_selected`. With
// `promote = false` the owed task still triggers past its cooldown and is a candidate with
// `policy_rerun_owed`, but is not moved ahead: the higher-ranked task is dispatched.
#[test]
fn sr2b2_t1_an_owed_rerun_ranks_first_with_policy_rerun_owed_and_promote_false_keeps_its_rank() {
    let plain = evaluate(&OwedReruns::default());
    assert_eq!(dispatched(&plain), vec![HIGH], "{plain:#?}");
    let owed_task = decision(&plain, OWED);
    assert!(!has(owed_task, OWED_CODE), "{owed_task:#?}");
    assert!(
        !has(owed_task, ALREADY_SELECTED),
        "no candidate: {owed_task:#?}"
    );

    let promoted = evaluate(&owed(true));
    assert_eq!(dispatched(&promoted), vec![OWED], "{promoted:#?}");
    let owed_task = decision(&promoted, OWED);
    assert!(has(owed_task, OWED_CODE), "{owed_task:#?}");
    let reason = owed_task
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == OWED_CODE)
        .expect("the owed reason");
    assert_eq!(reason, &owed_reason());
    let high = decision(&promoted, HIGH);
    assert!(has(high, ALREADY_SELECTED), "{high:#?}");

    let kept = evaluate(&owed(false));
    assert_eq!(dispatched(&kept), vec![HIGH], "{kept:#?}");
    let owed_task = decision(&kept, OWED);
    assert!(has(owed_task, OWED_CODE), "{owed_task:#?}");
    assert!(
        has(owed_task, ALREADY_SELECTED),
        "a candidate behind the higher-ranked task: {owed_task:#?}"
    );
}
