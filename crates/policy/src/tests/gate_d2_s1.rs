// SPDX-License-Identifier: AGPL-3.0-only

//! D2 S1 gate tests (Workflow #384; `rd-1009/d2/model-d2-fact-refresh.md` §8, rows T1-T7, the
//! S1 half; test plan P-2c). An input of the scheduling-layer selection document that is
//! unknown defers only the candidate that read it, under a registered reason with no wake, and
//! the round still succeeds.
//!
//! T6 (package select still follows its clauses) is covered by the selection-policy tests
//! `an_unknown_fact_drops_candidates_instead_of_scoring_it_as_zero`,
//! `a_declared_substitution_is_used_and_recorded`,
//! `an_aborting_rule_ends_the_evaluation_with_an_unknown_outcome` and
//! `a_gate_that_cannot_be_decided_follows_its_declared_handling`, unmodified.

use std::collections::BTreeMap;

use actingcommand_contract::outcome::Category;
use serde_json::{Value, json};

use super::selection_catalog::{
    DAY_MS, HOUR_MS, NOW, abort_evaluation, at_least, catalog, drop_candidate, evaluate_fixture,
    fact, fact_ref, field_ref, gate, identity_term, instance, integer_fact, integer_field,
    selection,
};
use crate::codes::PolicyCode;
use crate::{PolicyEvaluation, PolicyEvaluationResult, SchedulingDecisionState, TaskDecision};

const FACT_REFRESH_UNBOUND: &str = "fact_refresh_unbound";
const SELECTION_INPUT_UNKNOWN: &str = "selection_input_unknown";
/// Retired by D2 S1: the scheduling round no longer ends on an unknown input.
const SELECTION_EVALUATION_ABORTED: &str = "selection_evaluation_aborted";

const ALPHA: &str = "task.alpha";
const BETA: &str = "task.beta";
const A: &str = "instance-a";
const B: &str = "instance-b";
/// Realistic keys: instance facts that recognition writes (model §3.1).
const STOCK: &str = "resource.stock";
const K1: &str = "resource.k1";
const K2: &str = "resource.k2";
const K3: &str = "inventory.k3";
const K5: &str = "resource.k5";
const K6: &str = "resource.k6";
/// An instruction key: neither an instruction nor pack data supplies it for A.
const MODE: &str = "session.mode";
const PRIORITY: &str = "task.priority";
const AFFINITY: &str = "instance.affinity";

/// D2 T1 (model-d2-fact-refresh.md:581); test plan P-2 G13 abort sub-row (P-2c).
#[test]
fn gate_d2_t1_an_unknown_realistic_fact_defers_only_its_candidate_and_the_round_succeeds() {
    let evaluation = evaluate_stock_gate(abort_evaluation())
        .unwrap_or_else(|error| panic!("an unknown fact of A must not fail the round: {error}"));

    assert!(
        has_intent(&evaluation, ALPHA, B),
        "B keeps its dispatch intent"
    );
    assert!(!has_intent(&evaluation, ALPHA, A));
    let deferred = decision(&evaluation, ALPHA, A);
    assert_eq!(deferred.state, SchedulingDecisionState::Deferred);
    assert_registered_warning(PolicyCode::FactRefreshUnbound, FACT_REFRESH_UNBOUND);
    assert_eq!(
        values(deferred, FACT_REFRESH_UNBOUND),
        [unbound(STOCK, "fact_expired")]
    );
    assert!(
        !format!("{evaluation:?}").contains(SELECTION_EVALUATION_ABORTED),
        "the retired code appears nowhere"
    );
}

/// D2 T2 (model-d2-fact-refresh.md:582), half (a). Half (b), the clause warning, moves to the
/// slice that makes actingd load a scheduling-layer document (decision D3).
#[test]
fn gate_d2_t2_the_clause_a_scheduling_document_declares_does_not_change_the_round() {
    let dropped = evaluate_stock_gate(drop_candidate())
        .unwrap_or_else(|error| panic!("the drop document evaluates: {error}"));
    let substitute = evaluate_stock_gate(json!({"kind": "substitute_verdict", "passes": true}))
        .unwrap_or_else(|error| panic!("the substitute document evaluates: {error}"));
    assert_eq!(
        without_catalog_identity(substitute),
        without_catalog_identity(dropped.clone()),
        "a substitute clause changes nothing at the scheduling layer"
    );
    let abort = evaluate_stock_gate(abort_evaluation())
        .unwrap_or_else(|error| panic!("the abort document evaluates: {error}"));
    assert_eq!(
        without_catalog_identity(abort),
        without_catalog_identity(dropped),
        "an abort clause changes nothing at the scheduling layer"
    );
}

/// D2 T3 (model-d2-fact-refresh.md:583).
#[test]
fn gate_d2_t3_an_input_no_reading_can_supply_defers_with_selection_input_unknown() {
    // An instruction key B has and A lacks.
    let document = selection(
        Vec::new(),
        vec![integer_fact(MODE, DAY_MS)],
        vec![gate(
            "gate-mode",
            at_least(fact_ref(MODE), 1),
            drop_candidate(),
        )],
        Vec::new(),
    );
    let evaluation = evaluate_fixture(
        &catalog(&[(ALPHA, 100)], Some(&document)),
        &[instance(A, &[]), instance(B, &[])],
        vec![fact(B, MODE, 1, NOW - 60_000, None)],
    )
    .unwrap_or_else(|error| panic!("the instruction-key document evaluates: {error}"));
    assert_input_unknown(&evaluation, "gate-mode", "fact_missing");

    // A fixed field whose lookup covers B's value and not A's.
    let document = selection(
        vec![json!({"name": AFFINITY, "value_type": {"type": "boolean"}})],
        Vec::new(),
        Vec::new(),
        vec![json!({
            "term_id": "term-affinity",
            "value": field_ref(AFFINITY),
            "transform": {
                "kind": "lookup",
                "entries": [{"key": {"type": "boolean", "value": true}, "value_milli": 1000}]
            },
            "weight_milli": 1000,
            "on_unknown": drop_candidate()
        })],
    );
    let evaluation = evaluate_fixture(
        &catalog(&[(ALPHA, 100)], Some(&document)),
        &[instance(A, &[]), instance(B, &[ALPHA])],
        Vec::new(),
    )
    .unwrap_or_else(|error| panic!("the fixed-field document evaluates: {error}"));
    assert_input_unknown(&evaluation, "term-affinity", "lookup_miss");
}

/// D2 T4 (model-d2-fact-refresh.md:584).
#[test]
fn gate_d2_t4_a_candidate_carries_every_unknown_key_and_one_reading_none_still_ranks() {
    // The gate holds `all(K1 >= 1, K2 >= 1)` for alpha. Beta's priority settles the gate before
    // it reads either key, so beta reads none of the unknown keys.
    let stock_gate = gate(
        "gate-stock",
        json!({"kind": "any", "of": [
            at_least(field_ref(PRIORITY), 150),
            {"kind": "all", "of": [at_least(fact_ref(K1), 1), at_least(fact_ref(K2), 1)]}
        ]}),
        drop_candidate(),
    );
    let priority_term = identity_term("term-priority", field_ref(PRIORITY), drop_candidate());
    let tasks = [(ALPHA, 100), (BETA, 200)];
    let instances = [instance(A, &[])];
    let facts = || {
        vec![
            fact(A, K1, 5, NOW - 60_000, None),
            // Older than the document's one-hour bound, though its own record never expires.
            fact(A, K2, 5, NOW - 2 * HOUR_MS, None),
            fact(A, K5, 5, NOW - 2 * HOUR_MS, Some(NOW - HOUR_MS)),
        ]
    };

    // (1) K1 passes and K2 is stale: alpha is deferred with (K2, fact_stale) alone.
    // (4) Beta, on the same instance, ranks and gets the intent.
    let document = selection(
        vec![integer_field(PRIORITY)],
        vec![integer_fact(K1, HOUR_MS), integer_fact(K2, HOUR_MS)],
        vec![stock_gate.clone()],
        vec![priority_term.clone()],
    );
    let evaluation = evaluate_fixture(&catalog(&tasks, Some(&document)), &instances, facts())
        .unwrap_or_else(|error| panic!("the gate document evaluates: {error}"));
    let alpha = decision(&evaluation, ALPHA, A);
    assert_eq!(alpha.state, SchedulingDecisionState::Deferred);
    assert_registered_warning(PolicyCode::FactRefreshUnbound, FACT_REFRESH_UNBOUND);
    assert_eq!(
        values(alpha, FACT_REFRESH_UNBOUND),
        [unbound(K2, "fact_stale")]
    );
    assert_eq!(
        decision(&evaluation, BETA, A).state,
        SchedulingDecisionState::Selected
    );
    assert!(has_intent(&evaluation, BETA, A));

    // (2) A term reads K3, which is missing: alpha carries K3 next to K2.
    let document = selection(
        vec![integer_field(PRIORITY)],
        vec![
            integer_fact(K1, HOUR_MS),
            integer_fact(K2, HOUR_MS),
            integer_fact(K3, HOUR_MS),
        ],
        vec![stock_gate.clone()],
        vec![
            priority_term.clone(),
            identity_term("term-supply", fact_ref(K3), drop_candidate()),
        ],
    );
    let evaluation = evaluate_fixture(&catalog(&tasks, Some(&document)), &instances, facts())
        .unwrap_or_else(|error| panic!("the term document evaluates: {error}"));
    let alpha = decision(&evaluation, ALPHA, A);
    assert_eq!(alpha.state, SchedulingDecisionState::Deferred);
    assert_eq!(
        values(alpha, FACT_REFRESH_UNBOUND),
        [unbound(K2, "fact_stale"), unbound(K3, "fact_missing")]
    );

    // (3) A field read of K5, whose record has expired.
    let document = selection(
        vec![integer_field(PRIORITY), integer_field(K5)],
        vec![integer_fact(K1, HOUR_MS), integer_fact(K2, HOUR_MS)],
        vec![stock_gate],
        vec![
            priority_term,
            identity_term("term-reserve", field_ref(K5), drop_candidate()),
        ],
    );
    let evaluation = evaluate_fixture(&catalog(&tasks, Some(&document)), &instances, facts())
        .unwrap_or_else(|error| panic!("the field document evaluates: {error}"));
    let beta = decision(&evaluation, BETA, A);
    assert_eq!(beta.state, SchedulingDecisionState::Deferred);
    assert_eq!(
        values(beta, FACT_REFRESH_UNBOUND),
        [unbound(K5, "fact_expired")]
    );
}

/// D2 T5 (model-d2-fact-refresh.md:585).
#[test]
fn gate_d2_t5_every_document_fact_read_bounds_the_intent_and_wakes_the_round() {
    // A `fact` source: K expires at t; the document's own bound lies a day after publication.
    let t = NOW + 600_000;
    let document = selection(
        Vec::new(),
        vec![integer_fact(STOCK, DAY_MS)],
        Vec::new(),
        vec![identity_term(
            "term-stock",
            fact_ref(STOCK),
            drop_candidate(),
        )],
    );
    let evaluation = evaluate_fixture(
        &catalog(&[(ALPHA, 100)], Some(&document)),
        &[instance(A, &[])],
        vec![fact(A, STOCK, 5, NOW - 60_000, Some(t))],
    )
    .unwrap_or_else(|error| panic!("the fact-source document evaluates: {error}"));
    assert_bounded_by(&evaluation, t);

    // An instance-fact field: K6 expires at t6.
    let t6 = NOW + 300_000;
    let document = selection(
        vec![integer_field(K6)],
        Vec::new(),
        Vec::new(),
        vec![identity_term(
            "term-reserve",
            field_ref(K6),
            drop_candidate(),
        )],
    );
    let evaluation = evaluate_fixture(
        &catalog(&[(ALPHA, 100)], Some(&document)),
        &[instance(A, &[])],
        vec![fact(A, K6, 5, NOW - 60_000, Some(t6))],
    )
    .unwrap_or_else(|error| panic!("the field document evaluates: {error}"));
    assert_bounded_by(&evaluation, t6);
}

/// D2 T7 (model-d2-fact-refresh.md:587), the S1 half.
#[test]
fn gate_d2_t7_an_expired_field_is_never_scored_with_its_old_value() {
    let document = selection(
        vec![integer_field(STOCK)],
        Vec::new(),
        Vec::new(),
        vec![identity_term(
            "term-stock",
            field_ref(STOCK),
            drop_candidate(),
        )],
    );
    let evaluation = evaluate_fixture(
        &catalog(&[(ALPHA, 100)], Some(&document)),
        &[instance(A, &[])],
        vec![fact(A, STOCK, 9, NOW - 2 * HOUR_MS, Some(NOW - HOUR_MS))],
    )
    .unwrap_or_else(|error| panic!("the field document evaluates: {error}"));

    let deferred = decision(&evaluation, ALPHA, A);
    assert!(
        !deferred
            .reasons
            .iter()
            .any(|reason| reason.outcome.code().as_str() == "score_computed"),
        "the expired value is never scored: {:?}",
        deferred.reasons
    );
    assert!(evaluation.dispatch_intents.is_empty(), "nothing is ranked");
    assert_eq!(deferred.state, SchedulingDecisionState::Deferred);
    assert_registered_warning(PolicyCode::FactRefreshUnbound, FACT_REFRESH_UNBOUND);
    assert_eq!(
        values(deferred, FACT_REFRESH_UNBOUND),
        [unbound(STOCK, "fact_expired")]
    );
}

/// Instances A and B under one gate on the realistic key K through a `fact` source, with the
/// given clause. A's K has expired; B's is fresh.
fn evaluate_stock_gate(on_unknown: Value) -> PolicyEvaluationResult<PolicyEvaluation> {
    let document = selection(
        Vec::new(),
        vec![integer_fact(STOCK, DAY_MS)],
        vec![gate("gate-stock", at_least(fact_ref(STOCK), 1), on_unknown)],
        Vec::new(),
    );
    evaluate_fixture(
        &catalog(&[(ALPHA, 100)], Some(&document)),
        &[instance(A, &[]), instance(B, &[])],
        vec![
            fact(A, STOCK, 5, NOW - 2 * HOUR_MS, Some(NOW - HOUR_MS)),
            fact(B, STOCK, 5, NOW - 60_000, Some(NOW + HOUR_MS)),
        ],
    )
}

/// The evaluation without the five members that carry the catalog identity (decision D7). The
/// document is hashed into the catalog, so these differ between documents; nothing else may.
fn without_catalog_identity(mut evaluation: PolicyEvaluation) -> PolicyEvaluation {
    for intent in &mut evaluation.dispatch_intents {
        intent.catalog_hash.clear();
        intent.decision_id.clear();
        intent.reason_chain_id.clear();
    }
    for chain in &mut evaluation.reason_chains {
        chain.id.clear();
        chain.decision_id.clear();
    }
    evaluation
}

fn assert_input_unknown(evaluation: &PolicyEvaluation, rule_id: &str, unknown_reason: &str) {
    let deferred = decision(evaluation, ALPHA, A);
    assert_eq!(deferred.state, SchedulingDecisionState::Deferred);
    assert_registered_warning(PolicyCode::SelectionInputUnknown, SELECTION_INPUT_UNKNOWN);
    assert_eq!(
        values(deferred, SELECTION_INPUT_UNKNOWN),
        [tokens(&[
            ("instance_id", A),
            ("rule_id", rule_id),
            ("unknown_reason", unknown_reason),
        ])]
    );
    assert!(
        values(deferred, FACT_REFRESH_UNBOUND).is_empty(),
        "no reading is requested for an input that no reading can supply"
    );
    assert!(has_intent(evaluation, ALPHA, B), "B is unaffected");
    let unaffected = decision(evaluation, ALPHA, B);
    assert!(values(unaffected, SELECTION_INPUT_UNKNOWN).is_empty());
    assert!(values(unaffected, FACT_REFRESH_UNBOUND).is_empty());
}

/// The code is registered under the spelling the model gives and its one category, warning
/// (model §7.1).
fn assert_registered_warning(code: PolicyCode, spelling: &str) {
    assert_eq!(code.as_str(), spelling);
    assert_eq!(code.category(), Category::Warning);
}

fn assert_bounded_by(evaluation: &PolicyEvaluation, bound: u64) {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == ALPHA && intent.instance_id == A)
        .expect("A's candidate is selected");
    let fresh_until = intent.prerequisites.facts_fresh_until_unix_ms;
    assert!(
        fresh_until.is_some_and(|until| until <= bound),
        "facts_fresh_until_unix_ms {fresh_until:?} is not bounded by {bound}"
    );
    assert_eq!(evaluation.next_wake_unix_ms, Some(bound + 1));
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
        .unwrap_or_else(|| panic!("a decision for {task_id} on {instance_id}"))
}

fn has_intent(evaluation: &PolicyEvaluation, task_id: &str, instance_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id && intent.instance_id == instance_id)
}

/// The `key=value` tokens of every reason the decision carries under `code`; any other token
/// in a detail fails (outcome-code spec §2 rule 2: no prose).
fn values<'a>(decision: &'a TaskDecision, code: &str) -> Vec<BTreeMap<&'a str, &'a str>> {
    decision
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == code)
        .map(|reason| {
            reason
                .detail
                .split(' ')
                .map(|token| {
                    token.split_once('=').unwrap_or_else(|| {
                        panic!("`{token}` in `{}` is not a key=value token", reason.detail)
                    })
                })
                .collect()
        })
        .collect()
}

fn tokens<'a>(pairs: &[(&'a str, &'a str)]) -> BTreeMap<&'a str, &'a str> {
    pairs.iter().copied().collect()
}

/// The required values of `fact_refresh_unbound` for instance A; S1 writes no `reader_problem`.
fn unbound<'a>(fact_key: &'a str, unknown_reason: &'a str) -> BTreeMap<&'a str, &'a str> {
    tokens(&[
        ("instance_id", A),
        ("fact_key", fact_key),
        ("unknown_reason", unknown_reason),
    ])
}
