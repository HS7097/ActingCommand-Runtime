// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 A26 gate tests (policy 57; register §1.145; contracts/scheduling/README.md,
//! "Dispatch Decision Record" and "Why A Candidate Does Not Run"; #386 SR-6a). A dispatch
//! decision record persists why each candidate it lists did not run as typed values beside
//! `task_id`, counts the candidates it does not list by why, and a stored record with these
//! values reads back through the 0.12 reader unchanged. Every expected value comes from the
//! slice's requirements: each listed candidate's `why`, `decision_state` and `eligibility` are
//! its own decision's last reason, state and eligibility, the ones its detail prints.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{EventPayload, PolicyPayload, PolicyReasonRecord};
use serde_json::{Value, json};

use crate::evaluator::tests::{
    NOW, base_facts, base_resources, decision_for, due_clock, false_fact, two_task_catalog,
};
use crate::{
    CompiledCatalog, DecisionReason, DecisionReasonChain, DispatchIntent, EvaluationTime,
    PolicyEvaluation, evaluate,
};

const INSTANCE: &str = "fixture-instance-a";
/// At most this many related candidates are listed in one record (README, item 3).
const LISTED_AT_MOST: usize = 8;

/// A26 requirement 1: each candidate a dispatch decision record lists carries, beside
/// `task_id`, `why` (its decision's last reason code, never `none`), `decision_state` (its
/// decision's state) and `eligibility` (its decision's eligibility), the same `why` and state
/// its detail prints, ranked or not. Four tasks share the instance: one runs, two that were
/// ranked lose to it, and one whose trigger is false is never ranked.
#[test]
fn a26_a_not_selected_candidate_records_its_why_state_and_eligibility() {
    let evaluation = run(&catalog(4, &[3]));
    let (_, chain) = the_dispatch(&evaluation);
    let listed = listed(chain);
    assert_eq!(
        listed.len(),
        3,
        "the record lists the three other candidates of the instance"
    );
    let (mut ranked, mut not_ranked) = (0, 0);
    for reason in listed {
        let values = reason.outcome.values();
        let task_id = values
            .get("task_id")
            .and_then(Value::as_str)
            .expect("a listed candidate names its task_id");
        let decision = decision_for(&evaluation, task_id, INSTANCE);
        let why = decision
            .reasons
            .last()
            .map(|last| last.outcome.code().as_str())
            .expect("a candidate that was not selected carries a reason");
        assert_ne!(why, "none", "{task_id}: why is never none");
        let state = token_of(decision.state);
        let eligibility = token_of(decision.eligibility);
        assert_eq!(
            values.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["decision_state", "eligibility", "task_id", "why"]),
            "{task_id}: the values of a listed candidate"
        );
        assert_eq!(values.get("why"), Some(&json!(why)), "{task_id}: why");
        assert_eq!(
            values.get("decision_state"),
            Some(&json!(state)),
            "{task_id}: decision_state"
        );
        assert_eq!(
            values.get("eligibility"),
            Some(&json!(eligibility)),
            "{task_id}: eligibility"
        );
        let printed = tokens(&reason.detail);
        assert_eq!(
            printed.get("why").copied(),
            Some(why),
            "{task_id}: the detail's why"
        );
        assert_eq!(
            printed.get("state").copied(),
            Some(state.as_str()),
            "{task_id}: the detail's state"
        );
        match printed.get("eligibility") {
            Some(printed) => {
                not_ranked += 1;
                assert_eq!(*printed, eligibility, "{task_id}: the detail's eligibility");
            }
            None => {
                ranked += 1;
                assert!(
                    printed.contains_key("total"),
                    "{task_id}: a ranked detail prints its total"
                );
            }
        }
    }
    assert_eq!(
        (ranked, not_ranked),
        (2, 1),
        "two ranked candidates and one that was not ranked are listed"
    );
}

/// A26 requirement 2: `decision_record` carries `omitted_why`, the candidates beyond the eight
/// listed counted by why, one record `{why, count}` per code in ascending code order, and leaves
/// it out when it lists every related candidate. Eleven tasks share the instance: one runs, nine
/// that were ranked lose to it and one whose trigger is false is never ranked, so the record
/// lists eight and omits two, with two different whys.
#[test]
fn a26_a_decision_record_counts_the_omitted_candidates_by_why() {
    let evaluation = run(&catalog(11, &[10]));
    let (intent, chain) = the_dispatch(&evaluation);
    let listed_ids = listed(chain)
        .iter()
        .map(|reason| {
            reason
                .outcome
                .value("task_id")
                .and_then(Value::as_str)
                .expect("a listed candidate names its task_id")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(listed_ids.len(), LISTED_AT_MOST);
    let mut expected = BTreeMap::<&str, u64>::new();
    for decision in &evaluation.decisions {
        if decision.instance_id.as_deref() != Some(INSTANCE)
            || decision.task_id == intent.task_id
            || listed_ids.contains(decision.task_id.as_str())
        {
            continue;
        }
        let why = decision
            .reasons
            .last()
            .map(|last| last.outcome.code().as_str())
            .expect("an omitted candidate carries a reason");
        *expected.entry(why).or_default() += 1;
    }
    assert_eq!(
        expected.values().sum::<u64>(),
        2,
        "the record omits two candidates"
    );
    assert_eq!(expected.len(), 2, "the omitted candidates have two whys");
    let expected = expected
        .iter()
        .map(|(why, count)| json!({"why": why, "count": count}))
        .collect::<Vec<_>>();
    assert_eq!(
        decision_record(chain).outcome.value("omitted_why"),
        Some(&Value::Array(expected))
    );

    let all_listed = run(&catalog(4, &[3]));
    assert_eq!(
        decision_record(the_dispatch(&all_listed).1)
            .outcome
            .value("omitted_why"),
        None,
        "a record that lists every related candidate carries no omitted_why"
    );
}

/// A26 requirement 3: the record envelope and the event payload members are unchanged, and the
/// 0.12 reader accepts the new values. A `policy.dispatch_intent` record whose reasons are the
/// writers' form of the chain (each reason's outcome, `PolicyReasonRecord`) with the new values
/// decodes as the ledger and `actingledger` decode it, validates, keeps every value and encodes
/// back to the same record.
#[test]
fn a26_an_earlier_reader_accepts_the_new_values() {
    let evaluation = run(&catalog(11, &[10]));
    let (intent, chain) = the_dispatch(&evaluation);
    let reasons = chain
        .reasons
        .iter()
        .map(|reason| PolicyReasonRecord(reason.outcome.clone()))
        .collect::<Vec<_>>();
    let stored = json!({
        "family": "policy",
        "payload": {
            "kind": "dispatch_intent",
            "data": {
                "action": "policy.dispatch",
                "decision_id": intent.decision_id,
                "task_id": intent.task_id,
                "instance_id": intent.instance_id,
                "operation_id": intent.operation_id,
                // The Runtime host binds this member; a neutral value of its form.
                "procedure_binding_digest": format!("sha256:{}", "d".repeat(64)),
                "reason_chain_id": intent.reason_chain_id,
                "reasons": reasons,
                "catalog_hash": intent.catalog_hash,
                "catalog_version": intent.catalog_version,
                "input_ledger_position": intent.input_ledger_position,
                "fact_snapshot_id": intent.fact_snapshot_id,
                "approval_fact_ids": intent.approval_refs,
                "urgency_milli": intent.prerequisites.urgency_milli,
                "audit": {}
            }
        }
    });
    let stored_reasons = stored
        .pointer("/payload/data/reasons")
        .and_then(Value::as_array)
        .expect("the stored reasons");
    let not_selected = stored_reasons
        .iter()
        .filter(|reason| reason["code"] == "decision_candidate_not_selected")
        .collect::<Vec<_>>();
    assert_eq!(not_selected.len(), LISTED_AT_MOST);
    for reason in not_selected {
        assert_eq!(
            reason
                .get("values")
                .and_then(Value::as_object)
                .map(|values| values.keys().map(String::as_str).collect::<BTreeSet<_>>()),
            Some(BTreeSet::from([
                "decision_state",
                "eligibility",
                "task_id",
                "why"
            ])),
            "the record is stored with the new values"
        );
    }
    assert!(
        stored_reasons.iter().any(|reason| {
            reason["code"] == "decision_record" && reason["values"].get("omitted_why").is_some()
        }),
        "the record is stored with omitted_why"
    );

    // The reader: decode, validate, and encode back to the same record.
    let payload: EventPayload =
        serde_json::from_value(stored.clone()).expect("the 0.12 reader decodes the record");
    payload
        .validate()
        .expect("the 0.12 reader validates the record");
    assert_eq!(
        serde_json::to_value(&payload).expect("the record encodes"),
        stored,
        "the record reads back unchanged"
    );
    let EventPayload::Policy(PolicyPayload::DispatchIntent(dispatch)) = &payload else {
        panic!("a dispatch intent");
    };
    assert_eq!(
        dispatch.reasons(),
        reasons.as_slice(),
        "every reason reads back with its values"
    );
}

/// A catalog of `count` tasks scoped to the fixture instance, each due on its hourly clock and
/// never stopped, except those whose index `false_triggers` names: their trigger is false.
fn catalog(count: usize, false_triggers: &[usize]) -> CompiledCatalog {
    two_task_catalog(|tasks| {
        let template = tasks[0].clone();
        for index in tasks.len()..count {
            let mut task = template.clone();
            task["id"] = json!(format!("fixture.observe-{index:02}"));
            task["procedure_ref"] = json!(format!("procedure.observe-{index:02}"));
            task["instance_overrides"] = json!([]);
            tasks.push(task);
        }
        for (index, task) in tasks.iter_mut().enumerate() {
            task["trigger"] = if false_triggers.contains(&index) {
                false_fact()
            } else {
                due_clock()
            };
            task["feedback_stop"] = false_fact();
        }
    })
}

fn run(catalog: &CompiledCatalog) -> PolicyEvaluation {
    let time = EvaluationTime {
        unix_ms: NOW,
        monotonic_ms: NOW,
    };
    evaluate(catalog, &base_facts(), &base_resources(), time, 3)
        .unwrap_or_else(|error| panic!("the scenario evaluates: {error}"))
}

/// The one dispatch of the scenario and its reason chain.
fn the_dispatch(evaluation: &PolicyEvaluation) -> (&DispatchIntent, &DecisionReasonChain) {
    let [intent] = evaluation.dispatch_intents.as_slice() else {
        panic!(
            "the scenario dispatches one task, not {}",
            evaluation.dispatch_intents.len()
        );
    };
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the dispatch's reason chain");
    (intent, chain)
}

/// The `decision_candidate_not_selected` reasons of a chain.
fn listed(chain: &DecisionReasonChain) -> Vec<&DecisionReason> {
    chain
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == "decision_candidate_not_selected")
        .collect()
}

/// The `decision_record` reason of a chain.
fn decision_record(chain: &DecisionReasonChain) -> &DecisionReason {
    chain
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == "decision_record")
        .expect("the chain carries its decision record")
}

/// A decision's state or eligibility as its wire form spells it, the token the record prints.
fn token_of(value: impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(token)) => token,
        other => panic!("a state or eligibility is a string: {other:?}"),
    }
}

/// The `key=value` tokens of a reason detail.
fn tokens(detail: &str) -> BTreeMap<&str, &str> {
    detail
        .split_whitespace()
        .filter_map(|token| token.split_once('='))
        .collect()
}
