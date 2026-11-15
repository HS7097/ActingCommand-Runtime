// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V2 (0.12.61; register §1.382; contracts/package-variables.md, "The run";
// contracts/remedies.md section 4): a run reads every variable its package declares as
// `session.dispatch.parameter.<name>` with its kind's type: a `choice` a string, a `switch` a
// boolean, an `integer` an integer, an `ordered` variable a record list of rows `{key, rank}`,
// rank from 1. A run without a dispatch reads the declared defaults (`default_run_values`); a
// policy run the values its dispatch carries when the declaration takes them
// (`dispatched_variable_values`); the overlay passes the snapshot's up-front check and a select
// step's policy reads it as the design's table says (an `enum_string` gate, `integer_compare`, a
// `boolean_equals` gate and, for an `ordered` variable, `keyed_fact` over the list). A package
// that declares no variable reads exactly its parameters, and V1's pin of the parameters of
// `provides` holds. The cap read from an `integer` variable is `gate_390_out2`'s V2 test.
//
// The package is SEL-L6b's scan task (`gate_308_l6b`), with made-up variables; the policies are
// evaluated over made-up candidates with the selection evaluator a select step uses. Every
// expected value comes from the slice's requirement lines.

use super::gate_308_l6b::scan_task;
use super::*;
use actingcommand_contract::{
    FactContent, FactScalar, FactValue, InstanceFactContext, InstanceFactSnapshot,
    parse_package_variables,
};
use actingcommand_selection_policy::{
    Candidate, ScalarValue, SelectionDecision, SelectionFactSnapshot, SelectionOutcome,
    SelectionPolicy, evaluate as evaluate_policy,
};
use serde_json::{Value, json};

const NOW_UNIX_MS: u64 = 1_000_000;
const PICK_KEY: &str = "session.dispatch.parameter.pick";
const FLOOR_KEY: &str = "session.dispatch.parameter.floor";
const ORDER_KEY: &str = "session.dispatch.parameter.order";
const EXTRA_KEY: &str = "session.dispatch.parameter.extra";
const ITEMS: [&str; 3] = ["item_a", "item_b", "item_c"];

// One option `{value, title}` per item.
fn options(items: &[&str]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| json!({"value": item, "title": {"en": item}}))
            .collect(),
    )
}

// A variable of each kind: `pick` (choice, default item_b), `floor` (integer 0 to 200 on steps of
// 10, default 0), `order` (ordered, 0 to 3 keys, default item_b then item_a) and `extra` (switch,
// default false).
fn variables() -> Vec<actingcommand_contract::PackageVariable> {
    parse_package_variables(
        &json!([
            {"name": "pick", "type": "choice", "title": {"en": "Item"},
             "options": options(&ITEMS[..2]), "default": "item_b"},
            {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
             "min": 0, "max": 200, "step": 10, "default": 0},
            {"name": "order", "type": "ordered", "title": {"en": "Order"},
             "options": options(&ITEMS), "min_count": 0, "max_count": 3,
             "default": ["item_b", "item_a"]},
            {"name": "extra", "type": "switch", "title": {"en": "Also the extra"},
             "default": false}
        ]),
        &[],
    )
    .expect("the four variables")
}

// SEL-L6b's scan task with the four variables.
fn declaring() -> PreparedContainedTask {
    let mut task = scan_task();
    task.control.variables = variables();
    task
}

// The parameters a policy run of `task` reads when its dispatch carries `values`.
fn dispatched(task: &PreparedContainedTask, values: &[(&str, Value)]) -> RunParameters {
    let given = values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect::<Vec<_>>();
    task.dispatched_run_parameters(Vec::new(), NOW_UNIX_MS - 1_000)
        .with_variables(task.dispatched_variable_values(&given))
}

// An instance's stored fact snapshot without records, in the package's game and server.
fn instance_snapshot() -> InstanceFactSnapshot {
    InstanceFactSnapshot {
        snapshot_id: "snapshot-v2".to_owned(),
        ledger_position: 7,
        context: InstanceFactContext {
            instance_id: "instance-a".to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        },
        records: Vec::new(),
    }
}

// The snapshot with `parameters` overlaid, after its up-front check (`validate`, which a select
// step runs on the snapshot it is given before it reads it).
fn overlaid(parameters: &RunParameters) -> InstanceFactSnapshot {
    let mut snapshot = instance_snapshot();
    parameters.overlay(&mut snapshot, NOW_UNIX_MS);
    snapshot
        .validate()
        .expect("the overlaid snapshot passes the up-front check");
    snapshot
}

// Each overlaid record's key and inline value, in order.
fn records(snapshot: &InstanceFactSnapshot) -> Vec<(String, FactValue)> {
    snapshot
        .records
        .iter()
        .map(|record| {
            let FactContent::Inline { value } = &record.content else {
                panic!("an overlay record is inline: {record:?}");
            };
            (record.key.clone(), value.clone())
        })
        .collect()
}

// A row of an `ordered` variable's overlay.
fn row(key: &str, rank: i64) -> std::collections::BTreeMap<String, FactScalar> {
    std::collections::BTreeMap::from([
        ("key".to_owned(), FactScalar::String(key.to_owned())),
        ("rank".to_owned(), FactScalar::Integer(rank)),
    ])
}

// A selection policy over `fields` and `facts` with one gate and `scoring`, choosing at most one
// candidate, the lowest id on a tie.
fn policy(fields: Value, facts: Value, predicate: Value, scoring: Value) -> SelectionPolicy {
    let policy: SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "by-variable",
        "applies_to": {
            "candidate_layout_id": "layout/items",
            "outcome_keys": {"selected": "item_selected", "empty": "item_empty",
                             "insufficient": "item_insufficient",
                             "ambiguous": "item_ambiguous", "unknown": "item_unknown"}
        },
        "fields": fields,
        "facts": facts,
        "gates": [{"gate_id": "by-variable", "predicate": predicate,
                   "on_unknown": {"kind": "abort_evaluation"}}],
        "scoring": scoring,
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("selection policy");
    policy.validate().expect("valid selection policy");
    policy
}

// A fact declaration of `key`, fresh for a minute.
fn fact(key: &str, value_type: Value) -> Value {
    json!({"fact_key": key, "value_type": value_type,
           "max_age_ms": 60_000, "minimum_confidence_milli": 1_000})
}

// `policy` evaluated over `candidates` on the snapshot `parameters` overlay, its record lists
// carrying their rows as a select step builds them.
fn decide(
    policy: &SelectionPolicy,
    candidates: &[Candidate],
    parameters: &RunParameters,
) -> SelectionDecision {
    let snapshot = overlaid(parameters);
    let facts = SelectionFactSnapshot::from_instance_snapshot_with_lists(
        &snapshot,
        NOW_UNIX_MS,
        &policy.record_list_keys(),
    );
    evaluate_policy(policy, candidates, &facts, NOW_UNIX_MS).expect("the policy evaluates")
}

fn candidate(id: &str, fields: &[(&str, ScalarValue)]) -> Candidate {
    Candidate {
        candidate_id: id.to_owned(),
        fields: fields
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect(),
    }
}

fn member(id: &str, item: &str) -> Candidate {
    candidate(id, &[("member", ScalarValue::String(item.to_owned()))])
}

// The member field over `items`.
fn member_field(items: &[&str]) -> Value {
    json!([{"name": "member", "value_type": {"type": "enum_string", "allowed": items}}])
}

// Requirement 4 (the overlay's types) and the design's "Not verified", its first point: a run
// without a dispatch reads every variable at its default under the parameter prefix with its
// kind's type, the `ordered` default a record list of `{key, rank}` from rank 1, and the snapshot
// passes the up-front check. The parameters of `provides` alone are those of the package
// without variables (V1's pin), and a package without variables reads exactly its parameters.
#[test]
fn v2_each_kind_is_overlaid_with_its_type_and_passes_the_up_front_check() {
    let task = declaring();
    let snapshot = overlaid(&task.default_run_values());
    assert_eq!(
        records(&snapshot),
        vec![
            (PICK_KEY.to_owned(), FactValue::String("item_b".to_owned())),
            (FLOOR_KEY.to_owned(), FactValue::Integer(0)),
            (
                ORDER_KEY.to_owned(),
                FactValue::RecordList(vec![row("item_b", 1), row("item_a", 2)])
            ),
            (EXTRA_KEY.to_owned(), FactValue::Boolean(false)),
        ]
    );
    assert!(
        snapshot
            .records
            .iter()
            .all(|record| record.observed_at_unix_ms == NOW_UNIX_MS
                && record.expires_at_unix_ms.is_none()
                && record.confidence_milli == 1_000),
        "{snapshot:?}"
    );

    let plain = scan_task();
    assert_eq!(
        task.default_run_parameters(),
        plain.default_run_parameters()
    );
    assert_eq!(plain.default_run_values(), plain.default_run_parameters());
    assert!(overlaid(&plain.default_run_values()).records.is_empty());
}

// Requirement 1 and the design's table in C, first row: a `choice` variable read by an
// `enum_string` gate keeps the candidate whose field is the value: the default `item_b` keeps the
// second, a dispatched `item_a` the first, and a dispatched value that is no option reads the
// default. The select step's `input_sha256` differs per value.
#[test]
fn v2_a_choice_variable_gates_an_enum_string_field() {
    let task = declaring();
    let keeps = |item: &str| {
        json!({"kind": "all", "of": [
            {"kind": "string_in", "value": {"source": "field", "field": "member"},
             "allowed": [item]},
            {"kind": "string_in", "value": {"source": "fact", "fact_key": PICK_KEY},
             "allowed": [item]}
        ]})
    };
    let policy = policy(
        member_field(&ITEMS[..2]),
        json!([fact(
            PICK_KEY,
            json!({"type": "enum_string", "allowed": &ITEMS[..2]})
        )]),
        json!({"kind": "any", "of": [keeps("item_a"), keeps("item_b")]}),
        json!([]),
    );
    let candidates = [member("c0", "item_a"), member("c1", "item_b")];

    let by_default = decide(&policy, &candidates, &task.default_run_values());
    assert_eq!(by_default.selected, ["c1"], "{by_default:?}");
    let by_value = decide(
        &policy,
        &candidates,
        &dispatched(&task, &[("pick", json!("item_a"))]),
    );
    assert_eq!(by_value.selected, ["c0"], "{by_value:?}");
    assert_ne!(by_default.input_sha256, by_value.input_sha256);
    let gone = decide(
        &policy,
        &candidates,
        &dispatched(&task, &[("pick", json!("item_c"))]),
    );
    assert_eq!(gone.selected, ["c1"], "{gone:?}");
}

// The design's table in C, the threshold row: an `integer` variable is the right side of an
// `integer_compare` gate over a field: the default floor 0 keeps both readings (the lowest id
// wins), a floor of 100 keeps only the reading 150, and 200 keeps none (`empty`).
#[test]
fn v2_an_integer_variable_is_the_threshold_of_an_integer_compare() {
    let task = declaring();
    let policy = policy(
        json!([{"name": "reading", "value_type": {"type": "integer"}}]),
        json!([fact(FLOOR_KEY, json!({"type": "integer"}))]),
        json!({"kind": "integer_compare",
               "left": {"source": "field", "field": "reading"}, "op": "ge",
               "right": {"source": "fact", "fact_key": FLOOR_KEY}}),
        json!([]),
    );
    let candidates = [
        candidate("c0", &[("reading", ScalarValue::Integer(50))]),
        candidate("c1", &[("reading", ScalarValue::Integer(150))]),
    ];
    let by_default = decide(&policy, &candidates, &task.default_run_values());
    assert_eq!(by_default.selected, ["c0"], "{by_default:?}");
    let raised = decide(
        &policy,
        &candidates,
        &dispatched(&task, &[("floor", json!(100))]),
    );
    assert_eq!(raised.selected, ["c1"], "{raised:?}");
    let above = decide(
        &policy,
        &candidates,
        &dispatched(&task, &[("floor", json!(200))]),
    );
    assert_eq!(above.outcome, SelectionOutcome::Empty, "{above:?}");
    assert!(above.selected.is_empty());
}

// A `switch` variable reads as a boolean: a `boolean_equals` gate on it keeps nothing with the
// default false and the candidate with a dispatched true.
#[test]
fn v2_a_switch_variable_reads_as_a_boolean() {
    let task = declaring();
    let policy = policy(
        member_field(&ITEMS[..1]),
        json!([fact(EXTRA_KEY, json!({"type": "boolean"}))]),
        json!({"kind": "boolean_equals",
               "value": {"source": "fact", "fact_key": EXTRA_KEY}, "expected": true}),
        json!([]),
    );
    let candidates = [member("c0", "item_a")];
    let by_default = decide(&policy, &candidates, &task.default_run_values());
    assert_eq!(
        by_default.outcome,
        SelectionOutcome::Empty,
        "{by_default:?}"
    );
    let on = decide(
        &policy,
        &candidates,
        &dispatched(&task, &[("extra", json!(true))]),
    );
    assert_eq!(on.selected, ["c0"], "{on:?}");
}

// The design's table in C, the ordered preference row: an `ordered` variable read through
// `keyed_fact` (a gate that the candidate's member is listed, rank at least 1 with absent 0, and a
// term of weight -1000 on its rank) picks the best-ranked candidate present: item_b by the
// default, item_a by a dispatched list of item_a alone; with only item_c on screen, which neither
// list names, the decision is `empty`.
#[test]
fn v2_an_ordered_variable_ranks_the_candidates_through_keyed_fact() {
    let task = declaring();
    let rank = json!({"source": "keyed_fact", "fact_key": ORDER_KEY, "key_field": "member",
                      "value_column": "rank", "absent": {"type": "integer", "value": 0}});
    let policy = policy(
        member_field(&ITEMS),
        json!([fact(
            ORDER_KEY,
            json!({"type": "record_list", "key_column": "key",
                                      "columns": {"rank": {"type": "integer"}}})
        )]),
        json!({"kind": "integer_at_least", "value": rank.clone(), "threshold": 1}),
        json!([{"term_id": "rank", "value": rank, "transform": {"kind": "identity"},
                "weight_milli": -1_000, "on_unknown": {"kind": "abort_evaluation"}}]),
    );
    let all = [
        member("c0", "item_a"),
        member("c1", "item_b"),
        member("c2", "item_c"),
    ];
    let by_default = decide(&policy, &all, &task.default_run_values());
    assert_eq!(by_default.selected, ["c1"], "{by_default:?}");
    let by_value = decide(
        &policy,
        &all,
        &dispatched(&task, &[("order", json!(["item_a"]))]),
    );
    assert_eq!(by_value.selected, ["c0"], "{by_value:?}");
    let none_listed = decide(
        &policy,
        &[member("c2", "item_c")],
        &task.default_run_values(),
    );
    assert_eq!(
        none_listed.outcome,
        SelectionOutcome::Empty,
        "{none_listed:?}"
    );
}
