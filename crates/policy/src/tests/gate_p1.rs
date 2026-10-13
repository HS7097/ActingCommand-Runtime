// SPDX-License-Identifier: AGPL-3.0-only

//! P-1 gate tests of the policy crate (test plan v2.2 row P-1,
//! `rd-1009/tests/test-ci-plan.md:162`; `inventory-policy.md` gaps G7 and G10). Every expected
//! value comes from the cited requirement, not from what the code does today.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Category, catalog_json};
use serde_json::{Value, json};

use super::selection_catalog as fixture;
use crate::codes::{PolicyCode, SchedulingRulePolicyCode};
use crate::evaluator::tests::{
    NOW, base_facts, base_resources, decision_for, due_clock, false_fact,
    round_with_a_decision_left_without_a_reason, two_task_catalog,
};
use crate::{
    CandidateEligibility, CompiledCatalog, DecisionReason, EligibilityState, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactScalar, FactValue, InstanceSnapshot, ObservedFact,
    ObservedOutcome, PolicyEvaluation, PolicyEvaluationError, SchedulingDecisionState,
    ScopeSelector, TaskDecision, TaskRuntimeSnapshot, evaluate, evaluate_with_eligibility,
};

const HOUR_MS: u64 = 3_600_000;
/// Task 1: its trigger reads the mistyped observation.
const MISTYPED_TASK: &str = "fixture.observe";
/// Task 2: its trigger is due.
const DUE_TASK: &str = "fixture.observe-secondary";
const INSTANCE: &str = "fixture-instance-a";
const RECORD_LIST_KEY: &str = "inventory.expiring_items";

/// P-1 G7 (test-ci-plan.md:162; inventory-policy.md:87; req-basis-cp.md:101; model-355.md
/// D12 :192-210; contracts/scheduling/README.md:623-638). At each of the five sites of
/// model-355's table, an observed value whose kind differs from the kind its predicate reads
/// makes only that predicate Unknown, with `type_mismatch` or `field_missing`: `evaluate`
/// succeeds, the mistyped task does not dispatch and the other task does. Made permanent from
/// the one-off test of `5295411ba`.
#[test]
fn gate_p1_g7_a_mistyped_observation_makes_only_its_predicate_unknown() {
    let record_deadline = json!({
        "kind": "record_deadline",
        "scope": {"kind": "instance", "instance_id": INSTANCE},
        "fact_key": RECORD_LIST_KEY,
        "timestamp_field": "expires_at_unix_ms",
        "within_ms": 48 * HOUR_MS,
        "max_age_ms": 1000
    });
    let cases = [
        (
            "a fact predicate compares a string, the observation is an integer",
            json!({
                "kind": "fact",
                "scope": {"kind": "instance", "instance_id": INSTANCE},
                "fact_key": "fixture.label",
                "comparison": "eq",
                "value": {"type": "string", "value": "ready"},
                "max_age_ms": null
            }),
            Some(("fixture.label", FactValue::Integer(7))),
            None,
            "fixture.label",
            "type_mismatch",
        ),
        (
            "an outcome predicate compares a boolean, the outcome is an integer",
            json!({
                "kind": "outcome", "task_id": MISTYPED_TASK, "outcome_key": "completed",
                "comparison": "eq", "value": {"type": "boolean", "value": true}
            }),
            None,
            Some(FactValue::Integer(1)),
            "outcome.fixture.observe.completed",
            "type_mismatch",
        ),
        (
            "a record deadline over a fact that is not a record list",
            record_deadline.clone(),
            Some((RECORD_LIST_KEY, FactValue::Integer(5))),
            None,
            RECORD_LIST_KEY,
            "type_mismatch",
        ),
        (
            "a record deadline record without its timestamp field",
            record_deadline.clone(),
            Some((
                RECORD_LIST_KEY,
                FactValue::RecordList(vec![BTreeMap::from([(
                    "other_field".to_owned(),
                    FactScalar::TimestampMs(NOW + HOUR_MS),
                )])]),
            )),
            None,
            RECORD_LIST_KEY,
            "field_missing",
        ),
        (
            "a record deadline record whose timestamp field is not a timestamp",
            record_deadline,
            Some((
                RECORD_LIST_KEY,
                FactValue::RecordList(vec![BTreeMap::from([(
                    "expires_at_unix_ms".to_owned(),
                    FactScalar::Integer(5),
                )])]),
            )),
            None,
            RECORD_LIST_KEY,
            "type_mismatch",
        ),
    ];
    for (case, trigger, fact, outcome, fact_key, reason) in cases {
        let catalog = two_task_catalog(|tasks| {
            tasks[0]["trigger"] = trigger;
            tasks[1]["trigger"] = due_clock();
            tasks[0]["feedback_stop"] = false_fact();
            tasks[1]["feedback_stop"] = false_fact();
        });
        let mut facts = base_facts();
        if let Some((fact_key, value)) = fact {
            facts.facts.push(ObservedFact {
                scope: ScopeSelector::Instance {
                    instance_id: INSTANCE.to_owned(),
                },
                fact_key: fact_key.to_owned(),
                value,
                observed_at_unix_ms: NOW,
                expires_at_unix_ms: None,
                confidence_milli: 1_000,
            });
        }
        if let Some(value) = outcome {
            facts.outcomes.push(ObservedOutcome {
                task_id: MISTYPED_TASK.to_owned(),
                instance_id: INSTANCE.to_owned(),
                outcome_key: "completed".to_owned(),
                value,
                observed_at_unix_ms: NOW,
                expires_at_unix_ms: None,
                activity_window_id: None,
            });
        }
        let evaluation = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            3,
        )
        .unwrap_or_else(|error| {
            panic!("{case}: a mistyped observation must not fail the evaluation: {error:?}")
        });

        let mistyped = decision_for(&evaluation, MISTYPED_TASK, INSTANCE);
        assert_eq!(mistyped.eligibility, EligibilityState::Unknown, "{case}");
        assert!(
            mistyped
                .detection_suggestions
                .iter()
                .any(|suggestion| suggestion.fact_key == fact_key && suggestion.reason == reason),
            "{case}: a detection suggestion names {fact_key} with {reason}, got {:?}",
            mistyped.detection_suggestions
        );
        assert!(
            evaluation
                .dispatch_intents
                .iter()
                .all(|intent| intent.task_id != MISTYPED_TASK),
            "{case}: the mistyped task does not dispatch"
        );
        assert!(
            evaluation
                .dispatch_intents
                .iter()
                .any(|intent| intent.task_id == DUE_TASK && intent.instance_id == INSTANCE),
            "{case}: the other task still dispatches on the instance"
        );
    }
}

/// The evaluation instant of the base fixture.
const AT_NOW: EvaluationTime = EvaluationTime {
    unix_ms: NOW,
    monotonic_ms: NOW,
};
/// The Runtime host's deferral of a candidate whose instance another holder keeps (Workflow #369
/// W-1), as its admission callback hands it to the evaluator; the host registers the code.
const INSTANCE_HELD: &str = "dispatch_instance_held";
/// A realistic key of the selection fixture: an instance fact that recognition writes.
const STOCK: &str = "resource.stock";

/// P-1 G10 (test-ci-plan.md:162, 443; model-sched-rules.md v1.4.2 §8.2 items 1-3 and test row
/// T23; inventory-policy.md key 38; contracts/scheduling/README.md, "Why A Candidate Does Not
/// Run"). In every scenario each decision that is not Selected carries at least one reason, and
/// its last reason, the one that settled it, is an active code of the outcome catalog, never
/// `none`; each dispatch decision record names that same code as the `why` of every candidate it
/// lists. Each scenario reaches the deferral or block it is named after.
#[test]
fn gate_p1_g10_every_decision_that_is_not_selected_carries_a_registered_reason() {
    for (case, evaluation, settled_by) in scenarios() {
        assert_every_decision_explained(case, &evaluation);
        let whys = evaluation
            .decisions
            .iter()
            .filter(|decision| decision.state != SchedulingDecisionState::Selected)
            .filter_map(|decision| decision.reasons.last())
            .map(|reason| reason.code.as_str())
            .collect::<BTreeSet<_>>();
        for code in settled_by {
            assert!(
                whys.contains(code),
                "{case}: no decision is settled by `{code}`; the decisions are settled by {whys:?}"
            );
        }
    }
}

/// P-1 G10, the gap (model-sched-rules.md v1.4.2 §8.2 item 2; contracts/scheduling/README.md,
/// "Why A Candidate Does Not Run"): a path that would leave a decision without a reason is a
/// programming error. The round still closes; the decision stays unselected and deferred, with
/// the reason `decision_reason_missing` (error; `task_id`, `instance_id`), and the dispatch
/// decision record that lists it names that code as `why`, never `none`.
#[test]
fn gate_p1_g10_a_decision_left_without_a_reason_is_deferred_with_decision_reason_missing() {
    let missing = SchedulingRulePolicyCode::DecisionReasonMissing;
    assert_eq!(missing.category(), Category::Error);
    assert!(is_registered(missing.as_str()));

    let (decisions, reason_chains) =
        round_with_a_decision_left_without_a_reason().unwrap_or_else(|error| {
            panic!("a decision left without a reason must not fail the round: {error}")
        });

    let left = decisions
        .iter()
        .find(|decision| decision.task_id == DUE_TASK)
        .expect("the decision left without a reason");
    assert_eq!(left.state, SchedulingDecisionState::Deferred);
    assert_eq!(
        left.reasons
            .iter()
            .map(|reason| reason.code.as_str())
            .collect::<Vec<_>>(),
        [missing.as_str()]
    );
    assert_eq!(
        tokens(&left.reasons[0].detail),
        BTreeMap::from([("instance_id", INSTANCE), ("task_id", DUE_TASK)])
    );
    let listed = reason_chains[0]
        .reasons
        .iter()
        .find(|reason| reason.code.strip_prefix("candidate_not_selected:") == Some(DUE_TASK))
        .expect("the dispatch decision record lists the decision");
    assert_eq!(
        tokens(&listed.detail).get("why").copied(),
        Some(missing.as_str())
    );
}

/// The scenarios of G10: each one realistic catalog, snapshot and admission, the evaluation it
/// gives, and the reasons that must settle some of its decisions.
fn scenarios() -> Vec<(&'static str, PolicyEvaluation, Vec<&'static str>)> {
    let both_due = || {
        two_task_catalog(|tasks| {
            for task in tasks.iter_mut() {
                task["trigger"] = due_clock();
                task["feedback_stop"] = false_fact();
            }
        })
    };
    vec![
        (
            "two due tasks share one instance",
            run(&both_due(), &base_facts(), &base_resources()),
            vec![PolicyCode::InstanceAlreadySelected.as_str()],
        ),
        (
            "one trigger is false and one is unknown",
            run(
                &two_task_catalog(|tasks| {
                    tasks[0]["trigger"] = false_fact();
                    tasks[1]["trigger"] = unobserved_fact();
                    tasks[0]["feedback_stop"] = false_fact();
                    tasks[1]["feedback_stop"] = false_fact();
                }),
                &base_facts(),
                &base_resources(),
            ),
            vec![
                PolicyCode::TriggerFalse.as_str(),
                PolicyCode::TriggerUnknown.as_str(),
            ],
        ),
        (
            "a dispatch cooldown has not elapsed",
            {
                let mut facts = base_facts();
                facts.tasks.push(TaskRuntimeSnapshot {
                    task_id: MISTYPED_TASK.to_owned(),
                    instance_id: INSTANCE.to_owned(),
                    last_dispatched_unix_ms: Some(NOW - 100),
                    eligible_since_unix_ms: Some(NOW - 100),
                    terminal_state: None,
                    completed_window: None,
                    last_duration_ms: None,
                    failure_streak: None,
                });
                run(&both_due(), &facts, &base_resources())
            },
            vec![PolicyCode::TaskCooldownActive.as_str()],
        ),
        (
            "the instance is unavailable",
            {
                let mut facts = base_facts();
                facts.instances[0].available = false;
                run(&both_due(), &facts, &base_resources())
            },
            vec![PolicyCode::InstanceUnavailable.as_str()],
        ),
        (
            "the instance does not provide the entry operation",
            {
                let mut facts = base_facts();
                facts.instances[0].capability_operation_ids.clear();
                run(&both_due(), &facts, &base_resources())
            },
            vec![PolicyCode::CapabilityMissing.as_str()],
        ),
        (
            "the scope of a task matches no instance",
            run(
                &two_task_catalog(|tasks| {
                    for task in tasks.iter_mut() {
                        task["trigger"] = due_clock();
                        task["feedback_stop"] = false_fact();
                    }
                    tasks[1]["scope"] =
                        json!({"kind": "instance", "instance_id": "fixture-instance-z"});
                }),
                &base_facts(),
                &base_resources(),
            ),
            vec![PolicyCode::ScopeWithoutInstance.as_str()],
        ),
        (
            "the host cannot carry the load of a task",
            {
                let mut resources = base_resources();
                resources.hosts[0].cpu_available_milli = 100;
                run(&both_due(), &base_facts(), &resources)
            },
            vec![PolicyCode::HostCapacityInsufficient.as_str()],
        ),
        (
            "two instances share a host with room for one run",
            {
                let catalog = two_task_catalog(|tasks| {
                    tasks[0]["scope"] = json!({"kind": "server", "server_id": "fixture-server-a"});
                    tasks[0]["trigger"] = due_clock();
                    tasks[1]["trigger"] = false_fact();
                    tasks[0]["feedback_stop"] = false_fact();
                    tasks[1]["feedback_stop"] = false_fact();
                });
                let mut facts = base_facts();
                facts.instances.push(InstanceSnapshot {
                    instance_id: "fixture-instance-b".to_owned(),
                    server_id: "fixture-server-a".to_owned(),
                    game_id: "fixture-game-a".to_owned(),
                    host_id: "fixture-host-a".to_owned(),
                    available: true,
                    capability_operation_ids: vec!["operation.observe".to_owned()],
                    preferred_task_ids: vec![MISTYPED_TASK.to_owned()],
                });
                let mut resources = base_resources();
                resources.hosts[0].cpu_available_milli = 200;
                resources.hosts[0].gpu_available_milli = 100;
                resources.hosts[0].io_available_milli = 300;
                run(&catalog, &facts, &resources)
            },
            vec![
                PolicyCode::HostBudgetDeferred.as_str(),
                PolicyCode::TriggerFalse.as_str(),
            ],
        ),
        (
            "admission defers every candidate",
            evaluate_with_eligibility::<PolicyEvaluationError>(
                &both_due(),
                &base_facts(),
                &base_resources(),
                AT_NOW,
                3,
                |_| {
                    Ok(CandidateEligibility::Deferred {
                        reason: DecisionReason {
                            code: INSTANCE_HELD.to_owned(),
                            detail: "count=1".to_owned(),
                        },
                        next_wake_unix_ms: None,
                    })
                },
            )
            .unwrap_or_else(|error| panic!("the deferring admission evaluates: {error}")),
            vec![INSTANCE_HELD],
        ),
        (
            "a selection document reads a fact one instance has not observed",
            {
                let document = fixture::selection(
                    Vec::new(),
                    vec![fixture::integer_fact(STOCK, fixture::DAY_MS)],
                    vec![fixture::gate(
                        "gate-stock",
                        fixture::at_least(fixture::fact_ref(STOCK), 1),
                        fixture::drop_candidate(),
                    )],
                    Vec::new(),
                );
                fixture::evaluate_fixture(
                    &fixture::catalog(&[("task.alpha", 100), ("task.beta", 90)], Some(&document)),
                    &[
                        fixture::instance("instance-a", &[]),
                        fixture::instance("instance-b", &[]),
                    ],
                    vec![fixture::fact(
                        "instance-b",
                        STOCK,
                        5,
                        fixture::NOW - 60_000,
                        None,
                    )],
                )
                .unwrap_or_else(|error| panic!("the selection scenario evaluates: {error}"))
            },
            vec![
                PolicyCode::FactRefreshUnbound.as_str(),
                PolicyCode::InstanceAlreadySelected.as_str(),
            ],
        ),
    ]
}

/// The invariant of G10 over one evaluation.
fn assert_every_decision_explained(case: &str, evaluation: &PolicyEvaluation) {
    let mut not_selected = 0_usize;
    for decision in &evaluation.decisions {
        if decision.state != SchedulingDecisionState::Selected {
            not_selected += 1;
            settling_reason(case, decision);
        }
    }
    assert!(
        not_selected > 0,
        "{case}: the scenario leaves a decision unselected"
    );
    for chain in &evaluation.reason_chains {
        let instance_id = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.reason_chain_id == chain.id)
            .map(|intent| intent.instance_id.as_str())
            .unwrap_or_else(|| panic!("{case}: reason chain {} has no dispatch intent", chain.id));
        for listed in &chain.reasons {
            let Some(task_id) = listed.code.strip_prefix("candidate_not_selected:") else {
                continue;
            };
            let decision = decision_for(evaluation, task_id, instance_id);
            assert_eq!(
                tokens(&listed.detail).get("why").copied(),
                Some(settling_reason(case, decision)),
                "{case}: the record of {} names the last reason of {task_id} as why",
                chain.id
            );
        }
    }
}

/// The last reason of a decision that is not Selected, the one that settled it: present, never
/// `none`, and an active code of the outcome catalog.
fn settling_reason<'a>(case: &str, decision: &'a TaskDecision) -> &'a str {
    let why = decision
        .reasons
        .last()
        .map(|reason| reason.code.as_str())
        .unwrap_or_else(|| {
            panic!(
                "{case}: {} on {:?} is not selected and carries no reason",
                decision.task_id, decision.instance_id
            )
        });
    assert_ne!(why, "none", "{case}: {} names no reason", decision.task_id);
    assert!(
        is_registered(why),
        "{case}: `{why}`, the last reason of {} on {:?}, is not an active code of the outcome catalog",
        decision.task_id,
        decision.instance_id
    );
    why
}

/// A reason counts as registered when its spelling is an active code of the outcome catalog
/// (contracts/scheduling/README.md, "Why A Candidate Does Not Run").
fn is_registered(code: &str) -> bool {
    let catalog = catalog_json()
        .unwrap_or_else(|error| panic!("the embedded outcome catalog parses: {error}"));
    catalog["codes"][code]["status"] == "active"
}

/// The `key=value` tokens of a reason detail.
fn tokens(detail: &str) -> BTreeMap<&str, &str> {
    detail
        .split_whitespace()
        .filter_map(|token| token.split_once('='))
        .collect()
}

fn run(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
) -> PolicyEvaluation {
    evaluate(catalog, facts, resources, AT_NOW, 3)
        .unwrap_or_else(|error| panic!("the scenario evaluates: {error}"))
}

/// A fact predicate over a key the fixture never observes: Unknown.
fn unobserved_fact() -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": INSTANCE},
        "fact_key": "fixture.unobserved",
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": null
    })
}
