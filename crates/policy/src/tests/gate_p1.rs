// SPDX-License-Identifier: AGPL-3.0-only

//! P-1 gate test of the policy crate (test plan v2.2 row P-1,
//! `rd-1009/tests/test-ci-plan.md:162`; `inventory-policy.md` gap G7). Every expected value comes
//! from the cited requirement, not from what the code does today.

use std::collections::BTreeMap;

use serde_json::json;

use crate::evaluator::tests::{
    NOW, base_facts, base_resources, decision_for, due_clock, false_fact, two_task_catalog,
};
use crate::{
    EligibilityState, EvaluationTime, FactScalar, FactValue, ObservedFact, ObservedOutcome,
    ScopeSelector, evaluate,
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
