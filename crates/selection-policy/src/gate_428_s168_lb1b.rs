// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #428 S168-LB1b (register §1.309 rows LB1-7 to LB1-11, §1.312; contracts/
//! selection-policy.md, "Lower bound of a list maximum"): a list maximum that names a
//! `lower_bound_column` reads exact M, at least L, or Unknown only when nothing is known of the
//! class, and every reader takes "at least L" as the range [L, +∞). Each table is parsed from
//! bytes here and fixed at one instant, each list is a fake snapshot and every id is made up: no
//! file, clock or device.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use actingcommand_contract::{TableMismatch, parse_standard_pack_table};
use serde_json::{Value, json};

use crate::list_max::{ListMaxBound, ListMaxReading, read_list_maximum};
use crate::{
    Candidate, CandidateStatus, CandidateVerdict, DecisionReason, FieldDomain, GateOutcome,
    LoadedTable, ScalarValue, SelectionDecision, SelectionErrorCode, SelectionFactEntry,
    SelectionFactSnapshot, SelectionOutcome, SelectionPolicy, SelectionTables, TermOutcome,
    UnknownReason, UpperBound, ValueRef, canonical_bytes, canonical_sha256, evaluate_collect,
    evaluate_with_tables, upper_bound_with_tables,
};

const NOW: u64 = 10_000_000;
/// The documents' freshness bound of every list.
const MAX_AGE_MS: u64 = 3_600_000;
const KIND_A: &str = "inventory.list.example_kind_a";
const KIND_B: &str = "inventory.list.example_kind_b";
const TABLE: &str = "example_values";
/// The bound column a reference that opts in names.
const BOUND: &str = "value_at_least";
const MEMBERS: [&str; 26] = [
    "member.a", "member.b", "member.c", "member.d", "member.e", "member.f", "member.g", "member.h",
    "member.i", "member.j", "member.k", "member.l", "member.m", "member.n", "member.o", "member.p",
    "member.q", "member.r", "member.s", "member.t", "member.u", "member.v", "member.w", "member.x",
    "member.y", "member.z",
];
/// The keys every `selection_table_list_max` carries.
const READ_KEYS: [&str; 4] = ["table_id", "table_column", "fact_keys", "count"];
/// No list key, and no key beyond [`READ_KEYS`].
const NONE: &[&str] = &[];

/// The table's rows `[member, domain, value, value_at_least]`, `domain.x` the pinned class:
/// `member.a` (5) and `member.b` (9) with a value; `member.c` of `domain.y` only; `member.d` and
/// `member.e` with no value and the bounds 12 and 3; `member.f` with neither; `member.g` of
/// `domain.y` with a bound of 50. No other member has a row.
fn rows() -> Value {
    json!([
        ["member.a", "domain.x", 5, null],
        ["member.b", "domain.x", 9, null],
        ["member.c", "domain.y", 7, null],
        ["member.d", "domain.x", null, 12],
        ["member.e", "domain.x", null, 3],
        ["member.f", "domain.x", null, null],
        ["member.g", "domain.y", null, 50]
    ])
}

/// The first `count` cells of each of `rows`.
fn first_cells(rows: Value, count: usize) -> Value {
    rows.as_array()
        .expect("rows")
        .iter()
        .map(|row| Value::Array(row.as_array().expect("a row")[..count].to_vec()))
        .collect()
}

fn integer_column(name: &str) -> Value {
    json!({"name": name, "type": "integer", "min": -1000, "max": 1000, "nullable": true})
}

/// A table file with the key, the class and then `columns`, holding `rows`, loaded at NOW.
fn table_of(columns: Vec<Value>, rows: Value) -> Arc<LoadedTable> {
    let mut all = vec![
        json!({"name": "member", "type": "key"}),
        json!({"name": "domain", "type": "enum_string", "allowed": ["domain.x", "domain.y"]}),
    ];
    all.extend(columns);
    let value = json!({
        "schema_version": "actingcommand.table.v1",
        "game": "neutral",
        "table_id": TABLE,
        "row_key": ["member", "domain"],
        "columns": all,
        "rows": rows
    });
    let bytes = serde_json::to_vec(&value).expect("table bytes");
    let table = parse_standard_pack_table(&bytes, Some("neutral"), "example_values.json")
        .expect("a valid table");
    Arc::new(LoadedTable::load(table, NOW))
}

/// The table of [`rows`]: a value column and a bound column, both nullable integers.
fn table() -> Arc<LoadedTable> {
    table_of(vec![integer_column("value"), integer_column(BOUND)], rows())
}

/// A list maximum of `value` under the pin `domain.x` over `lists`, `empty` -1, with no bound
/// column: the reference of S168-5a.
fn plain(lists: Value) -> Value {
    json!({"source": "table_max_over_list", "table_id": TABLE, "value_column": "value",
           "pins": [{"column": "domain", "equals": {"type": "string", "value": "domain.x"}}],
           "lists": lists, "empty": {"type": "integer", "value": -1}})
}

/// [`plain`] with the bound column: the reference opts in.
fn opted(lists: Value) -> Value {
    let mut reference = plain(lists);
    reference["lower_bound_column"] = json!(BOUND);
    reference
}

fn over_a() -> Value {
    opted(json!([{"fact_key": KIND_A}]))
}

fn over_b() -> Value {
    opted(json!([{"fact_key": KIND_B}]))
}

fn plain_a() -> Value {
    plain(json!([{"fact_key": KIND_A}]))
}

fn price() -> Value {
    json!({"source": "field", "field": "price"})
}

fn list_fact(fact_key: &str) -> Value {
    json!({"fact_key": fact_key,
           "value_type": {"type": "record_list", "key_column": "id", "columns": {
               "state": {"type": "enum_string", "allowed": ["state.a", "state.b"]}}},
           "max_age_ms": MAX_AGE_MS, "minimum_confidence_milli": 1000})
}

/// The gate `gate-compare`: `left op right`.
fn compare_gate(left: Value, op: &str, right: Value, on_unknown: &str) -> Value {
    json!({"gate_id": "gate-compare",
           "predicate": {"kind": "integer_compare", "left": left, "op": op, "right": right},
           "on_unknown": {"kind": on_unknown}})
}

/// `gate` with `mul` 2 and `add` -3: its right side is `right × 2 − 3`.
fn scaled(mut gate: Value) -> Value {
    gate["predicate"]["mul"] = json!(2);
    gate["predicate"]["add"] = json!(-3);
    gate
}

fn term(
    term_id: &str,
    value: Value,
    transform: Value,
    weight_milli: i64,
    on_unknown: &str,
) -> Value {
    json!({"term_id": term_id, "value": value, "transform": transform,
           "weight_milli": weight_milli, "on_unknown": {"kind": on_unknown}})
}

fn identity() -> Value {
    json!({"kind": "identity"})
}

fn threshold(at_least: i64) -> Value {
    json!({"kind": "threshold", "at_least": at_least, "then_milli": 300, "otherwise_milli": 700})
}

fn document(gates: Value, scoring: Value) -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "by-lower-bound",
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
                        "value": {"type": "integer"},
                        "value_at_least": {"type": "integer"}
                    }}],
        "gates": gates,
        "scoring": scoring,
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

/// A gate `maximum ge price` and a term that scores `maximum`, both under `on_unknown`.
fn gate_and_term(maximum: Value, on_unknown: &str) -> Value {
    document(
        json!([compare_gate(maximum.clone(), "ge", price(), on_unknown)]),
        json!([term("term-best", maximum, identity(), 1000, on_unknown)]),
    )
}

/// A document with one term, `term-best`, that scores `maximum` through `identity`.
fn scoring_only(maximum: Value) -> Value {
    document(
        json!([]),
        json!([term(
            "term-best",
            maximum,
            identity(),
            1000,
            "drop_candidate"
        )]),
    )
}

/// A document with the one gate `gate`.
fn gate_only(gate: Value) -> Value {
    document(json!([gate]), json!([]))
}

fn policy_of(document: Value) -> SelectionPolicy {
    let policy: SelectionPolicy = serde_json::from_value(document).expect("the document decodes");
    policy.validate().expect("a valid document");
    policy
}

fn bound_tables(policy: &SelectionPolicy) -> SelectionTables {
    let table = table();
    SelectionTables::bind(policy, std::slice::from_ref(&table)).expect("the table binds")
}

/// Both lists fresh, every row `state.a`.
fn lists(kind_a: &[&str], kind_b: &[&str]) -> SelectionFactSnapshot {
    let entry = |keys: &[&str]| SelectionFactEntry::RecordList {
        rows: keys
            .iter()
            .map(|key| {
                BTreeMap::from([
                    ("id".to_owned(), ScalarValue::String((*key).to_owned())),
                    (
                        "state".to_owned(),
                        ScalarValue::String("state.a".to_owned()),
                    ),
                ])
            })
            .collect(),
        published_at_unix_ms: NOW,
        expires_at_unix_ms: None,
        confidence_milli: 1000,
    };
    SelectionFactSnapshot {
        snapshot_id: "snapshot-a".to_owned(),
        snapshot_at_unix_ms: NOW,
        facts: BTreeMap::from([
            (KIND_A.to_owned(), entry(kind_a)),
            (KIND_B.to_owned(), entry(kind_b)),
        ]),
    }
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
    let policy = policy_of(document);
    let tables = bound_tables(&policy);
    evaluate_with_tables(&policy, &tables, &candidates(prices), facts, NOW).expect("a decision")
}

/// The first list maximum of `document`, read alone.
fn read_first(document: Value, facts: &SelectionFactSnapshot) -> ListMaxReading {
    let policy = policy_of(document);
    let tables = bound_tables(&policy);
    let (reference, _) = policy.list_maxima()[0];
    read_list_maximum(&policy, reference, &tables, facts, NOW)
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

fn codes(decision: &SelectionDecision) -> Vec<&str> {
    decision.reasons.iter().map(code).collect()
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

/// The keys a reason carries.
fn keys_of(reason: &DecisionReason) -> BTreeSet<&str> {
    reason.outcome.values().keys().map(String::as_str).collect()
}

/// [`READ_KEYS`] and `extra`.
fn read_keys(extra: &[&'static str]) -> BTreeSet<&'static str> {
    READ_KEYS.into_iter().chain(extra.iter().copied()).collect()
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

/// The decision of `gated` over two candidates: the first's price the bound settles, to
/// `status`; the second's price it does not, so that candidate is dropped, `lookup_miss`, and
/// that is no static table drop.
fn assert_settles(
    gated: Value,
    facts: &SelectionFactSnapshot,
    label: &str,
    settled: (i64, CandidateStatus),
    unsettled: i64,
) {
    let decision = decide(gated, facts, &[settled.0, unsettled]);
    let first = verdict(&decision, "slot-0");
    assert_eq!(first.status, settled.1, "{label}: {:?}", first.gates);
    let second = verdict(&decision, "slot-1");
    assert_eq!(
        second.status,
        CandidateStatus::UnknownDropped,
        "{label}: {:?}",
        second.gates
    );
    assert_eq!(
        gate_reason(second),
        Some(UnknownReason::LookupMiss),
        "{label}"
    );
    assert!(!second.static_table_drop, "{label}");
}

/// One case of [`lower_bound_column_validation`]: its label, its change and the refusal's code.
type RuleCase<'a> = (&'a str, Box<dyn Fn(&mut Value) + 'a>, SelectionErrorCode);

fn case<'a>(
    label: &'a str,
    change: impl Fn(&mut Value) + 'a,
    code: SelectionErrorCode,
) -> RuleCase<'a> {
    (label, Box::new(change), code)
}

/// The bound column of both references of [`gate_and_term`].
fn set_bound(document: &mut Value, column: &str) {
    document["gates"][0]["predicate"]["left"]["lower_bound_column"] = json!(column);
    document["scoring"][0]["value"]["lower_bound_column"] = json!(column);
}

/// Register §1.312 S168-LB1b test 1 (LB1-7): each rule of `lower_bound_column` refuses at its
/// place with the detail `table_max_over_list=lower_bound_column`: an undeclared column is a
/// dangling reference, another type or the `value_column` itself a type mismatch. The binding
/// refuses a file without the column (`column_absent`) and one where it has another type
/// (`column_type`), as for the `value_column`; the collect option still refuses the source.
#[test]
fn lower_bound_column_validation() {
    use SelectionErrorCode as Code;
    let written = gate_and_term(over_a(), "drop_candidate");
    policy_of(written.clone());
    let cases = vec![
        case(
            "an undeclared column",
            |document| set_bound(document, "value_at_most"),
            Code::DanglingReference,
        ),
        case(
            "a boolean column",
            |document| {
                document["tables"][0]["columns"]["flag"] = json!({"type": "boolean"});
                set_bound(document, "flag");
            },
            Code::TypeMismatch,
        ),
        case(
            "an enum_string column",
            |document| {
                document["tables"][0]["columns"]["tier"] =
                    json!({"type": "enum_string", "allowed": ["tier.a"]});
                set_bound(document, "tier");
            },
            Code::TypeMismatch,
        ),
        case(
            "the class column",
            |document| set_bound(document, "domain"),
            Code::TypeMismatch,
        ),
        case(
            "the value_column itself",
            |document| set_bound(document, "value"),
            Code::TypeMismatch,
        ),
    ];
    for (label, change, code) in cases {
        let mut changed = written.clone();
        change(&mut changed);
        let policy: SelectionPolicy = serde_json::from_value(changed).expect(label);
        let error = policy.validate().expect_err(label);
        assert_eq!(error.code(), code, "{label}: {error}");
        assert!(
            error
                .detail()
                .contains("table_max_over_list=lower_bound_column"),
            "{label}: {error}"
        );
    }

    // The binding checks the bound column as it checks the value column.
    let policy = policy_of(written);
    let without = table_of(vec![integer_column("value")], first_cells(rows(), 3));
    let absent = SelectionTables::bind(&policy, std::slice::from_ref(&without))
        .expect_err("a file without the bound column");
    assert_eq!(absent.mismatch, TableMismatch::ColumnAbsent);
    assert_eq!(absent.column.as_deref(), Some(BOUND));
    let boolean = table_of(
        vec![
            integer_column("value"),
            json!({"name": BOUND, "type": "boolean", "nullable": true}),
        ],
        json!([
            ["member.a", "domain.x", 5, true],
            ["member.b", "domain.x", 9, null]
        ]),
    );
    let other_type = SelectionTables::bind(&policy, std::slice::from_ref(&boolean))
        .expect_err("a bound column of another type");
    assert_eq!(other_type.mismatch, TableMismatch::ColumnType);
    assert_eq!(other_type.column.as_deref(), Some(BOUND));

    // The scheduling consumer's call still refuses the source.
    let error = evaluate_collect(&policy, &candidates(&[1]), &lists(&[], &[]), NOW)
        .expect_err("the collect option");
    assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);
}

/// Register §1.312 S168-LB1b test 2 (LB1-7, identity): a document without the member keeps the
/// bytes it was written with, so its identity, and reads and decides as S168-5a's: a key with no
/// row and a null cell are Unknown `lookup_miss` (also where the bound column holds a bound), and
/// no reason carries `bound`, `unknown_count`, `lower_bound` or `lower_bound_identity` or a
/// `selection_decided_on_lower_bound`. Writing the member writes it, and makes another
/// reference.
#[test]
fn lower_bound_absent_reads_as_before() {
    let written = gate_and_term(plain_a(), "drop_candidate");
    let policy = policy_of(written.clone());
    let bytes = canonical_bytes(&policy).expect("bytes");
    assert_eq!(bytes, canonical_bytes(&written).expect("the written bytes"));
    assert!(
        !String::from_utf8(bytes)
            .expect("UTF-8")
            .contains("lower_bound_column")
    );
    assert_eq!(
        canonical_sha256(&policy).expect("identity"),
        canonical_sha256(&written).expect("identity")
    );
    let opted_in = policy_of(gate_and_term(over_a(), "drop_candidate"));
    let opted_bytes = String::from_utf8(canonical_bytes(&opted_in).expect("bytes")).expect("UTF-8");
    assert!(
        opted_bytes.contains(r#""lower_bound_column":"value_at_least""#),
        "{opted_bytes}"
    );
    assert_ne!(
        canonical_sha256(&opted_in).expect("identity"),
        canonical_sha256(&policy).expect("identity")
    );
    let both = document(
        json!([]),
        json!([
            term("term-plain", plain_a(), identity(), 1000, "drop_candidate"),
            term("term-opted", over_a(), identity(), 1000, "drop_candidate")
        ]),
    );
    assert_eq!(policy_of(both).list_maxima().len(), 2);

    let cases = [
        (
            "a key with no row",
            &["member.a", "member.b", "member.z"][..],
            Err(UnknownReason::LookupMiss),
            &["unknown_reason"][..],
        ),
        (
            "a null cell with a bound",
            &["member.a", "member.d"][..],
            Err(UnknownReason::LookupMiss),
            &["unknown_reason"][..],
        ),
        (
            "every key with a value",
            &["member.a", "member.b", "member.c"][..],
            Ok(ScalarValue::Integer(9)),
            &["maximum", "maximum_identity"][..],
        ),
        (
            "no key of the class",
            &["member.c", "member.g"][..],
            Ok(ScalarValue::Integer(-1)),
            NONE,
        ),
    ];
    for (label, keys, read, extra) in cases {
        let facts = lists(keys, &[]);
        let reading = read_first(scoring_only(plain_a()), &facts);
        assert_eq!(reading.value, read, "{label}");
        assert_eq!(reading.bound, None, "{label}");
        assert!(!reading.at_least(), "{label}");

        let decision = decide(gate_and_term(plain_a(), "drop_candidate"), &facts, &[1]);
        let reasons = list_max_reasons(&decision);
        assert_eq!(reasons.len(), 1, "{label}");
        assert_eq!(keys_of(reasons[0]), read_keys(extra), "{label}");
        assert!(
            !codes(&decision).contains(&"selection_decided_on_lower_bound"),
            "{label}"
        );
        if read.is_err() {
            let dropped = verdict(&decision, "slot-0");
            assert_eq!(dropped.status, CandidateStatus::UnknownDropped, "{label}");
            assert_eq!(
                gate_reason(dropped),
                Some(UnknownReason::LookupMiss),
                "{label}"
            );
        }
    }
}

/// Register §1.312 S168-LB1b test 3 (LB1-8): the opted-in read. Exact M when every key of the
/// class has a value; at least L from a known value, from a lower bound and from both, L the
/// largest of them with the smallest key that gives it; a bound of another class adds nothing;
/// Unknown `lookup_miss` only when nothing of the class is known; `empty`, exact, with no key of
/// the class.
#[test]
fn lower_bound_read() {
    use ListMaxBound::{AtLeast, Exact};
    let cases = [
        (
            "exact",
            &["member.a", "member.b", "member.c"][..],
            Ok(ScalarValue::Integer(9)),
            Some("member.b"),
            Some(Exact),
            0,
        ),
        (
            "at least from a known value",
            &["member.a", "member.b", "member.z"][..],
            Ok(ScalarValue::Integer(9)),
            Some("member.b"),
            Some(AtLeast),
            1,
        ),
        (
            "at least from a lower bound",
            &["member.d", "member.y"][..],
            Ok(ScalarValue::Integer(12)),
            Some("member.d"),
            Some(AtLeast),
            2,
        ),
        (
            "at least from both, a bound above the values",
            &[
                "member.a", "member.b", "member.d", "member.e", "member.f", "member.z",
            ][..],
            Ok(ScalarValue::Integer(12)),
            Some("member.d"),
            Some(AtLeast),
            4,
        ),
        (
            "at least from both, a value above the bound",
            &["member.b", "member.e"][..],
            Ok(ScalarValue::Integer(9)),
            Some("member.b"),
            Some(AtLeast),
            1,
        ),
        (
            "a bound of another class",
            &["member.a", "member.g"][..],
            Ok(ScalarValue::Integer(5)),
            Some("member.a"),
            Some(Exact),
            0,
        ),
        (
            "nothing known",
            &["member.f", "member.z"][..],
            Err(UnknownReason::LookupMiss),
            None,
            None,
            0,
        ),
        (
            "nothing known beside a key of another class",
            &["member.c", "member.f"][..],
            Err(UnknownReason::LookupMiss),
            None,
            None,
            0,
        ),
        (
            "empty, a key of another class",
            &["member.c"][..],
            Ok(ScalarValue::Integer(-1)),
            None,
            Some(Exact),
            0,
        ),
        (
            "empty, no key",
            NONE,
            Ok(ScalarValue::Integer(-1)),
            None,
            Some(Exact),
            0,
        ),
    ];
    for (label, keys, read, identity, bound, unknown_count) in cases {
        let reading = read_first(scoring_only(over_a()), &lists(keys, &[]));
        assert_eq!(reading.value, read, "{label}");
        assert_eq!(reading.maximum_identity.as_deref(), identity, "{label}");
        assert_eq!(reading.bound, bound, "{label}");
        assert_eq!(reading.at_least(), bound == Some(AtLeast), "{label}");
        assert_eq!(reading.unknown_count, unknown_count, "{label}");
        assert_eq!(reading.count, keys.len(), "{label}");
    }
}

/// Register §1.312 S168-LB1b test 4 (LB1-9): with the list maximum on the left, read as at
/// least 9, each operator is settled where every value of [9, +∞) agrees (`ge` true at 9, `gt`
/// at 8, `le` and `eq` false at 8, `lt` false at 9, `ne` true at 8) and Unknown `lookup_miss` one
/// step further, which the gate's `on_unknown` decides; an exact 9 still compares exactly.
#[test]
fn lower_bound_compare_left() {
    use CandidateStatus::{GateRejected, Ranked};
    let at_least_nine = lists(&["member.a", "member.b", "member.z"], &[]);
    for (op, settled, status, unsettled) in [
        ("ge", 9, Ranked, 10),
        ("gt", 8, Ranked, 9),
        ("le", 8, GateRejected, 9),
        ("lt", 9, GateRejected, 10),
        ("eq", 8, GateRejected, 9),
        ("ne", 8, Ranked, 9),
    ] {
        let gated = gate_only(compare_gate(over_a(), op, price(), "drop_candidate"));
        assert_settles(gated, &at_least_nine, op, (settled, status), unsettled);
    }

    // An exact 9: `le` holds at 9, where the range settles nothing.
    let exact_nine = lists(&["member.a", "member.b"], &[]);
    let exact = decide(
        gate_only(compare_gate(over_a(), "le", price(), "drop_candidate")),
        &exact_nine,
        &[9],
    );
    assert_eq!(verdict(&exact, "slot-0").status, Ranked);

    // Under `abort_evaluation` an unsettled comparison ends the evaluation unknown.
    let aborted = decide(
        gate_only(compare_gate(over_a(), "ge", price(), "abort_evaluation")),
        &at_least_nine,
        &[10],
    );
    assert!(
        matches!(
            aborted.outcome,
            SelectionOutcome::Unknown {
                reason: UnknownReason::LookupMiss,
                ..
            }
        ),
        "{:?}",
        aborted.outcome
    );
}

/// Register §1.312 S168-LB1b test 5 (LB1-9): with the list maximum on the right, `mul` 2 and
/// `add` -3, the range [9, +∞) is [15, +∞): `le` true at 15, `lt` at 14, `ge` and `eq` false at
/// 14, `gt` false at 15, `ne` true at 14, and each Unknown `lookup_miss` one step further; an
/// exact 9 is 15 exactly.
#[test]
fn lower_bound_compare_right() {
    use CandidateStatus::{GateRejected, Ranked};
    let at_least_nine = lists(&["member.a", "member.b", "member.z"], &[]);
    for (op, settled, status, unsettled) in [
        ("le", 15, Ranked, 16),
        ("lt", 14, Ranked, 15),
        ("ge", 14, GateRejected, 15),
        ("gt", 15, GateRejected, 16),
        ("eq", 14, GateRejected, 15),
        ("ne", 14, Ranked, 15),
    ] {
        let gated = gate_only(scaled(compare_gate(
            price(),
            op,
            over_a(),
            "drop_candidate",
        )));
        assert_settles(gated, &at_least_nine, op, (settled, status), unsettled);
    }

    // An exact 9 on the right is 15: `ge` holds at 15, where the range settles nothing.
    let exact_nine = lists(&["member.a", "member.b"], &[]);
    let exact = decide(
        gate_only(scaled(compare_gate(
            price(),
            "ge",
            over_a(),
            "drop_candidate",
        ))),
        &exact_nine,
        &[15],
    );
    assert_eq!(verdict(&exact, "slot-0").status, Ranked);
}

/// Register §1.312 S168-LB1b test 6 (LB1-9): two sides read at least, [9, +∞) and [12, +∞),
/// settle no operator; an exact side against an at-least one is read by the range rule.
#[test]
fn lower_bound_two_ranges_settle_nothing() {
    let facts = lists(
        &["member.a", "member.b", "member.z"],
        &["member.d", "member.y"],
    );
    for op in ["eq", "ne", "lt", "le", "gt", "ge"] {
        let decision = decide(
            gate_only(compare_gate(over_a(), op, over_b(), "drop_candidate")),
            &facts,
            &[0],
        );
        let candidate = verdict(&decision, "slot-0");
        assert_eq!(candidate.status, CandidateStatus::UnknownDropped, "{op}");
        assert_eq!(
            gate_reason(candidate),
            Some(UnknownReason::LookupMiss),
            "{op}"
        );
    }

    // [9, +∞) is above an exact 5.
    let exact_b = lists(&["member.a", "member.b", "member.z"], &["member.a"]);
    let decision = decide(
        gate_only(compare_gate(over_a(), "gt", over_b(), "drop_candidate")),
        &exact_b,
        &[0],
    );
    assert_eq!(verdict(&decision, "slot-0").status, CandidateStatus::Ranked);
}

/// Register §1.312 S168-LB1b test 7 (LB1-9): a threshold at L or below gives `then_milli`, one
/// above L is Unknown `lookup_miss` (dropped under `drop_candidate`, the evaluation unknown under
/// `abort_evaluation`), an exact value below it still gives `otherwise_milli`; `identity` takes L
/// (12, from a bound), and under a negative weight contributes -12.
#[test]
fn lower_bound_terms() {
    let at_least_nine = lists(&["member.a", "member.b", "member.z"], &[]);
    let exact_nine = lists(&["member.a", "member.b"], &[]);
    let scored =
        |transform: Value, weight_milli: i64, on_unknown: &str, facts: &SelectionFactSnapshot| {
            decide(
                document(
                    json!([]),
                    json!([term(
                        "term-best",
                        over_a(),
                        transform,
                        weight_milli,
                        on_unknown
                    )]),
                ),
                facts,
                &[0],
            )
        };

    let reached = scored(threshold(9), 1000, "drop_candidate", &at_least_nine);
    let candidate = verdict(&reached, "slot-0");
    assert_eq!(
        candidate.terms[0].outcome,
        TermOutcome::Scored {
            transformed_milli: 300
        }
    );
    assert_eq!(candidate.score_milli, Some(300));

    let unreached = scored(threshold(10), 1000, "drop_candidate", &at_least_nine);
    let candidate = verdict(&unreached, "slot-0");
    assert_eq!(candidate.status, CandidateStatus::UnknownDropped);
    assert_eq!(term_reason(candidate), Some(UnknownReason::LookupMiss));
    assert!(!candidate.static_table_drop);

    let aborted = scored(threshold(10), 1000, "abort_evaluation", &at_least_nine);
    assert!(
        matches!(
            aborted.outcome,
            SelectionOutcome::Unknown {
                reason: UnknownReason::LookupMiss,
                ..
            }
        ),
        "{:?}",
        aborted.outcome
    );

    let exact = scored(threshold(10), 1000, "drop_candidate", &exact_nine);
    assert_eq!(
        verdict(&exact, "slot-0").terms[0].outcome,
        TermOutcome::Scored {
            transformed_milli: 700
        }
    );

    let at_least_twelve = lists(&["member.a", "member.d"], &[]);
    let positive = scored(identity(), 1000, "drop_candidate", &at_least_twelve);
    let candidate = verdict(&positive, "slot-0");
    assert_eq!(
        candidate.terms[0].outcome,
        TermOutcome::Scored {
            transformed_milli: 12
        }
    );
    assert_eq!(candidate.score_milli, Some(12));
    let negative = scored(identity(), -1000, "drop_candidate", &at_least_twelve);
    let candidate = verdict(&negative, "slot-0");
    assert_eq!(candidate.terms[0].contribution_milli, -12);
    assert_eq!(candidate.score_milli, Some(-12));
}

/// Register §1.312 S168-LB1b test 8 (LB1-9, the upper bound): over [9, +∞) `identity` has no
/// finite bound with a positive weight and its bound at L with a negative one; `threshold` gives
/// `then_milli` once L reaches `at_least` and the larger of its two values otherwise; a gate
/// compares the range as a ranged input (a price of 50 to 100 may lie below [9, +∞), never below
/// an exact 9); an exact read stays exact and an Unknown one is as before.
#[test]
fn lower_bound_upper_bound() {
    let at_least_nine = lists(&["member.a", "member.b", "member.z"], &[]);
    let exact_nine = lists(&["member.a", "member.b"], &[]);
    let nothing = lists(&["member.f", "member.z"], &[]);
    let upper = |document: Value, facts: &SelectionFactSnapshot, min: i64, max: i64| {
        let policy = policy_of(document);
        let tables = bound_tables(&policy);
        let domains = BTreeMap::from([
            ("member".to_owned(), FieldDomain::Identity),
            ("price".to_owned(), FieldDomain::Integer { min, max }),
        ]);
        upper_bound_with_tables(
            &policy,
            &tables,
            facts,
            NOW,
            &["member.a".to_owned()],
            &domains,
        )
        .expect("a bound")
    };
    let best = |transform: Value, weight_milli: i64, on_unknown: &str| {
        document(
            json!([]),
            json!([term(
                "term-best",
                over_a(),
                transform,
                weight_milli,
                on_unknown
            )]),
        )
    };

    assert_eq!(
        upper(
            best(identity(), 1000, "drop_candidate"),
            &at_least_nine,
            0,
            100
        ),
        UpperBound::Unbounded
    );
    assert_eq!(
        upper(
            best(identity(), -1000, "drop_candidate"),
            &at_least_nine,
            0,
            100
        ),
        UpperBound::Max(-9)
    );
    assert_eq!(
        upper(
            best(threshold(9), 1000, "drop_candidate"),
            &at_least_nine,
            0,
            100
        ),
        UpperBound::Max(300)
    );
    assert_eq!(
        upper(
            best(threshold(10), 1000, "drop_candidate"),
            &at_least_nine,
            0,
            100
        ),
        UpperBound::Max(700)
    );

    let below = document(
        json!([compare_gate(price(), "lt", over_a(), "drop_candidate")]),
        json!([term(
            "term-price",
            price(),
            identity(),
            1000,
            "drop_candidate"
        )]),
    );
    assert_eq!(
        upper(below.clone(), &at_least_nine, 50, 100),
        UpperBound::Max(100)
    );
    assert_eq!(upper(below, &exact_nine, 50, 100), UpperBound::NoneEligible);

    assert_eq!(
        upper(
            best(identity(), 1000, "drop_candidate"),
            &exact_nine,
            0,
            100
        ),
        UpperBound::Max(9)
    );
    assert_eq!(
        upper(best(identity(), 1000, "drop_candidate"), &nothing, 0, 100),
        UpperBound::NoneEligible
    );
    assert_eq!(
        upper(best(identity(), 1000, "abort_evaluation"), &nothing, 0, 100),
        UpperBound::Unbounded
    );
}

/// Register §1.312 S168-LB1b test 9 (LB1-10, LB1-11): only a reference that opts in carries
/// `bound` and `unknown_count`, with `lower_bound` and `lower_bound_identity` in the place of
/// `maximum` and `maximum_identity` when it reads at least; an Unknown read carries
/// `unknown_reason` as before. A decision made on a lower bound carries
/// `selection_decided_on_lower_bound` once, after the list maximum's reason and right before the
/// outcome's reasons, with the first such gate or term and their number; a decision the bound
/// settled nothing of, an exact one and an `unknown` evaluation carry none.
#[test]
fn lower_bound_reasons() {
    let at_least = &["member.a", "member.b", "member.z"][..];
    let known = &["member.a", "member.b", "member.c"][..];
    let cases = [
        (
            "at least",
            over_a(),
            at_least,
            &[
                "bound",
                "unknown_count",
                "lower_bound",
                "lower_bound_identity",
            ][..],
        ),
        (
            "exact",
            over_a(),
            known,
            &["bound", "unknown_count", "maximum", "maximum_identity"][..],
        ),
        (
            "empty",
            over_a(),
            &["member.c"][..],
            &["bound", "unknown_count"][..],
        ),
        (
            "nothing known",
            over_a(),
            &["member.f", "member.z"][..],
            &["unknown_reason"][..],
        ),
        (
            "not opted in, unknown",
            plain_a(),
            at_least,
            &["unknown_reason"][..],
        ),
        (
            "not opted in, known",
            plain_a(),
            known,
            &["maximum", "maximum_identity"][..],
        ),
    ];
    for (label, reference, keys, extra) in cases {
        let decision = decide(scoring_only(reference), &lists(keys, &[]), &[0]);
        let reasons = list_max_reasons(&decision);
        assert_eq!(reasons.len(), 1, "{label}");
        assert_eq!(keys_of(reasons[0]), read_keys(extra), "{label}");
    }

    let decision = decide(scoring_only(over_a()), &lists(at_least, &[]), &[0]);
    let reason = list_max_reasons(&decision)[0];
    assert_eq!(value(reason, "bound"), Some(&json!("at_least")));
    assert_eq!(value(reason, "unknown_count"), Some(&json!(1)));
    assert_eq!(value(reason, "lower_bound"), Some(&json!(9)));
    assert_eq!(
        value(reason, "lower_bound_identity"),
        Some(&json!("member.b"))
    );
    assert_eq!(value(reason, "count"), Some(&json!(3)));
    let decision = decide(scoring_only(over_a()), &lists(known, &[]), &[0]);
    let reason = list_max_reasons(&decision)[0];
    assert_eq!(value(reason, "bound"), Some(&json!("exact")));
    assert_eq!(value(reason, "unknown_count"), Some(&json!(0)));
    assert_eq!(value(reason, "maximum"), Some(&json!(9)));
    assert_eq!(value(reason, "maximum_identity"), Some(&json!("member.b")));

    // The info reason: once, after the list maximum's reason and before the outcome's.
    let chain = |outcome: &'static str| {
        [
            "selection_policy_identity",
            "selection_inputs_identity",
            "selection_requirement_declared",
            "selection_table_list_max",
            "selection_decided_on_lower_bound",
            outcome,
            "selection_outcome_key_chosen",
        ]
    };
    let decided = |decision: &SelectionDecision| -> (Value, Value) {
        let reason = decision
            .reasons
            .iter()
            .find(|reason| code(reason) == "selection_decided_on_lower_bound")
            .expect("the decided reason");
        (
            value(reason, "rule_id").cloned().unwrap_or(Value::Null),
            value(reason, "count").cloned().unwrap_or(Value::Null),
        )
    };
    let lists_at_least = lists(at_least, &[]);

    // The gate settles the first candidate and the term scores it at L; the second is dropped.
    let both = decide(
        gate_and_term(over_a(), "drop_candidate"),
        &lists_at_least,
        &[9, 10],
    );
    assert_eq!(codes(&both), chain("selection_selected"));
    assert_eq!(decided(&both), (json!("gate-compare"), json!(2)));
    assert_eq!(
        verdict(&both, "slot-1").status,
        CandidateStatus::UnknownDropped
    );

    let scored = decide(scoring_only(over_a()), &lists_at_least, &[0]);
    assert_eq!(codes(&scored), chain("selection_selected"));
    assert_eq!(decided(&scored), (json!("term-best"), json!(1)));

    // A rejection the bound settled counts too.
    let rejected = decide(
        gate_only(compare_gate(over_a(), "le", price(), "drop_candidate")),
        &lists_at_least,
        &[8],
    );
    assert_eq!(codes(&rejected), chain("selection_empty"));
    assert_eq!(decided(&rejected), (json!("gate-compare"), json!(1)));

    // None where the bound settled nothing, where the read is exact, and on an evaluation that
    // ended unknown although the bound settled its first candidate.
    let unsettled = decide(
        gate_only(compare_gate(over_a(), "ge", price(), "drop_candidate")),
        &lists_at_least,
        &[10],
    );
    let exact = decide(
        gate_and_term(over_a(), "drop_candidate"),
        &lists(known, &[]),
        &[9, 10],
    );
    let unknown = decide(
        gate_and_term(over_a(), "abort_evaluation"),
        &lists_at_least,
        &[9, 10],
    );
    assert!(
        matches!(unknown.outcome, SelectionOutcome::Unknown { .. }),
        "{:?}",
        unknown.outcome
    );
    for (label, decision) in [
        ("unsettled", &unsettled),
        ("exact", &exact),
        ("unknown", &unknown),
    ] {
        assert!(
            !codes(decision).contains(&"selection_decided_on_lower_bound"),
            "{label}: {:?}",
            codes(decision)
        );
    }
}

/// Register §1.312 S168-LB1b test 10 (the revision, `package` 9): a document that writes the
/// member carries it in its bytes, and this build reads it. A reference denies the members it
/// does not know, at decode, before any validation: that is how a `package` 8 reader, whose
/// reference has no `lower_bound_column`, refuses such a document at admission
/// (`contained_task_select_policy_invalid`), and a member this build does not know is refused
/// the same way, in the document and in the reference alone.
#[test]
fn lower_bound_member_needs_package_9() {
    let written = gate_and_term(over_a(), "drop_candidate");
    let policy = policy_of(written.clone());
    let bytes = String::from_utf8(canonical_bytes(&policy).expect("bytes")).expect("UTF-8");
    assert!(
        bytes.contains(r#""lower_bound_column":"value_at_least""#),
        "{bytes}"
    );

    let mut unknown_member = written;
    let reference = &mut unknown_member["scoring"][0]["value"];
    let member = reference
        .as_object_mut()
        .expect("the reference")
        .remove("lower_bound_column")
        .expect("the member");
    reference["upper_bound_column"] = member;
    let error = serde_json::from_value::<SelectionPolicy>(unknown_member)
        .expect_err("a document with a member the reference does not know");
    assert!(error.to_string().contains("unknown field"), "{error}");

    let mut alone = over_a();
    alone["lower_bound_columns"] = json!(BOUND);
    let error = serde_json::from_value::<ValueRef>(alone)
        .expect_err("a reference with a member it does not know");
    assert!(error.to_string().contains("unknown field"), "{error}");
}
