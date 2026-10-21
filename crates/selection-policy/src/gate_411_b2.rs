// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B2 (b-plan §2.2 and its BT-B2 tests T1 and T3; contracts/selection-policy.md,
// section Predicates; register §1.146 B2-1; policy key 58): a gate may compare two values,
// `left op right × mul + add`, with value-test semantics, and a document that does not use the
// new members keeps its canonical bytes and its `policy_sha256`. Every expected value comes from
// §2.2. The documents are neutral: a candidate's `cost` against its `funds` less a reserve.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{
    Candidate, CandidateStatus, CandidateVerdict, GateOutcome, GateResult,
    SELECTION_POLICY_SCHEMA_VERSION, ScalarValue, SelectionDecision, SelectionFactSnapshot,
    SelectionOutcome, SelectionPolicy, canonical_bytes, canonical_sha256, evaluate,
};

const NOW: u64 = 1_000_500;
/// The source of the crate's fixture document, which uses none of the new members.
const FIXTURE: &str = include_str!("../tests/fixtures/policy.json");
/// The fixture document's `policy_sha256` before B2: the SHA-256 of the canonical form of its
/// source, which declares every member it serialises.
const FIXTURE_POLICY_SHA256: &str =
    "sha256:dad4d6af65c8547c207c697e5e33f9ab35fa2afc2b0bdf759c90483816040f5c";

/// A document with one gate: `cost le funds`, with `mul` and `add` when `scale` gives them.
fn compare_document(scale: Option<(u64, i64)>) -> Value {
    let mut predicate = json!({
        "kind": "integer_compare",
        "left": {"source": "field", "field": "cost"},
        "op": "le",
        "right": {"source": "field", "field": "funds"}
    });
    if let Some((mul, add)) = scale {
        predicate["mul"] = json!(mul);
        predicate["add"] = json!(add);
    }
    json!({
        "schema_version": SELECTION_POLICY_SCHEMA_VERSION,
        "policy_id": "policy-compare",
        "applies_to": {
            "candidate_layout_id": "layout-compare",
            "outcome_keys": {
                "selected": "compare-selected", "empty": "compare-empty",
                "insufficient": "compare-insufficient", "ambiguous": "compare-ambiguous",
                "unknown": "compare-unknown"
            }
        },
        "fields": [
            {"name": "cost", "value_type": {"type": "integer"}},
            {"name": "funds", "value_type": {"type": "integer"}}
        ],
        "facts": [],
        "gates": [{
            "gate_id": "gate-affordable",
            "predicate": predicate,
            "on_unknown": {"kind": "drop_candidate"}
        }],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

fn candidate(candidate_id: &str, cost: i64, funds: i64) -> Candidate {
    Candidate {
        candidate_id: candidate_id.to_owned(),
        fields: BTreeMap::from([
            ("cost".to_owned(), ScalarValue::Integer(cost)),
            ("funds".to_owned(), ScalarValue::Integer(funds)),
        ]),
    }
}

fn no_facts() -> SelectionFactSnapshot {
    SelectionFactSnapshot::from_fact_records("snapshot-compare", NOW, &[])
}

fn verdict<'a>(decision: &'a SelectionDecision, candidate_id: &str) -> &'a CandidateVerdict {
    decision
        .candidates
        .iter()
        .find(|verdict| verdict.candidate_id == candidate_id)
        .expect("candidate verdict")
}

fn gate(outcome: GateOutcome) -> [GateResult; 1] {
    [GateResult {
        gate_id: "gate-affordable".to_owned(),
        outcome,
    }]
}

#[test]
fn b2_t1_integer_compare_keeps_the_candidate_within_its_funds_less_the_reserve() {
    // §2.2: `cost le funds × 1 + (−2000)` between two fields of each candidate.
    let policy: SelectionPolicy = serde_json::from_value(compare_document(Some((1, -2_000))))
        .expect("an integer_compare document decodes");
    // Both cost 4500, so only the right side tells them apart: 4500 > 6000 − 2000 breaks the
    // gate, 4500 <= 7000 − 2000 holds it.
    let candidates = [
        candidate("layout-compare#00", 4_500, 6_000),
        candidate("layout-compare#01", 4_500, 7_000),
    ];

    let decision = evaluate(&policy, &candidates, &no_facts(), NOW).expect("decision");

    assert_eq!(decision.outcome, SelectionOutcome::Selected { count: 1 });
    assert_eq!(decision.selected, ["layout-compare#01"]);
    let breaking = verdict(&decision, "layout-compare#00");
    assert_eq!(breaking.status, CandidateStatus::GateRejected);
    assert_eq!(breaking.gates, gate(GateOutcome::Failed));
    let kept = verdict(&decision, "layout-compare#01");
    assert_eq!(kept.status, CandidateStatus::Ranked);
    assert_eq!(kept.gates, gate(GateOutcome::Passed));
}

#[test]
fn b2_t3_a_document_without_the_new_members_keeps_its_canonical_bytes_and_policy_sha256() {
    // §2.2: a document without the new members keeps its canonical bytes and `policy_sha256`.
    let source: Value = serde_json::from_str(FIXTURE).expect("fixture JSON");
    let policy: SelectionPolicy = serde_json::from_str(FIXTURE).expect("fixture policy decodes");
    assert_eq!(
        canonical_bytes(&policy).expect("canonical bytes"),
        canonical_bytes(&source).expect("canonical source")
    );
    let decision = evaluate(&policy, &[], &no_facts(), NOW).expect("decision");
    assert_eq!(decision.policy_sha256, FIXTURE_POLICY_SHA256);

    // An `integer_compare` that leaves out the optional `mul` and `add` gains neither member:
    // its canonical bytes are those it spells.
    let source = compare_document(None);
    let policy: SelectionPolicy =
        serde_json::from_value(source.clone()).expect("an integer_compare document decodes");
    assert_eq!(
        String::from_utf8(canonical_bytes(&policy).expect("canonical bytes")),
        String::from_utf8(canonical_bytes(&source).expect("canonical source"))
    );
    let decision = evaluate(&policy, &[], &no_facts(), NOW).expect("decision");
    assert_eq!(
        decision.policy_sha256,
        canonical_sha256(&source).expect("source identity")
    );
}
