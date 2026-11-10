// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #428 S168-5a (register §1.291, its nine named tests; contracts/selection-policy.md,
//! "Table maximum over a list"): the value source `table_max_over_list`, on tables loaded with
//! `LoadedTable::load` and fake snapshots, with made-up ids only. No file, clock or device: every
//! table is parsed from bytes here and fixed at one instant.

use std::collections::BTreeMap;
use std::sync::Arc;

use actingcommand_contract::parse_standard_pack_table;
use serde_json::{Value, json};

use crate::list_max::{ListMaxReading, read_list_maximum};
use crate::{
    Candidate, CandidateStatus, CandidateVerdict, DecisionReason, FieldDomain, GateOutcome,
    LoadedTable, ScalarValue, SelectionDecision, SelectionErrorCode, SelectionFactEntry,
    SelectionFactSnapshot, SelectionOutcome, SelectionPolicy, SelectionTables, TermOutcome,
    UnknownReason, UpperBound, canonical_bytes, canonical_sha256, evaluate_collect,
    evaluate_with_tables, upper_bound_with_tables,
};

const NOW: u64 = 10_000_000;
/// The documents' freshness bound of every list.
const MAX_AGE_MS: u64 = 3_600_000;
const KIND_A: &str = "inventory.list.example_kind_a";
const KIND_B: &str = "inventory.list.example_kind_b";
const TABLE: &str = "example_values";
const MEMBERS: [&str; 26] = [
    "member.a", "member.b", "member.c", "member.d", "member.e", "member.f", "member.g", "member.h",
    "member.i", "member.j", "member.k", "member.l", "member.m", "member.n", "member.o", "member.p",
    "member.q", "member.r", "member.s", "member.t", "member.u", "member.v", "member.w", "member.x",
    "member.y", "member.z",
];

/// The table's rows: `member.a`, `member.b` and `member.d` of `domain.x` with a value,
/// `member.c` of `domain.y` only, `member.n` of `domain.x` with a null cell; no other member
/// has a row.
fn rows() -> Value {
    json!([
        ["member.a", "domain.x", 5],
        ["member.b", "domain.x", 9],
        ["member.c", "domain.y", 7],
        ["member.d", "domain.x", 12],
        ["member.n", "domain.x", null]
    ])
}

fn table(rows: Value) -> Arc<LoadedTable> {
    let value = json!({
        "schema_version": "actingcommand.table.v1",
        "game": "neutral",
        "table_id": TABLE,
        "row_key": ["member", "domain"],
        "columns": [
            {"name": "member", "type": "key"},
            {"name": "domain", "type": "enum_string", "allowed": ["domain.x", "domain.y"]},
            {"name": "value", "type": "integer", "min": -1000, "max": 1000, "nullable": true}
        ],
        "rows": rows
    });
    let bytes = serde_json::to_vec(&value).expect("table bytes");
    let table = parse_standard_pack_table(&bytes, Some("neutral"), "example_values.json")
        .expect("a valid table");
    Arc::new(LoadedTable::load(table, NOW))
}

/// A list maximum of `value` under the pin `domain.x` over `lists`, `empty` -1.
fn maximum(lists: Value) -> Value {
    json!({"source": "table_max_over_list", "table_id": TABLE, "value_column": "value",
           "pins": [{"column": "domain", "equals": {"type": "string", "value": "domain.x"}}],
           "lists": lists, "empty": {"type": "integer", "value": -1}})
}

fn kind_a() -> Value {
    maximum(json!([{"fact_key": KIND_A}]))
}

fn list_fact(fact_key: &str) -> Value {
    json!({"fact_key": fact_key,
           "value_type": {"type": "record_list", "key_column": "id", "columns": {
               "state": {"type": "enum_string", "allowed": ["state.a", "state.b"]}}},
           "max_age_ms": MAX_AGE_MS, "minimum_confidence_milli": 1000})
}

/// A gate that keeps a candidate whose `price` is above the maximum.
fn above_gate(maximum: Value, on_unknown: &str) -> Value {
    json!({"gate_id": "gate-above-best",
           "predicate": {"kind": "integer_compare", "left": {"source": "field", "field": "price"},
                         "op": "gt", "right": maximum},
           "on_unknown": {"kind": on_unknown}})
}

/// A term that scores the maximum itself.
fn best_term(term_id: &str, maximum: Value, on_unknown: &str) -> Value {
    json!({"term_id": term_id, "value": maximum, "transform": {"kind": "identity"},
           "weight_milli": 1000, "on_unknown": {"kind": on_unknown}})
}

fn document(gates: Value, scoring: Value) -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "by-list-maximum",
        "applies_to": {"candidate_layout_id": "layout-a", "outcome_keys": {
            "selected": "s", "empty": "e", "insufficient": "i", "ambiguous": "a", "unknown": "u"
        }},
        "fields": [{"name": "member", "value_type": {"type": "enum_string", "allowed": MEMBERS}},
                   {"name": "price", "value_type": {"type": "integer"}}],
        "facts": [list_fact(KIND_A), list_fact(KIND_B)],
        "tables": [{"table_id": TABLE, "path": "tables/example_values.json",
                    "row_key": ["member", "domain"],
                    "columns": {
                        "domain": {"type": "enum_string", "allowed": ["domain.x", "domain.y"]},
                        "value": {"type": "integer"}
                    }}],
        "gates": gates,
        "scoring": scoring,
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

/// The gate and the term over one maximum, both under `on_unknown`.
fn gate_and_term(maximum: Value, on_unknown: &str) -> Value {
    document(
        json!([above_gate(maximum.clone(), on_unknown)]),
        json!([best_term("term-best", maximum, on_unknown)]),
    )
}

fn policy_of(document: Value) -> SelectionPolicy {
    let policy: SelectionPolicy = serde_json::from_value(document).expect("the document decodes");
    policy.validate().expect("a valid document");
    policy
}

fn bound(policy: &SelectionPolicy, table: &Arc<LoadedTable>) -> SelectionTables {
    SelectionTables::bind(policy, std::slice::from_ref(table)).expect("the table binds")
}

/// One list's entry in a fake snapshot: its rows `(key, state)`.
fn list_entry(
    rows: &[(&str, &str)],
    published_at_unix_ms: u64,
    expires_at_unix_ms: Option<u64>,
    confidence_milli: u16,
) -> SelectionFactEntry {
    SelectionFactEntry::RecordList {
        rows: rows
            .iter()
            .map(|(key, state)| {
                BTreeMap::from([
                    ("id".to_owned(), ScalarValue::String((*key).to_owned())),
                    ("state".to_owned(), ScalarValue::String((*state).to_owned())),
                ])
            })
            .collect(),
        published_at_unix_ms,
        expires_at_unix_ms,
        confidence_milli,
    }
}

fn fresh(keys: &[&str]) -> SelectionFactEntry {
    let rows = keys.iter().map(|key| (*key, "state.a")).collect::<Vec<_>>();
    list_entry(&rows, NOW, None, 1000)
}

fn snapshot(entries: Vec<(&str, SelectionFactEntry)>) -> SelectionFactSnapshot {
    SelectionFactSnapshot {
        snapshot_id: "snapshot-a".to_owned(),
        snapshot_at_unix_ms: NOW,
        facts: entries
            .into_iter()
            .map(|(key, entry)| (key.to_owned(), entry))
            .collect(),
    }
}

/// Both lists fresh, every row `state.a`.
fn lists(kind_a: &[&str], kind_b: &[&str]) -> SelectionFactSnapshot {
    snapshot(vec![(KIND_A, fresh(kind_a)), (KIND_B, fresh(kind_b))])
}

fn candidates(prices: &[i64]) -> Vec<Candidate> {
    prices
        .iter()
        .enumerate()
        .map(|(index, price)| Candidate {
            candidate_id: format!("slot-{index}"),
            fields: BTreeMap::from([("price".to_owned(), ScalarValue::Integer(*price))]),
        })
        .collect()
}

fn decide(document: Value, facts: &SelectionFactSnapshot, prices: &[i64]) -> SelectionDecision {
    decide_on(document, rows(), facts, prices)
}

fn decide_on(
    document: Value,
    rows: Value,
    facts: &SelectionFactSnapshot,
    prices: &[i64],
) -> SelectionDecision {
    let policy = policy_of(document);
    let tables = bound(&policy, &table(rows));
    evaluate_with_tables(&policy, &tables, &candidates(prices), facts, NOW).expect("a decision")
}

/// The first list maximum of `document`, read alone: no up-front check of the lists.
fn read_alone(document: Value, facts: &SelectionFactSnapshot) -> ListMaxReading {
    let policy = policy_of(document);
    let tables = bound(&policy, &table(rows()));
    let (reference, _) = policy.list_maxima()[0];
    read_list_maximum(&policy, reference, &tables, facts, NOW)
}

/// `maximum` read alone as the term of a document.
fn read_term(maximum: Value, facts: &SelectionFactSnapshot) -> ListMaxReading {
    read_alone(
        document(
            json!([]),
            json!([best_term("term-best", maximum, "drop_candidate")]),
        ),
        facts,
    )
}

fn verdict<'a>(decision: &'a SelectionDecision, candidate_id: &str) -> &'a CandidateVerdict {
    decision
        .candidates
        .iter()
        .find(|verdict| verdict.candidate_id == candidate_id)
        .expect("the candidate's verdict")
}

fn code(reason: &DecisionReason) -> &str {
    reason.outcome.code().as_str()
}

fn list_max_reasons(decision: &SelectionDecision) -> Vec<&DecisionReason> {
    decision
        .reasons
        .iter()
        .filter(|reason| code(reason) == "selection_table_list_max")
        .collect()
}

fn value<'a>(reason: &'a DecisionReason, key: &str) -> Option<&'a Value> {
    reason.outcome.value(key)
}

fn gate_reason(verdict: &CandidateVerdict) -> Option<UnknownReason> {
    verdict.gates.iter().find_map(|gate| match &gate.outcome {
        GateOutcome::UnknownDropped { reason } => Some(*reason),
        _ => None,
    })
}

fn term_reason(verdict: &CandidateVerdict) -> Option<UnknownReason> {
    verdict.terms.iter().find_map(|term| match &term.outcome {
        TermOutcome::UnknownDropped { reason } => Some(*reason),
        _ => None,
    })
}

/// Register §1.291 S168-5a test 1: the largest value of the class over the keys of one list,
/// read once for the gate and the term, and the reason that says so right after the three the
/// chain opens with.
#[test]
fn table_list_max_reads_largest_of_class() {
    let facts = lists(&["member.a", "member.b", "member.c"], &[]);
    let reading = read_term(kind_a(), &facts);
    assert_eq!(reading.value, Ok(ScalarValue::Integer(9)));
    assert_eq!(reading.maximum_identity.as_deref(), Some("member.b"));
    assert_eq!(reading.count, 3);

    // The gate keeps a price above 9 and the term scores 9; one read serves both.
    let decision = decide(gate_and_term(kind_a(), "drop_candidate"), &facts, &[10, 9]);
    assert_eq!(decision.selected, ["slot-0"]);
    assert_eq!(verdict(&decision, "slot-0").score_milli, Some(9));
    assert_eq!(
        verdict(&decision, "slot-1").status,
        CandidateStatus::GateRejected
    );
    let reasons = list_max_reasons(&decision);
    assert_eq!(reasons.len(), 1, "{:?}", decision.reasons);
    assert_eq!(code(&decision.reasons[3]), "selection_table_list_max");
    let reason = reasons[0];
    assert_eq!(value(reason, "table_id"), Some(&json!(TABLE)));
    assert_eq!(value(reason, "table_column"), Some(&json!("value")));
    assert_eq!(value(reason, "fact_keys"), Some(&json!([KIND_A])));
    assert_eq!(value(reason, "count"), Some(&json!(3)));
    assert_eq!(value(reason, "maximum"), Some(&json!(9)));
    assert_eq!(value(reason, "maximum_identity"), Some(&json!("member.b")));
    assert_eq!(value(reason, "unknown_reason"), None);
}

/// Register §1.291 S168-5a test 2: one empty list, two empty lists and a list whose only key is
/// of another class each read the document's `empty`, a known value.
#[test]
fn table_list_max_empty_and_out_of_class() {
    let both = maximum(json!([{"fact_key": KIND_A}, {"fact_key": KIND_B}]));
    let cases = [
        ("one empty list", kind_a(), lists(&[], &["member.b"]), 0),
        ("two empty lists", both, lists(&[], &[]), 0),
        (
            "a key of another class",
            kind_a(),
            lists(&["member.c"], &[]),
            1,
        ),
    ];
    for (label, reference, facts, count) in cases {
        let reading = read_term(reference.clone(), &facts);
        assert_eq!(reading.value, Ok(ScalarValue::Integer(-1)), "{label}");
        assert_eq!(reading.maximum_identity, None, "{label}");
        assert_eq!(reading.count, count, "{label}");

        // Known: the gate decides on it and the term scores it.
        let decision = decide(gate_and_term(reference, "abort_evaluation"), &facts, &[0]);
        assert_eq!(decision.selected, ["slot-0"], "{label}");
        assert_eq!(
            verdict(&decision, "slot-0").score_milli,
            Some(-1),
            "{label}"
        );
        let reasons = list_max_reasons(&decision);
        assert_eq!(reasons.len(), 1, "{label}");
        assert_eq!(value(reasons[0], "count"), Some(&json!(count)), "{label}");
        for key in ["maximum", "maximum_identity", "unknown_reason"] {
            assert_eq!(value(reasons[0], key), None, "{label}: {key}");
        }
    }
}

/// Register §1.291 S168-5a test 3: a key the table has no row for among known ones, every key
/// unknown to the table, and a null cell of the class each read Unknown `lookup_miss`, never 0;
/// the candidate is dropped under `drop_candidate`, the evaluation unknown under
/// `abort_evaluation`.
#[test]
fn table_list_max_missing_key_is_unknown() {
    let cases = [
        ("a key with no row", &["member.a", "member.z"][..]),
        ("every key with no row", &["member.y", "member.z"][..]),
        ("a null cell of the class", &["member.a", "member.n"][..]),
    ];
    for (label, keys) in cases {
        let facts = lists(keys, &[]);
        let reading = read_term(kind_a(), &facts);
        assert_eq!(reading.value, Err(UnknownReason::LookupMiss), "{label}");
        assert_eq!(reading.maximum_identity, None, "{label}");

        // A price of 1 is above 0: had the read been 0, the candidate would rank.
        let dropped = decide(gate_and_term(kind_a(), "drop_candidate"), &facts, &[1]);
        let candidate = verdict(&dropped, "slot-0");
        assert_eq!(candidate.status, CandidateStatus::UnknownDropped, "{label}");
        assert_eq!(
            gate_reason(candidate),
            Some(UnknownReason::LookupMiss),
            "{label}"
        );
        assert!(dropped.selected.is_empty(), "{label}");
        let reasons = list_max_reasons(&dropped);
        assert_eq!(
            value(reasons[0], "unknown_reason"),
            Some(&json!("lookup_miss")),
            "{label}"
        );
        assert_eq!(value(reasons[0], "maximum"), None, "{label}");

        let aborted = decide(gate_and_term(kind_a(), "abort_evaluation"), &facts, &[1]);
        assert!(
            matches!(
                aborted.outcome,
                SelectionOutcome::Unknown {
                    reason: UnknownReason::LookupMiss,
                    ..
                }
            ),
            "{label}: {:?}",
            aborted.outcome
        );
        assert!(aborted.selected.is_empty(), "{label}");
    }
}

/// One case of [`table_list_max_validation`]: its label, its change and the refusal's code.
type RuleCase<'a> = (&'a str, Box<dyn Fn(&mut Value) + 'a>, SelectionErrorCode);

fn case<'a>(
    label: &'a str,
    change: impl Fn(&mut Value) + 'a,
    code: SelectionErrorCode,
) -> RuleCase<'a> {
    (label, Box::new(change), code)
}

/// The maxima the gate and the term of [`gate_and_term`] read.
fn set_maximum(document: &mut Value, change: &dyn Fn(&mut Value)) {
    change(&mut document["gates"][0]["predicate"]["right"]);
    change(&mut document["scoring"][0]["value"]);
}

/// `count` record lists `inventory.list.example_kind_<i>`, declared and all read by one maximum.
fn many_lists(document: &mut Value, count: usize) {
    let keys = (0..count)
        .map(|index| format!("inventory.list.example_kind_{index}"))
        .collect::<Vec<_>>();
    let facts = document["facts"].as_array_mut().expect("facts");
    facts.extend(keys.iter().map(|key| list_fact(key.as_str())));
    let lists = keys
        .iter()
        .map(|key| json!({"fact_key": key}))
        .collect::<Vec<_>>();
    set_maximum(document, &|maximum| maximum["lists"] = json!(lists));
}

/// `count` terms, the i-th reading the maximum whose `empty` is `-1 - (i % distinct)`.
fn terms(document: &mut Value, count: i64, distinct: i64) {
    document["gates"] = json!([]);
    document["scoring"] = (0..count)
        .map(|index| {
            let mut read = kind_a();
            read["empty"]["value"] = json!(-1 - index % distinct);
            best_term(&format!("term-{index}"), read, "drop_candidate")
        })
        .collect();
}

/// Register §1.291 S168-5a test 4: every rule of a list maximum's place in a document, one
/// refusal each, with no table at hand; 32 distinct references that occur more often than 32
/// times are valid; the collect option refuses a document that uses the source. The catalog
/// compile's refusal is the half of this test in `actingcommand-policy`'s `source` module, the
/// crate that compiles a catalog.
#[test]
fn table_list_max_validation() {
    use SelectionErrorCode as Code;
    let cases = vec![
        case(
            "a boolean value_column",
            |document| {
                document["tables"][0]["columns"]["flag"] = json!({"type": "boolean"});
                set_maximum(document, &|maximum| maximum["value_column"] = json!("flag"));
            },
            Code::TypeMismatch,
        ),
        case(
            "an enum_string value_column",
            |document| {
                document["tables"][0]["columns"]["tier"] =
                    json!({"type": "enum_string", "allowed": ["tier.a"]});
                set_maximum(document, &|maximum| maximum["value_column"] = json!("tier"));
            },
            Code::TypeMismatch,
        ),
        case(
            "an undeclared list",
            |document| {
                set_maximum(document, &|maximum| {
                    maximum["lists"] = json!([{"fact_key": "inventory.list.example_kind_c"}]);
                });
            },
            Code::DanglingReference,
        ),
        case(
            "a list that is no record list",
            |document| {
                document["facts"][1]["value_type"] = json!({"type": "integer"});
                set_maximum(document, &|maximum| {
                    maximum["lists"] = json!([{"fact_key": KIND_B}]);
                });
            },
            Code::TypeMismatch,
        ),
        case(
            "an undeclared table",
            |document| set_maximum(document, &|maximum| maximum["table_id"] = json!("other")),
            Code::DanglingReference,
        ),
        case(
            "empty lists",
            |document| set_maximum(document, &|maximum| maximum["lists"] = json!([])),
            Code::MissingRequiredField,
        ),
        case(
            "a 17th list",
            |document| many_lists(document, 17),
            Code::LimitExceeded,
        ),
        case(
            "a repeated fact_key",
            |document| {
                set_maximum(document, &|maximum| {
                    maximum["lists"] = json!([{"fact_key": KIND_A}, {"fact_key": KIND_A}]);
                });
            },
            Code::DuplicateId,
        ),
        case(
            "a where outside the list's values",
            |document| {
                set_maximum(document, &|maximum| {
                    maximum["lists"] =
                        json!([{"fact_key": KIND_A, "where": {"column": "state", "equals": "x"}}]);
                });
            },
            Code::TypeMismatch,
        ),
        case(
            "a gate that substitutes a verdict",
            |document| {
                document["gates"][0]["on_unknown"] =
                    json!({"kind": "substitute_verdict", "passes": false});
            },
            Code::TypeMismatch,
        ),
        case(
            "a term that substitutes a value",
            |document| {
                document["scoring"][0]["on_unknown"] =
                    json!({"kind": "substitute_milli", "value_milli": 0});
            },
            Code::TypeMismatch,
        ),
        case(
            "a tie-break key",
            |document| {
                document["tie_break"] = json!([
                    {"kind": "value", "value": kind_a(), "direction": "highest_first"},
                    {"kind": "candidate_id", "direction": "lowest_first"}
                ]);
            },
            Code::TypeMismatch,
        ),
        case(
            "a fallback_column",
            |document| {
                set_maximum(document, &|maximum| {
                    maximum["fallback_column"] = json!("value")
                });
            },
            Code::TypeMismatch,
        ),
        case(
            "a gate predicate other than integer_compare",
            |document| {
                document["gates"][0]["predicate"] =
                    json!({"kind": "integer_at_least", "value": kind_a(), "threshold": 1});
            },
            Code::TypeMismatch,
        ),
        case(
            "a lookup transform",
            |document| {
                document["scoring"][0]["transform"] = json!({"kind": "lookup",
                    "entries": [{"key": {"type": "integer", "value": 1}, "value_milli": 1}]});
            },
            Code::TypeMismatch,
        ),
        case(
            "an empty outside the safe integers",
            |document| {
                set_maximum(document, &|maximum| {
                    maximum["empty"]["value"] = json!(9_007_199_254_740_992_i64);
                });
            },
            Code::IntegerOutOfRange,
        ),
        case(
            "a 33rd distinct reference",
            |document| terms(document, 33, 33),
            Code::LimitExceeded,
        ),
    ];
    for (label, change, code) in cases {
        let mut changed = gate_and_term(kind_a(), "drop_candidate");
        change(&mut changed);
        let policy: SelectionPolicy = serde_json::from_value(changed).expect(label);
        let error = policy.validate().expect_err(label);
        assert_eq!(error.code(), code, "{label}: {error}");
    }

    // Sixteen lists are the most; 32 distinct references that occur 40 times are not 33.
    let mut sixteen = gate_and_term(kind_a(), "drop_candidate");
    many_lists(&mut sixteen, 16);
    policy_of(sixteen);
    let mut repeated = gate_and_term(kind_a(), "drop_candidate");
    terms(&mut repeated, 40, 32);
    let policy = policy_of(repeated);
    assert_eq!(policy.list_maxima().len(), 32);

    // The scheduling consumer's call refuses the document.
    let policy = policy_of(gate_and_term(kind_a(), "drop_candidate"));
    let error = evaluate_collect(&policy, &candidates(&[1]), &lists(&[], &[]), NOW)
        .expect_err("the collect option");
    assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);
    assert!(error.detail().contains("table_max_over_list"), "{error}");
}

/// Register §1.291 S168-5a test 5: the input identity holds every row the read took and the rows
/// of every listed list; a row of a key outside the union does not move it, and a document
/// without the source keeps its bytes and both identities (the DB-2 golden, computed apart from
/// this code).
#[test]
fn table_list_max_input_identity() {
    let both = maximum(json!([{"fact_key": KIND_A}, {"fact_key": KIND_B}]));
    let document = gate_and_term(both, "drop_candidate");
    let facts = lists(&["member.a", "member.b"], &["member.d"]);
    let input = |rows: Value, facts: &SelectionFactSnapshot| {
        decide_on(document.clone(), rows, facts, &[20]).input_sha256
    };
    let read = input(rows(), &facts);

    let mut contributing = rows();
    contributing[0][2] = json!(6);
    assert_ne!(input(contributing, &facts), read, "a contributing row");

    let changed_list = snapshot(vec![
        (KIND_A, fresh(&["member.a", "member.b"])),
        (
            KIND_B,
            list_entry(&[("member.d", "state.b")], NOW, None, 1000),
        ),
    ]);
    assert_ne!(input(rows(), &changed_list), read, "a row of a listed list");

    let mut unlisted = rows();
    unlisted[4][2] = json!(4);
    assert_eq!(input(unlisted, &facts), read, "a row of an unlisted key");

    // The DB-2 golden: a document without the source keeps its bytes and both identities.
    let golden = json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "by-table",
        "applies_to": {"candidate_layout_id": "layout-a", "outcome_keys": {
            "selected": "s", "empty": "e", "insufficient": "i", "ambiguous": "a", "unknown": "u"
        }},
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": ["unit.a", "unit.b"]}}],
        "facts": [],
        "gates": [],
        "scoring": [{"term_id": "t", "value": {"source": "field", "field": "member"},
                     "transform": {"kind": "lookup",
                                   "entries": [{"key": {"type": "string", "value": "unit.a"},
                                                "value_milli": 5}],
                                   "default_milli": 1},
                     "weight_milli": 1000, "on_unknown": {"kind": "drop_candidate"}}],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    });
    let policy = policy_of(golden);
    assert_eq!(
        String::from_utf8(canonical_bytes(&policy).expect("bytes")).expect("UTF-8"),
        r#"{"applies_to":{"candidate_layout_id":"layout-a","outcome_keys":{"ambiguous":"a","empty":"e","insufficient":"i","selected":"s","unknown":"u"}},"facts":[],"fields":[{"name":"member","value_type":{"allowed":["unit.a","unit.b"],"type":"enum_string"}}],"gates":[],"policy_id":"by-table","schema_version":"actingcommand.selection-policy.v1","scoring":[{"on_unknown":{"kind":"drop_candidate"},"term_id":"t","transform":{"default_milli":1,"entries":[{"key":{"type":"string","value":"unit.a"},"value_milli":5}],"kind":"lookup"},"value":{"field":"member","source":"field"},"weight_milli":1000}],"selection":{"mode":"exactly_one","required_count":1},"tie_break":[{"direction":"lowest_first","kind":"candidate_id"}]}"#
    );
    assert_eq!(
        canonical_sha256(&policy).expect("identity"),
        "sha256:a83ca293574f1362a8a0efd80abfcc3312c25f89dc546422c01e8e8b91f1f791"
    );
    let golden_candidates = ["c1", "c2"]
        .into_iter()
        .zip(["unit.a", "unit.b"])
        .map(|(candidate_id, member)| Candidate {
            candidate_id: candidate_id.to_owned(),
            fields: BTreeMap::from([("member".to_owned(), ScalarValue::String(member.to_owned()))]),
        })
        .collect::<Vec<_>>();
    let golden_facts = SelectionFactSnapshot {
        snapshot_id: "snapshot-a".to_owned(),
        snapshot_at_unix_ms: 1000,
        facts: BTreeMap::new(),
    };
    let decision =
        crate::evaluate(&policy, &golden_candidates, &golden_facts, 1000).expect("a decision");
    assert_eq!(
        decision.input_sha256,
        "sha256:a4d6f37fa6dd921ac8e22cb51dc2e77abb76e0e7ed075af3fe29b2f8339e79fb"
    );
    assert!(list_max_reasons(&decision).is_empty());
}

/// Register §1.291 S168-5a test 6: two lists read over the union of their keys, a key both hold
/// looked up and counted once, and the lists named in the order of `lists`.
#[test]
fn table_list_max_union_of_lists() {
    let facts = lists(&["member.a", "member.b"], &["member.b", "member.d"]);
    for (order, fact_keys) in [
        (
            json!([{"fact_key": KIND_A}, {"fact_key": KIND_B}]),
            [KIND_A, KIND_B],
        ),
        (
            json!([{"fact_key": KIND_B}, {"fact_key": KIND_A}]),
            [KIND_B, KIND_A],
        ),
    ] {
        let reading = read_term(maximum(order.clone()), &facts);
        assert_eq!(reading.value, Ok(ScalarValue::Integer(12)));
        assert_eq!(reading.maximum_identity.as_deref(), Some("member.d"));
        assert_eq!(reading.count, 3);

        let decision = decide(
            gate_and_term(maximum(order), "drop_candidate"),
            &facts,
            &[13],
        );
        assert_eq!(verdict(&decision, "slot-0").score_milli, Some(12));
        let reasons = list_max_reasons(&decision);
        assert_eq!(reasons.len(), 1);
        assert_eq!(value(reasons[0], "count"), Some(&json!(3)));
        assert_eq!(value(reasons[0], "fact_keys"), Some(&json!(fact_keys)));
        assert_eq!(
            value(reasons[0], "maximum_identity"),
            Some(&json!("member.d"))
        );
    }
}

/// Register §1.291 S168-5a test 7: (1) read alone, an unusable list makes the reference Unknown
/// whatever the other holds, and with two unusable lists the reason is the first's in the order
/// of `lists`; (2) a whole evaluation of a document that declares the two lists in the opposite
/// order of `lists` ends `unknown` at the up-front check, before any candidate and any list
/// maximum, with the reason of the first unusable list in the order of `facts[]`.
#[test]
fn table_list_max_unusable_list_is_unknown() {
    let stale = || list_entry(&[("member.a", "state.a")], NOW - MAX_AGE_MS - 1, None, 1000);
    let expired = || list_entry(&[("member.a", "state.a")], NOW, Some(NOW - 1), 1000);
    let low = || list_entry(&[("member.a", "state.a")], NOW, None, 999);
    let in_order = maximum(json!([{"fact_key": KIND_A}, {"fact_key": KIND_B}]));
    let reversed = maximum(json!([{"fact_key": KIND_B}, {"fact_key": KIND_A}]));

    // (1) One reference read alone, its lists handed in a fake snapshot.
    let b_stale = snapshot(vec![
        (KIND_A, fresh(&["member.a", "member.b"])),
        (KIND_B, stale()),
    ]);
    assert_eq!(
        read_term(in_order.clone(), &b_stale).value,
        Err(UnknownReason::FactStale)
    );
    let both = snapshot(vec![(KIND_B, low())]);
    assert_eq!(
        read_term(in_order.clone(), &both).value,
        Err(UnknownReason::FactMissing)
    );
    assert_eq!(
        read_term(reversed, &both).value,
        Err(UnknownReason::FactLowConfidence)
    );

    // (2) A whole evaluation: `facts[]` declares kind_b first, `lists` reads kind_a first.
    let mut document = gate_and_term(in_order, "drop_candidate");
    document["facts"] = json!([list_fact(KIND_B), list_fact(KIND_A)]);
    let facts = snapshot(vec![(KIND_A, expired()), (KIND_B, stale())]);
    assert_eq!(
        read_alone(document.clone(), &facts).value,
        Err(UnknownReason::FactExpired)
    );
    let decision = decide(document, &facts, &[20, 30]);
    assert!(
        matches!(
            decision.outcome,
            SelectionOutcome::Unknown {
                reason: UnknownReason::FactStale,
                ..
            }
        ),
        "{:?}",
        decision.outcome
    );
    assert!(decision.candidates.is_empty());
    assert!(list_max_reasons(&decision).is_empty());
}

/// Register §1.291 S168-5a test 8: a `where` leaves out a row of another state whose key the
/// table has no row for; that key is not looked up, is no miss and is not counted.
#[test]
fn table_list_max_where_skips_rows() {
    let facts = snapshot(vec![
        (
            KIND_A,
            list_entry(
                &[
                    ("member.a", "state.a"),
                    ("member.q", "state.b"),
                    ("member.b", "state.a"),
                ],
                NOW,
                None,
                1000,
            ),
        ),
        (KIND_B, fresh(&[])),
    ]);
    let kept = maximum(json!([{"fact_key": KIND_A,
                               "where": {"column": "state", "equals": "state.a"}}]));
    let reading = read_term(kept.clone(), &facts);
    assert_eq!(reading.value, Ok(ScalarValue::Integer(9)));
    assert_eq!(reading.count, 2);
    assert_eq!(reading.maximum_identity.as_deref(), Some("member.b"));

    // Without the `where`, `member.q` is a key of the union and the table has no row for it.
    assert_eq!(
        read_term(kind_a(), &facts).value,
        Err(UnknownReason::LookupMiss)
    );

    let decision = decide(gate_and_term(kept, "abort_evaluation"), &facts, &[10]);
    assert_eq!(decision.selected, ["slot-0"]);
    assert_eq!(
        value(list_max_reasons(&decision)[0], "count"),
        Some(&json!(2))
    );
}

/// Register §1.291 S168-5a test 9: an Unknown `lookup_miss` maximum (a key of the union with no
/// row; a null cell of the class), read by an `integer_compare` gate and by a term, follows each
/// rule's declared action: `drop_candidate` drops every candidate it assesses, and that is no
/// static table drop; `abort_evaluation` makes the evaluation `unknown` with the rule; no
/// candidate ranks on the value, nothing reads 0, and the upper bound follows the same action.
#[test]
fn table_list_max_unknown_follows_the_declared_action() {
    for keys in [&["member.a", "member.z"][..], &["member.a", "member.n"][..]] {
        let facts = lists(keys, &[]);
        // Prices above 0 and below the known values: a read of 0 would rank them.
        let prices = [1, 2];
        let gate_only = |on_unknown: &str| {
            document(
                json!([above_gate(kind_a(), on_unknown)]),
                json!([best_term(
                    "term-price",
                    json!({"source": "field", "field": "price"}),
                    "drop_candidate"
                )]),
            )
        };
        let term_only = |on_unknown: &str| {
            document(
                json!([]),
                json!([best_term("term-best", kind_a(), on_unknown)]),
            )
        };

        for (label, dropping) in [
            ("gate", gate_only("drop_candidate")),
            ("term", term_only("drop_candidate")),
        ] {
            let decision = decide(dropping, &facts, &prices);
            for candidate_id in ["slot-0", "slot-1"] {
                let dropped = verdict(&decision, candidate_id);
                assert_eq!(dropped.status, CandidateStatus::UnknownDropped, "{label}");
                let reason = if label == "gate" {
                    gate_reason(dropped)
                } else {
                    term_reason(dropped)
                };
                assert_eq!(reason, Some(UnknownReason::LookupMiss), "{label}");
                assert!(!dropped.static_table_drop, "{label}");
                assert_eq!(dropped.rank, None, "{label}");
            }
            assert!(
                matches!(decision.outcome, SelectionOutcome::Empty),
                "{label}"
            );
        }

        for (label, aborting, rule_id, abort_code) in [
            (
                "gate",
                gate_only("abort_evaluation"),
                "gate-above-best",
                "gate_unknown_aborted",
            ),
            (
                "term",
                term_only("abort_evaluation"),
                "term-best",
                "term_unknown_aborted",
            ),
        ] {
            let decision = decide(aborting, &facts, &prices);
            assert!(
                matches!(
                    decision.outcome,
                    SelectionOutcome::Unknown {
                        reason: UnknownReason::LookupMiss,
                        ..
                    }
                ),
                "{label}: {:?}",
                decision.outcome
            );
            assert!(decision.selected.is_empty(), "{label}");
            assert!(
                decision
                    .candidates
                    .iter()
                    .all(|verdict| verdict.status != CandidateStatus::Ranked
                        && verdict.score_milli.is_none()),
                "{label}"
            );
            let abort = decision
                .candidates
                .iter()
                .flat_map(|verdict| &verdict.reasons)
                .find(|reason| code(reason) == abort_code)
                .expect("the rule that ended the evaluation");
            assert_eq!(value(abort, "rule_id"), Some(&json!(rule_id)), "{label}");
            let unknown = decision
                .reasons
                .iter()
                .position(|reason| code(reason) == "selection_unknown")
                .expect("the unknown outcome's reason");
            assert_eq!(code(&decision.reasons[3]), "selection_table_list_max");
            assert!(unknown > 3, "{label}");
        }

        // The upper bound: an unknown maximum drops the identity under `drop_candidate` and has
        // no finite bound under `abort_evaluation`.
        let domains = BTreeMap::from([
            ("member".to_owned(), FieldDomain::Identity),
            (
                "price".to_owned(),
                FieldDomain::Integer { min: 0, max: 100 },
            ),
        ]);
        let upper = |document: Value| {
            let policy = policy_of(document);
            let tables = bound(&policy, &table(rows()));
            upper_bound_with_tables(
                &policy,
                &tables,
                &facts,
                NOW,
                &["member.a".to_owned()],
                &domains,
            )
            .expect("a bound")
        };
        assert_eq!(upper(gate_only("drop_candidate")), UpperBound::NoneEligible);
        assert_eq!(upper(term_only("drop_candidate")), UpperBound::NoneEligible);
        assert_eq!(upper(gate_only("abort_evaluation")), UpperBound::Unbounded);
        assert_eq!(upper(term_only("abort_evaluation")), UpperBound::Unbounded);
    }
}
