// SPDX-License-Identifier: AGPL-3.0-only

// Workflow 114-A (register `contracts/v012-register.md` §1.236; design
// `rd-1031/design-114-reasons-v5.md` §2.6 and §11.3; `contracts/ledger-observer.md` §11.1): the
// ledger's side of `policy.not_run_changed` before any writer. Its index keys: a change set names
// its changed keys, a baseline part its `rows`, `selected` and `gone`, a retirement its `gone`
// (a `recorder_faulted` one none), a marker none, and every record `not_run_stream/<alias>`; and
// its durability: the record commits with FULL sync, as the value records do. The records are
// JSON records of the frozen shape, decoded and validated as the reader does. No clock is read.

use crate::global::index::not_run_keys;
use actingcommand_contract::{EventPayload, PolicyNotRunChangedPayload, PolicyPayload};
use actingcommand_runtime_database::CommitSync;
use serde_json::{Map, Value, json};

const ALIAS: &str = "instance-a";
const EPOCH: &str = "epoch_00000000000000000000000000000002";

fn row(task: &str) -> Value {
    json!({"task_id": task, "why": {"code": "trigger_false", "category": "info", "values": {}},
        "layer": "candidate", "value_gate": "before", "state": "blocked",
        "eligibility": "false", "since_unix_ms": 1_761_900_000_000_u64})
}

fn summary() -> Value {
    json!({"why": {"code": "due_task_missing", "category": "info", "values": {}},
        "since_unix_ms": 1_761_900_000_000_u64, "layer": "gate"})
}

/// One record of `kind` with `body`'s members, read as the reader reads it.
fn record(kind: &str, base: Value, body: Value) -> EventPayload {
    let mut data = Map::new();
    data.insert("action".to_owned(), json!("policy.not_run"));
    data.insert("kind".to_owned(), json!(kind));
    data.insert("instance_alias".to_owned(), json!(ALIAS));
    data.insert("stream".to_owned(), json!({"owner_epoch": EPOCH, "seq": 2}));
    data.insert(
        "prev".to_owned(),
        json!({"sequence": 40, "owner_epoch": EPOCH, "seq": 1}),
    );
    data.insert("base".to_owned(), base);
    data.insert(
        "evaluated_at_unix_ms".to_owned(),
        json!(1_761_900_000_000_u64),
    );
    data.insert("eval_seq".to_owned(), json!(3));
    data.insert(
        "catalog_hash".to_owned(),
        json!(format!("sha256:{}", "b".repeat(64))),
    );
    data.insert(
        "budget".to_owned(),
        json!({"window_start_unix_ms": 1_761_897_600_000_u64, "used": 2, "bytes": 1024}),
    );
    let Value::Object(body) = body else {
        panic!("a body object");
    };
    data.extend(body);
    data.insert("audit".to_owned(), json!({}));
    let stored = json!({"family": "policy", "payload": {"kind": "not_run_changed", "data": data}});
    let payload: EventPayload = serde_json::from_value(stored).expect("the record decodes");
    payload.validate().expect("the record validates");
    payload
}

fn not_run(payload: &EventPayload) -> &PolicyNotRunChangedPayload {
    let EventPayload::Policy(PolicyPayload::NotRunChanged(record)) = payload else {
        panic!("a policy.not_run_changed record");
    };
    record
}

fn keys(payload: &EventPayload) -> Vec<String> {
    not_run_keys(not_run(payload))
}

fn key(task: &str) -> String {
    format!("not_run/{ALIAS}/{task}")
}

fn stream() -> String {
    format!("not_run_stream/{ALIAS}")
}

fn base() -> Value {
    json!({"sequence": 39, "seq": 1})
}

fn records() -> Vec<(EventPayload, Vec<String>)> {
    let change = record(
        "change",
        base(),
        json!({"entries": [
            {"task_id": "task.alpha", "from": null, "to": row("task.alpha")},
            {"task_id": "task.beta", "from": {"code": "trigger_false"},
             "to": {"gone": "selected", "decision_id": "decision.beta.1", "dispatch": "pending",
                    "attempts": 1}},
        ]}),
    );
    let control_only = record(
        "change",
        base(),
        json!({"entries": [], "control": {"from": {}, "to": {
            "global_pause": {"revision": 1, "pause_reason": "operator.check",
                             "since_unix_ms": 1_761_899_000_000_u64},
            "effective": "global_pause"}}}),
    );
    let baseline = record(
        "baseline",
        Value::Null,
        json!({"baseline_reason": "catalog", "rows": [row("task.alpha")],
            "selected": [{"task_id": "task.delta", "decision_id": "decision.delta.1",
                          "dispatch": "intent", "attempts": 1}],
            "gone": ["task.gamma"], "control": {}, "summary": summary(),
            "part": 1, "parts": 1, "prev_base": null}),
    );
    let suppressed = record(
        "suppressed",
        base(),
        json!({"window": {"start_unix_ms": 1_761_897_600_000_u64,
                          "end_unix_ms": 1_761_901_200_000_u64},
            "trigger": "records", "limit_records": 120, "pending_key_changes": 1}),
    );
    let retired = record(
        "retired",
        base(),
        json!({"reason": "unconfigured", "gone": ["task.alpha", "task.delta"]}),
    );
    let faulted = record(
        "retired",
        base(),
        json!({"reason": "recorder_faulted", "check": "construction"}),
    );
    vec![
        (change, vec![key("task.alpha"), key("task.beta"), stream()]),
        (control_only, vec![stream()]),
        (
            baseline,
            vec![
                key("task.alpha"),
                key("task.delta"),
                key("task.gamma"),
                stream(),
            ],
        ),
        (suppressed, vec![stream()]),
        (
            retired,
            vec![key("task.alpha"), key("task.delta"), stream()],
        ),
        (faulted, vec![stream()]),
    ]
}

// 114-A test `tagged_keys`: a change set gives its changed keys and the stream key; a baseline
// part `rows`, `selected` and `gone` and the stream key; a marker only the stream key; a
// retirement `gone` and the stream key, a `recorder_faulted` one only the stream key. Every record
// is therefore tagged, and commits on arrival.
#[test]
fn n114a_tagged_keys_name_the_changed_keys_and_the_stream() {
    for (payload, expected) in records() {
        assert_eq!(keys(&payload), expected, "{:?}", not_run(&payload).kind());
    }
}

// 114-A test "耐久分类是 Durable": every kind of record commits with FULL sync.
#[test]
fn n114a_the_record_is_durable() {
    for (payload, _) in records() {
        assert!(
            crate::durability::commit_sync(&payload) == CommitSync::Full,
            "{:?}",
            not_run(&payload).kind()
        );
    }
}
