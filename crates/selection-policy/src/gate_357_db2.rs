// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-2 (#430 S4a-1; model M-357a v1.1 §3, §8; register §1.244): a selection
//! document that reads a shared table, on synthetic tables only. The document's rules, K2, the
//! read rule of §3.4 end to end through `evaluate_with_tables`, the input identity and the upper
//! bound. No file, clock or device: every table is parsed from bytes here and fixed at an instant.

use std::collections::BTreeMap;
use std::sync::Arc;

use actingcommand_contract::{TableMismatch, parse_standard_pack_table};
use serde_json::{Value, json};

use crate::{
    Candidate, CandidateStatus, CandidateVerdict, FieldDomain, GateOutcome, LoadedTable,
    ScalarValue, SelectionDecision, SelectionErrorCode, SelectionFactEntry, SelectionFactSnapshot,
    SelectionOutcome, SelectionPolicy, SelectionTables, TermOutcome, UnknownReason, UpperBound,
    canonical_bytes, canonical_sha256, evaluate_collect, evaluate_with_tables,
    upper_bound_with_tables,
};

const NOW: u64 = 1_000_000;
const GROUP_FACT: &str = "session.example.group";
/// The second the `secondary_milli` column is good until.
const SECONDARY_UNTIL_S: i64 = 2_000;
const MEMBERS: [&str; 7] = [
    "unit.a", "unit.b", "unit.c", "unit.d", "unit.e", "unit.f", "unit.g",
];

/// A table of three row-key columns: `unit` has `alpha` rows and `beta` rows; `unit.g` is not in
/// it at all.
fn table_value() -> Value {
    json!({
        "schema_version": "actingcommand.table.v1",
        "game": "neutral",
        "table_id": "unit_scores",
        "row_key": ["unit", "group", "variant"],
        "columns": [
            {"name": "unit", "type": "key"},
            {"name": "group", "type": "enum_string", "allowed": ["alpha", "beta"]},
            {"name": "variant", "type": "enum_string", "allowed": ["v1", "v2"]},
            {"name": "listed", "type": "boolean"},
            {"name": "primary_milli", "type": "integer", "min": 0, "max": 100000, "nullable": true},
            {"name": "secondary_milli", "type": "integer", "min": 0, "max": 100000, "nullable": true,
             "valid_until_unix_s": SECONDARY_UNTIL_S},
            {"name": "kind", "type": "enum_string", "allowed": ["x", "y", "z"], "nullable": true}
        ],
        "rows": [
            ["unit.a", "alpha", "v1", true, 900, 100, "x"],
            ["unit.a", "beta", "v1", true, 300, 100, "x"],
            ["unit.b", "alpha", "v1", true, 500, 200, "y"],
            ["unit.c", "beta", "v1", true, 700, 300, "x"],
            ["unit.d", "alpha", "v1", true, null, 400, "x"],
            ["unit.e", "alpha", "v1", true, null, null, "x"],
            ["unit.f", "alpha", "v1", true, 800, 600, "z"]
        ]
    })
}

fn load(value: &Value, now_unix_ms: u64) -> Arc<LoadedTable> {
    let bytes = serde_json::to_vec(value).expect("table bytes");
    let table = parse_standard_pack_table(&bytes, Some("neutral"), "unit_scores.json")
        .expect("a valid table");
    Arc::new(LoadedTable::load(table, now_unix_ms))
}

/// The two pins of the document: the group from the fact, the variant a literal.
fn pins() -> Value {
    json!([
        {"column": "group", "equals_fact": GROUP_FACT},
        {"column": "variant", "equals": {"type": "string", "value": "v1"}}
    ])
}

fn read(value_column: &str) -> Value {
    json!({"source": "table", "table_id": "unit_scores", "key_field": "member", "pins": pins(),
           "value_column": value_column})
}

/// The class gate: the row's constant `listed`, `absent` false.
fn class_gate() -> Value {
    let mut value = read("listed");
    value["absent"] = json!({"type": "boolean", "value": false});
    json!({"gate_id": "gate-listed",
           "predicate": {"kind": "boolean_equals", "value": value, "expected": true},
           "on_unknown": {"kind": "drop_candidate"}})
}

fn term(term_id: &str, value: Value) -> Value {
    json!({"term_id": term_id, "value": value, "transform": {"kind": "identity"},
           "weight_milli": 1000, "on_unknown": {"kind": "drop_candidate"}})
}

/// One class gate and one term over `primary_milli`, both under `drop_candidate`.
fn document() -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "by-table",
        "applies_to": {"candidate_layout_id": "layout-a", "outcome_keys": {
            "selected": "s", "empty": "e", "insufficient": "i", "ambiguous": "a", "unknown": "u"
        }},
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": MEMBERS}}],
        "facts": [{"fact_key": GROUP_FACT,
                   "value_type": {"type": "enum_string", "allowed": ["alpha", "beta"]},
                   "max_age_ms": 3_600_000, "minimum_confidence_milli": 1000}],
        "tables": [{"table_id": "unit_scores", "path": "tables/unit_scores.json",
                    "row_key": ["unit", "group", "variant"],
                    "columns": {
                        "group": {"type": "enum_string", "allowed": ["alpha", "beta"]},
                        "variant": {"type": "enum_string", "allowed": ["v1", "v2"]},
                        "listed": {"type": "boolean"},
                        "primary_milli": {"type": "integer"},
                        "secondary_milli": {"type": "integer"},
                        "kind": {"type": "enum_string", "allowed": ["x", "y"]}
                    }}],
        "gates": [class_gate()],
        "scoring": [term("term-primary", read("primary_milli"))],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

fn policy_of(document: Value) -> SelectionPolicy {
    let policy: SelectionPolicy = serde_json::from_value(document).expect("the document decodes");
    policy.validate().expect("a valid document");
    policy
}

fn candidates(members: &[&str]) -> Vec<Candidate> {
    members
        .iter()
        .map(|member| Candidate {
            candidate_id: format!("c-{member}"),
            fields: BTreeMap::from([(
                "member".to_owned(),
                ScalarValue::String((*member).to_owned()),
            )]),
        })
        .collect()
}

fn facts(group: Option<&str>) -> SelectionFactSnapshot {
    let mut facts = BTreeMap::new();
    if let Some(group) = group {
        facts.insert(
            GROUP_FACT.to_owned(),
            SelectionFactEntry::Published {
                value: ScalarValue::String(group.to_owned()),
                published_at_unix_ms: NOW,
                expires_at_unix_ms: None,
                confidence_milli: 1000,
            },
        );
    }
    SelectionFactSnapshot {
        snapshot_id: "snapshot-a".to_owned(),
        snapshot_at_unix_ms: NOW,
        facts,
    }
}

fn bound(policy: &SelectionPolicy, table: &Arc<LoadedTable>) -> SelectionTables {
    SelectionTables::bind(policy, std::slice::from_ref(table)).expect("the table binds")
}

/// The decision of `document` over `members` with the group fact at `group`, table loaded at NOW.
fn decide(document: Value, members: &[&str], group: Option<&str>) -> SelectionDecision {
    let policy = policy_of(document);
    let tables = bound(&policy, &load(&table_value(), NOW));
    evaluate_with_tables(&policy, &tables, &candidates(members), &facts(group), NOW)
        .expect("a decision")
}

fn verdict<'a>(decision: &'a SelectionDecision, member: &str) -> &'a CandidateVerdict {
    decision
        .candidates
        .iter()
        .find(|verdict| verdict.candidate_id == format!("c-{member}"))
        .expect("the candidate's verdict")
}

fn term_reason(verdict: &CandidateVerdict) -> Option<UnknownReason> {
    verdict.terms.iter().find_map(|term| match &term.outcome {
        TermOutcome::UnknownDropped { reason } => Some(*reason),
        _ => None,
    })
}

fn gate_reason(verdict: &CandidateVerdict) -> Option<UnknownReason> {
    verdict.gates.iter().find_map(|gate| match &gate.outcome {
        GateOutcome::UnknownDropped { reason } => Some(*reason),
        _ => None,
    })
}

fn fallback_reasons(verdict: &CandidateVerdict) -> Vec<&crate::DecisionReason> {
    verdict
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == "term_fallback_used")
        .collect()
}

/// #430 §8.3 case 1: a full match selects, and the score is the row's value.
#[test]
fn db2_a_full_match_selects_by_the_table_value() {
    let decision = decide(document(), &["unit.a", "unit.b"], Some("alpha"));
    assert_eq!(decision.selected, ["c-unit.a"]);
    assert_eq!(verdict(&decision, "unit.a").score_milli, Some(900));
    assert_eq!(verdict(&decision, "unit.b").score_milli, Some(500));
    assert!(matches!(
        decision.outcome,
        SelectionOutcome::Selected { .. }
    ));
}

/// #430 §8.3 case 2: a key with rows, none matching the pins, reads the gate's `absent`: the gate
/// decides against it, a known value and no unknown.
#[test]
fn db2_a_key_with_rows_but_none_matching_the_pins_reads_absent() {
    let decision = decide(document(), &["unit.c"], Some("alpha"));
    let verdict = verdict(&decision, "unit.c");
    assert_eq!(verdict.status, CandidateStatus::GateRejected);
    assert_eq!(gate_reason(verdict), None);
    assert!(!verdict.static_table_drop);
    assert!(matches!(decision.outcome, SelectionOutcome::Empty));
}

/// #430 §8.3 case 3: a pinned fact unknown is the fact's reason, instance data and no static
/// drop, under `drop_candidate`; under `abort_evaluation` it ends the evaluation.
#[test]
fn db2_a_required_fact_unknown_drops_or_aborts_as_the_document_says() {
    let dropped = decide(document(), &["unit.a"], None);
    let verdict_dropped = verdict(&dropped, "unit.a");
    assert_eq!(verdict_dropped.status, CandidateStatus::UnknownDropped);
    assert_eq!(
        gate_reason(verdict_dropped),
        Some(UnknownReason::FactMissing)
    );
    assert!(!verdict_dropped.static_table_drop);

    let mut aborting = document();
    aborting["gates"][0]["on_unknown"] = json!({"kind": "abort_evaluation"});
    let aborted = decide(aborting, &["unit.a"], None);
    assert!(matches!(
        aborted.outcome,
        SelectionOutcome::Unknown {
            reason: UnknownReason::FactMissing,
            ..
        }
    ));
}

/// #430 §8.3 case 4: a key no row holds and a null cell are `lookup_miss`; an enum cell outside
/// the declared `allowed` is `type_mismatch`, never the lookup's default; all three are the
/// table's own static data, so the other candidates still rank.
#[test]
fn db2_a_missing_key_a_null_cell_and_an_undeclared_enum_are_static_drops() {
    let mut no_key_gate = document();
    no_key_gate["gates"] = json!([]);
    no_key_gate["tables"][0]["row_key"] = json!(["unit"]);
    // The key-only reading of the same rows: a table with one row key column.
    let mut single = table_value();
    single["row_key"] = json!(["unit"]);
    single["rows"] = json!([
        ["unit.a", "alpha", "v1", true, 900, 100, "x"],
        ["unit.e", "alpha", "v1", true, null, null, "x"]
    ]);
    no_key_gate["scoring"][0]["value"]["pins"] = json!([]);
    let policy = policy_of(no_key_gate);
    let tables = bound(&policy, &load(&single, NOW));
    let decision = evaluate_with_tables(
        &policy,
        &tables,
        &candidates(&["unit.a", "unit.e", "unit.g"]),
        &facts(Some("alpha")),
        NOW,
    )
    .expect("a decision");
    assert_eq!(decision.selected, ["c-unit.a"]);
    for member in ["unit.e", "unit.g"] {
        let dropped = verdict(&decision, member);
        assert_eq!(dropped.status, CandidateStatus::UnknownDropped, "{member}");
        assert_eq!(
            term_reason(dropped),
            Some(UnknownReason::LookupMiss),
            "{member}"
        );
        assert!(dropped.static_table_drop, "{member}");
    }

    let mut by_kind = document();
    by_kind["scoring"] = json!([{
        "term_id": "term-kind", "value": read("kind"),
        "transform": {"kind": "lookup",
                      "entries": [{"key": {"type": "string", "value": "x"}, "value_milli": 1}],
                      "default_milli": 5},
        "weight_milli": 1000, "on_unknown": {"kind": "drop_candidate"}
    }]);
    let decision = decide(by_kind, &["unit.a", "unit.f"], Some("alpha"));
    let undeclared = verdict(&decision, "unit.f");
    assert_eq!(term_reason(undeclared), Some(UnknownReason::TypeMismatch));
    assert!(undeclared.static_table_drop);
    assert_eq!(decision.selected, ["c-unit.a"]);
}

/// #430 §8.3 case 5: a column past its `valid_until_unix_s` reads null from the second after it.
#[test]
fn db2_an_expired_column_reads_as_null() {
    let mut by_secondary = document();
    by_secondary["scoring"] = json!([term("term-secondary", read("secondary_milli"))]);
    let policy = policy_of(by_secondary);
    let at = |now_unix_ms: u64| {
        let tables = bound(&policy, &load(&table_value(), now_unix_ms));
        evaluate_with_tables(
            &policy,
            &tables,
            &candidates(&["unit.a"]),
            &facts(Some("alpha")),
            NOW,
        )
        .expect("a decision")
    };
    let until_ms = u64::try_from(SECONDARY_UNTIL_S).expect("an instant") * 1000;
    assert_eq!(verdict(&at(until_ms), "unit.a").score_milli, Some(100));
    let expired = at(until_ms + 1000);
    let dropped = verdict(&expired, "unit.a");
    assert_eq!(term_reason(dropped), Some(UnknownReason::LookupMiss));
    assert!(dropped.static_table_drop);
}

/// #430 §8.3 case 6: a null cell reads the `fallback_column` and says so, at most once per
/// candidate and rule; a fallback that is null as well is `lookup_miss`; a fallback not needed is
/// not reported.
#[test]
fn db2_a_fallback_column_is_used_and_reported_or_does_not_apply() {
    let mut with_fallback = document();
    with_fallback["scoring"][0]["value"]["fallback_column"] = json!("secondary_milli");
    let decision = decide(
        with_fallback,
        &["unit.a", "unit.d", "unit.e"],
        Some("alpha"),
    );
    assert!(fallback_reasons(verdict(&decision, "unit.a")).is_empty());
    let fell_back = verdict(&decision, "unit.d");
    assert_eq!(fell_back.score_milli, Some(400));
    let reasons = fallback_reasons(fell_back);
    assert_eq!(reasons.len(), 1);
    let rule = reasons[0].outcome.value("rule_id").and_then(Value::as_str);
    assert_eq!(rule, Some("term-primary"));
    assert!(reasons[0].outcome.value("table_column").is_none());
    let neither = verdict(&decision, "unit.e");
    assert_eq!(term_reason(neither), Some(UnknownReason::LookupMiss));
    assert!(neither.static_table_drop);
    assert!(fallback_reasons(neither).is_empty());
}

/// A pin's `otherwise` is used when its fact is unavailable and names its column in the reason.
#[test]
fn db2_a_pins_otherwise_is_used_and_names_its_column() {
    let mut with_otherwise = document();
    let otherwise = json!({"equals": {"type": "string", "value": "alpha"}});
    with_otherwise["gates"][0]["predicate"]["value"]["pins"][0]["otherwise"] = otherwise.clone();
    with_otherwise["scoring"][0]["value"]["pins"][0]["otherwise"] = otherwise;
    let decision = decide(with_otherwise, &["unit.a"], None);
    let chosen = verdict(&decision, "unit.a");
    assert_eq!(chosen.score_milli, Some(900));
    let reasons = fallback_reasons(chosen);
    assert_eq!(reasons.len(), 2, "the gate's and the term's: {reasons:?}");
    for reason in reasons {
        let column = reason.outcome.value("table_column").and_then(Value::as_str);
        assert_eq!(column, Some("group"));
    }
}

/// #430 §8.3 case 7: a pin on a fact that holds another value. One the document declares and no
/// row of this key holds reads `absent`; one outside the fact's declaration is `type_mismatch`,
/// instance data, never a static drop.
#[test]
fn db2_a_pin_on_a_fact_holding_another_value() {
    let other_class = decide(document(), &["unit.b"], Some("beta"));
    assert_eq!(
        verdict(&other_class, "unit.b").status,
        CandidateStatus::GateRejected
    );
    let undeclared = decide(document(), &["unit.a"], Some("gamma"));
    let dropped = verdict(&undeclared, "unit.a");
    assert_eq!(gate_reason(dropped), Some(UnknownReason::TypeMismatch));
    assert!(!dropped.static_table_drop);
}

/// #430 §8.3 case 8, a golden: a document without `tables` keeps its canonical bytes, its
/// `policy_sha256` and its `input_sha256`, the values computed apart from this code.
#[test]
fn db2_a_document_without_tables_keeps_its_bytes_and_identities() {
    let document = json!({
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
    let policy = policy_of(document);
    assert_eq!(
        String::from_utf8(canonical_bytes(&policy).expect("bytes")).expect("UTF-8"),
        r#"{"applies_to":{"candidate_layout_id":"layout-a","outcome_keys":{"ambiguous":"a","empty":"e","insufficient":"i","selected":"s","unknown":"u"}},"facts":[],"fields":[{"name":"member","value_type":{"allowed":["unit.a","unit.b"],"type":"enum_string"}}],"gates":[],"policy_id":"by-table","schema_version":"actingcommand.selection-policy.v1","scoring":[{"on_unknown":{"kind":"drop_candidate"},"term_id":"t","transform":{"default_milli":1,"entries":[{"key":{"type":"string","value":"unit.a"},"value_milli":5}],"kind":"lookup"},"value":{"field":"member","source":"field"},"weight_milli":1000}],"selection":{"mode":"exactly_one","required_count":1},"tie_break":[{"direction":"lowest_first","kind":"candidate_id"}]}"#
    );
    assert_eq!(
        canonical_sha256(&policy).expect("identity"),
        "sha256:a83ca293574f1362a8a0efd80abfcc3312c25f89dc546422c01e8e8b91f1f791"
    );
    let decision = crate::evaluate(&policy, &golden_candidates(), &golden_facts(), GOLDEN_NOW)
        .expect("a decision");
    assert_eq!(
        decision.input_sha256,
        "sha256:a4d6f37fa6dd921ac8e22cb51dc2e77abb76e0e7ed075af3fe29b2f8339e79fb"
    );
}

const GOLDEN_NOW: u64 = 1000;

fn golden_candidates() -> Vec<Candidate> {
    ["c1", "c2"]
        .into_iter()
        .zip(["unit.a", "unit.b"])
        .map(|(candidate_id, member)| Candidate {
            candidate_id: candidate_id.to_owned(),
            fields: BTreeMap::from([("member".to_owned(), ScalarValue::String(member.to_owned()))]),
        })
        .collect()
}

fn golden_facts() -> SelectionFactSnapshot {
    SelectionFactSnapshot {
        snapshot_id: "snapshot-a".to_owned(),
        snapshot_at_unix_ms: GOLDEN_NOW,
        facts: BTreeMap::new(),
    }
}

/// §3.7, a golden: the input identity of a document with a table covers the rows it reads, each
/// an object of its key and its declared columns, in row-key order; a row of another key is not
/// in it, so changing that row keeps the hash and changing a row read moves it.
#[test]
fn db2_the_input_identity_covers_the_rows_read() {
    let document = json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "by-table",
        "applies_to": {"candidate_layout_id": "layout-a", "outcome_keys": {
            "selected": "s", "empty": "e", "insufficient": "i", "ambiguous": "a", "unknown": "u"
        }},
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": ["unit.a", "unit.b"]}}],
        "facts": [],
        "tables": [{"table_id": "unit_scores", "path": "tables/unit_scores.json",
                    "row_key": ["unit"], "columns": {"score_milli": {"type": "integer"}}}],
        "gates": [],
        "scoring": [{"term_id": "t",
                     "value": {"source": "table", "table_id": "unit_scores",
                               "key_field": "member", "pins": [],
                               "value_column": "score_milli"},
                     "transform": {"kind": "identity"},
                     "weight_milli": 1000, "on_unknown": {"kind": "drop_candidate"}}],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    });
    let policy = policy_of(document);
    let table = |rows: Value| {
        load(
            &json!({
                "schema_version": "actingcommand.table.v1",
                "game": "neutral",
                "table_id": "unit_scores",
                "row_key": ["unit"],
                "columns": [
                    {"name": "unit", "type": "key"},
                    {"name": "score_milli", "type": "integer", "min": 0, "max": 1000,
                     "nullable": true}
                ],
                "rows": rows
            }),
            GOLDEN_NOW,
        )
    };
    let input = |rows: Value| {
        let tables = bound(&policy, &table(rows));
        evaluate_with_tables(
            &policy,
            &tables,
            &golden_candidates(),
            &golden_facts(),
            GOLDEN_NOW,
        )
        .expect("a decision")
    };
    let read = input(json!([["unit.b", null], ["unit.a", 700], ["unit.z", 1]]));
    assert_eq!(
        canonical_sha256(&policy).expect("identity"),
        "sha256:036ab7bb0eb89ba1b8c4d8a4c6b887554cacab8b5fc0acc620188f59cbd9092c"
    );
    assert_eq!(
        read.input_sha256,
        "sha256:6b814ee03e3a1cc4406b918a11742884fab146682a7d852fd1c52cd4cd7d9c8a"
    );
    let other_row = input(json!([["unit.b", null], ["unit.a", 700], ["unit.z", 2]]));
    assert_eq!(other_row.input_sha256, read.input_sha256);
    let row_read = input(json!([["unit.b", null], ["unit.a", 701], ["unit.z", 1]]));
    assert_ne!(row_read.input_sha256, read.input_sha256);
}

/// K2: each difference of a table from its declaration is one word of `table_mismatch`.
#[test]
fn db2_bind_refuses_each_difference_from_the_declaration() {
    let policy = policy_of(document());
    let refused = |change: &dyn Fn(&mut Value)| {
        let mut value = table_value();
        change(&mut value);
        SelectionTables::bind(&policy, &[load(&value, NOW)]).expect_err("a refusal")
    };
    let row_key = refused(&|table| table["row_key"] = json!(["unit", "group"]));
    assert_eq!(row_key.mismatch, TableMismatch::RowKey);
    assert_eq!(row_key.path, "tables/unit_scores.json");

    let absent = refused(&|table| {
        table["columns"][5]["name"] = json!("other_milli");
    });
    assert_eq!(absent.mismatch, TableMismatch::ColumnAbsent);
    assert_eq!(absent.column.as_deref(), Some("secondary_milli"));

    let column_type = refused(&|table| {
        table["columns"][3] = json!({"name": "listed", "type": "integer", "min": 0, "max": 1});
        let rows = table["rows"].as_array_mut().expect("rows");
        for row in rows {
            row[3] = json!(1);
        }
    });
    assert_eq!(column_type.mismatch, TableMismatch::ColumnType);
    assert_eq!(column_type.column.as_deref(), Some("listed"));

    let not_subset = refused(&|table| {
        table["columns"][6]["allowed"] = json!(["x", "z"]);
        for row in table["rows"].as_array_mut().expect("rows") {
            if row[6] == json!("y") {
                row[6] = json!("x");
            }
        }
    });
    assert_eq!(not_subset.mismatch, TableMismatch::AllowedNotSubset);
    assert_eq!(not_subset.column.as_deref(), Some("kind"));

    // A table may allow more than the document declares: `z` above binds as it is.
    SelectionTables::bind(&policy, &[load(&table_value(), NOW)]).expect("the file's superset");
}

/// A declared table nothing bound is `dangling_reference` from the evaluator and the bound, and
/// the collect option, the scheduling consumer's, refuses the document (TB10).
#[test]
fn db2_an_unbound_table_is_refused_and_the_collect_option_refuses_tables() {
    let policy = policy_of(document());
    let group = facts(Some("alpha"));
    let members = candidates(&["unit.a"]);
    let error = crate::evaluate(&policy, &members, &group, NOW).expect_err("unbound");
    assert_eq!(error.code(), SelectionErrorCode::DanglingReference);
    let error =
        crate::upper_bound(&policy, &group, NOW, &[], &BTreeMap::new()).expect_err("unbound");
    assert_eq!(error.code(), SelectionErrorCode::DanglingReference);
    let error = evaluate_collect(&policy, &members, &group, NOW).expect_err("scheduling");
    assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);
    assert!(error.detail().contains("tables=declared"), "{error}");
}

fn identity_domain() -> BTreeMap<String, FieldDomain> {
    BTreeMap::from([("member".to_owned(), FieldDomain::Identity)])
}

fn bound_over(policy: &SelectionPolicy, members: &[&str], group: Option<&str>) -> UpperBound {
    let identities = members
        .iter()
        .map(|member| (*member).to_owned())
        .collect::<Vec<_>>();
    upper_bound_with_tables(
        policy,
        &bound(policy, &load(&table_value(), NOW)),
        &facts(group),
        NOW,
        &identities,
        &identity_domain(),
    )
    .expect("a bound")
}

/// §3.6: a table value is exact for the identity; a gate that reads a table excludes what it
/// cannot hold for under `drop_candidate` (a key no row holds, `absent` false) and has no finite
/// bound where it may be unknown under `abort_evaluation`; a term's static Unknown excludes the
/// identity under `drop_candidate`.
#[test]
fn db2_the_upper_bound_reads_the_table_exactly() {
    let dropping = policy_of(document());
    assert_eq!(
        bound_over(&dropping, &["unit.a", "unit.b"], Some("alpha")),
        UpperBound::Max(900)
    );
    // `unit.g` has no row: the class gate cannot hold. `unit.c` has rows, none of this class.
    assert_eq!(
        bound_over(&dropping, &["unit.g", "unit.c"], Some("alpha")),
        UpperBound::NoneEligible
    );
    // `unit.e`'s cell is null: the term excludes it.
    assert_eq!(
        bound_over(&dropping, &["unit.e", "unit.b"], Some("alpha")),
        UpperBound::Max(500)
    );

    let mut aborting = document();
    aborting["gates"][0]["on_unknown"] = json!({"kind": "abort_evaluation"});
    let aborting = policy_of(aborting);
    assert_eq!(
        bound_over(&aborting, &["unit.g"], Some("alpha")),
        UpperBound::Unbounded
    );
    // `absent` is a known value, so the abort has nothing to do there.
    assert_eq!(
        bound_over(&aborting, &["unit.c"], Some("alpha")),
        UpperBound::NoneEligible
    );
}

/// A tie-break may read the row a gate or term reads; a null cell orders behind a value.
#[test]
fn db2_a_tie_break_reads_a_row_the_terms_read() {
    let mut tied = document();
    tied["scoring"] = json!([term("term-listed", {
        let mut value = read("secondary_milli");
        value["fallback_column"] = json!("primary_milli");
        value
    })]);
    tied["scoring"][0]["transform"] = json!({
        "kind": "threshold", "at_least": 0, "then_milli": 1, "otherwise_milli": 1
    });
    tied["tie_break"] = json!([
        {"kind": "value", "value": read("primary_milli"), "direction": "highest_first"},
        {"kind": "candidate_id", "direction": "lowest_first"}
    ]);
    let decision = decide(tied, &["unit.b", "unit.d", "unit.a"], Some("alpha"));
    assert_eq!(decision.selected, ["c-unit.a"]);
    assert_eq!(verdict(&decision, "unit.d").rank, Some(3));
}

/// One case of [`db2_every_document_rule_refuses`], which gives its closure its signature.
fn case<'a>(
    label: &'a str,
    change: impl Fn(&mut Value) + 'a,
    code: SelectionErrorCode,
) -> (&'a str, Box<dyn Fn(&mut Value) + 'a>, SelectionErrorCode) {
    (label, Box::new(change), code)
}

/// §3.1 to §3.4: every rule of the document, one refusal each, without any table.
#[test]
fn db2_every_document_rule_refuses() {
    use SelectionErrorCode as Code;
    let set_pins = |document: &mut Value, pins: Value| {
        document["gates"][0]["predicate"]["value"]["pins"] = pins.clone();
        document["scoring"][0]["value"]["pins"] = pins;
    };
    let literal = |value: Value| {
        json!([
            {"column": "group", "equals": {"type": "string", "value": "alpha"}},
            {"column": "variant", "equals": value}
        ])
    };
    let fact_pin = |fact_key: &str| {
        json!([
            {"column": "group", "equals_fact": fact_key},
            {"column": "variant", "equals": {"type": "string", "value": "v1"}}
        ])
    };
    let cases = vec![
        case(
            "an undeclared table",
            |document| {
                document["scoring"][0]["value"]["table_id"] = json!("other");
            },
            Code::DanglingReference,
        ),
        case(
            "an undeclared column",
            |document| {
                document["scoring"][0]["value"]["value_column"] = json!("other");
            },
            Code::DanglingReference,
        ),
        case(
            "a path that is not the table's",
            |document| document["tables"][0]["path"] = json!("tables/other.json"),
            Code::InvalidArguments,
        ),
        case(
            "a table id outside the grammar",
            |document| {
                document["tables"][0]["table_id"] = json!("Unit");
                document["tables"][0]["path"] = json!("tables/Unit.json");
            },
            Code::InvalidArguments,
        ),
        case(
            "a table declared twice",
            |document| {
                let table = document["tables"][0].clone();
                document["tables"]
                    .as_array_mut()
                    .expect("tables")
                    .push(table);
            },
            Code::DuplicateId,
        ),
        case(
            "more than eight tables",
            |document| {
                let table = document["tables"][0].clone();
                let tables = document["tables"].as_array_mut().expect("tables");
                for index in 0..8 {
                    let mut other = table.clone();
                    other["table_id"] = json!(format!("other_{index}"));
                    other["path"] = json!(format!("tables/other_{index}.json"));
                    tables.push(other);
                }
            },
            Code::LimitExceeded,
        ),
        case(
            "the key column among the columns",
            |document| {
                document["tables"][0]["columns"]["unit"] = json!({"type": "integer"});
            },
            Code::DuplicateId,
        ),
        case(
            "a record list column",
            |document| {
                document["tables"][0]["columns"]["listed"] =
                    json!({"type": "record_list", "key_column": "id", "columns": {}});
            },
            Code::TypeMismatch,
        ),
        case(
            "a row key column undeclared",
            |document| {
                document["tables"][0]["columns"]
                    .as_object_mut()
                    .expect("columns")
                    .remove("variant");
            },
            Code::DanglingReference,
        ),
        case(
            "a row key not pinned whole",
            |document| {
                set_pins(
                    document,
                    json!([{"column": "group", "equals_fact": GROUP_FACT}]),
                );
            },
            Code::TypeMismatch,
        ),
        case(
            "a pin outside the row key",
            |document| {
                set_pins(
                    document,
                    json!([
                        {"column": "group", "equals_fact": GROUP_FACT},
                        {"column": "listed", "equals": {"type": "boolean", "value": true}}
                    ]),
                );
            },
            Code::TypeMismatch,
        ),
        case(
            "a column pinned twice",
            |document| {
                set_pins(
                    document,
                    json!([
                        {"column": "group", "equals_fact": GROUP_FACT},
                        {"column": "group", "equals_fact": GROUP_FACT}
                    ]),
                );
            },
            Code::DuplicateId,
        ),
        case(
            "a literal of another type",
            |document| set_pins(document, literal(json!({"type": "integer", "value": 1}))),
            Code::TypeMismatch,
        ),
        case(
            "a literal outside the declared values",
            |document| {
                set_pins(document, literal(json!({"type": "string", "value": "v9"})));
            },
            Code::TypeMismatch,
        ),
        case(
            "an undeclared pinned fact",
            |document| set_pins(document, fact_pin("session.example.other")),
            Code::DanglingReference,
        ),
        case(
            "a pinned fact of another type",
            |document| {
                document["facts"][0]["value_type"] = json!({"type": "integer"});
                set_pins(document, fact_pin(GROUP_FACT));
            },
            Code::TypeMismatch,
        ),
        case(
            "a pinned list",
            |document| {
                document["facts"][0]["value_type"] =
                    json!({"type": "record_list", "key_column": "id", "columns": {}});
                set_pins(document, fact_pin(GROUP_FACT));
            },
            Code::TypeMismatch,
        ),
        case(
            "a pinned fact whose values are not the column's",
            |document| {
                document["facts"][0]["value_type"]["allowed"] = json!(["alpha", "gamma"]);
            },
            Code::TypeMismatch,
        ),
        case(
            "a pin with both a literal and a fact",
            |document| {
                set_pins(
                    document,
                    json!([
                        {"column": "group", "equals_fact": GROUP_FACT,
                         "equals": {"type": "string", "value": "alpha"}},
                        {"column": "variant", "equals": {"type": "string", "value": "v1"}}
                    ]),
                );
            },
            Code::InvalidArguments,
        ),
        case(
            "an otherwise after a literal",
            |document| {
                set_pins(
                    document,
                    json!([
                        {"column": "group", "equals": {"type": "string", "value": "alpha"},
                         "otherwise": {"equals": {"type": "string", "value": "beta"}}},
                        {"column": "variant", "equals": {"type": "string", "value": "v1"}}
                    ]),
                );
            },
            Code::InvalidArguments,
        ),
        case(
            "a term that substitutes a value",
            |document| {
                document["scoring"][0]["on_unknown"] =
                    json!({"kind": "substitute_milli", "value_milli": 1});
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
            "a pinned read without a class gate",
            |document| document["gates"] = json!([]),
            Code::TypeMismatch,
        ),
        case(
            "a tie-break on a row nothing reads",
            |document| {
                let mut other = read("primary_milli");
                other["pins"][1]["equals"]["value"] = json!("v2");
                document["tie_break"] =
                    json!([{"kind": "value", "value": other, "direction": "highest_first"}]);
            },
            Code::TypeMismatch,
        ),
        case(
            "a fallback of another type",
            |document| {
                document["scoring"][0]["value"]["fallback_column"] = json!("listed");
            },
            Code::TypeMismatch,
        ),
        case(
            "a fallback that is the same column",
            |document| {
                document["scoring"][0]["value"]["fallback_column"] = json!("primary_milli");
            },
            Code::TypeMismatch,
        ),
        case(
            "a fallback on a gate",
            |document| {
                document["gates"][0]["predicate"]["value"]["fallback_column"] = json!("listed");
            },
            Code::TypeMismatch,
        ),
        case(
            "an absent on a term",
            |document| {
                document["scoring"][0]["value"]["absent"] = json!({"type": "integer", "value": 0});
            },
            Code::TypeMismatch,
        ),
        case(
            "an absent of another type",
            |document| {
                document["gates"][0]["predicate"]["value"]["absent"] =
                    json!({"type": "integer", "value": 0});
            },
            Code::TypeMismatch,
        ),
    ];
    for (label, change, code) in cases {
        let mut changed = document();
        change(&mut changed);
        let policy: SelectionPolicy = serde_json::from_value(changed).expect(label);
        let error = policy.validate().expect_err(label);
        assert_eq!(error.code(), code, "{label}: {error}");
    }
    // One level of `otherwise`: a nested one does not decode.
    let mut nested = document();
    nested["scoring"][0]["value"]["pins"][0]["otherwise"] =
        json!({"equals_fact": GROUP_FACT, "otherwise": {"equals_fact": GROUP_FACT}});
    assert!(serde_json::from_value::<SelectionPolicy>(nested).is_err());
}
