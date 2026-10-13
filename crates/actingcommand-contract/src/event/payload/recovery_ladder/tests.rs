// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #387 D1c, test row D1-L1 (frozen model M387 v1.2 §8.2): the ladder records describe
// themselves and validation checks only that each agrees with itself (§6.2). Every rule of the
// §6.2 table gets one record that keeps it and one that breaks it; any token code is accepted,
// since no code table is read any more.

use super::*;
use serde_json::{Value, json};

const RUN: &str = "run_11111111111111111111111111111111";
const TASK: &str = "task_22222222222222222222222222222222";
const EVENT: &str = "evt_33333333333333333333333333333333";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn phase(value: Value) -> RuntimeLifecyclePhase {
    serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("the phase decodes: {error}: {value}"))
}

fn accepted(value: Value) -> bool {
    validate_recovery_ladder_phase(&phase(value)).is_ok()
}

fn rungs() -> Value {
    json!([
        {"rung": "return_home", "state": "pending"},
        {"rung": "application_restart", "state": "skipped", "reason": "no_startup_package"},
        {"rung": "emulator_restart", "state": "pending"},
    ])
}

fn started(trigger: Value) -> Value {
    json!({
        "kind": "recovery_ladder_started",
        "trigger": trigger,
        "rungs": rungs(),
        "config_digest": DIGEST,
    })
}

fn task_trigger() -> Value {
    json!({
        "stage": "task",
        "run_id": RUN,
        "task_id": TASK,
        "failure_code": "contained_task_page_unknown",
        "rule": "pack_logic",
        "matched": {"by": "class", "ladder_class": "pack_logic_mismatch"},
        "start": "immediately",
    })
}

fn preparation_trigger() -> Value {
    json!({
        "stage": "connection_preparation",
        "preparation": {"sequence": 7, "event_id": EVENT},
        "failure_code": "capture_backend_open_failed",
        "rule": "preparation_backend",
        "matched": {"by": "code"},
        "start": "immediately",
    })
}

fn burst_trigger() -> Value {
    json!({
        "stage": "stability_warnings",
        "failure_code": "capture_backend_operation_failed",
        "rule": "stability_burst",
        "matched": {
            "by": "warning_burst",
            "count": 3,
            "window_ms": 600_000,
            "first_unix_ms": 1_000,
            "last_unix_ms": 2_000,
        },
        "start": "immediately",
    })
}

fn fence_trigger() -> Value {
    json!({
        "stage": "resource_fence",
        "failure_code": "capture_backend_operation_failed",
        "start": "immediately",
    })
}

/// `value` with `member` set to `replacement` (or removed when it is `null`).
fn with(mut value: Value, member: &str, replacement: Value) -> Value {
    let object = value.as_object_mut().expect("an object");
    if replacement.is_null() {
        object.remove(member);
    } else {
        object.insert(member.to_owned(), replacement);
    }
    value
}

#[test]
fn d1_l1_each_trigger_stage_keeps_its_links_and_how_it_matched() {
    let cases = [
        // task: a run and a task, no preparation, a rule, matched by code or class.
        ("task", task_trigger(), true),
        (
            "task matched by code",
            with(task_trigger(), "matched", json!({"by": "code"})),
            true,
        ),
        (
            "task without its run",
            with(task_trigger(), "run_id", Value::Null),
            false,
        ),
        (
            "task with a preparation",
            with(
                task_trigger(),
                "preparation",
                json!({"sequence": 7, "event_id": EVENT}),
            ),
            false,
        ),
        (
            "task matched as a burst",
            with(
                task_trigger(),
                "matched",
                burst_trigger()["matched"].clone(),
            ),
            false,
        ),
        (
            "task without a rule",
            with(task_trigger(), "rule", Value::Null),
            false,
        ),
        // startup or connection preparation: a preparation after sequence 0, no run.
        ("connection preparation", preparation_trigger(), true),
        (
            "startup preparation",
            with(preparation_trigger(), "stage", json!("startup_preparation")),
            true,
        ),
        (
            "preparation at sequence 0",
            with(
                preparation_trigger(),
                "preparation",
                json!({"sequence": 0, "event_id": EVENT}),
            ),
            false,
        ),
        (
            "preparation with a run",
            with(preparation_trigger(), "run_id", json!(RUN)),
            false,
        ),
        // stability warnings: nothing linked, a burst of at least one record in a window.
        ("burst", burst_trigger(), true),
        (
            "burst of no record",
            with(
                burst_trigger(),
                "matched",
                json!({"by": "warning_burst", "count": 0, "window_ms": 600_000,
                       "first_unix_ms": 1_000, "last_unix_ms": 2_000}),
            ),
            false,
        ),
        (
            "burst whose first record is after its last",
            with(
                burst_trigger(),
                "matched",
                json!({"by": "warning_burst", "count": 3, "window_ms": 600_000,
                       "first_unix_ms": 3_000, "last_unix_ms": 2_000}),
            ),
            false,
        ),
        (
            "burst in no window",
            with(
                burst_trigger(),
                "matched",
                json!({"by": "warning_burst", "count": 3, "window_ms": 0,
                       "first_unix_ms": 1_000, "last_unix_ms": 2_000}),
            ),
            false,
        ),
        (
            "burst matched by class",
            with(
                burst_trigger(),
                "matched",
                json!({"by": "class", "ladder_class": "environment_fault"}),
            ),
            false,
        ),
        (
            "burst with a task",
            with(burst_trigger(), "task_id", json!(TASK)),
            false,
        ),
        // recovery preparation: never (A2).
        (
            "recovery preparation",
            with(
                preparation_trigger(),
                "stage",
                json!("recovery_preparation"),
            ),
            false,
        ),
        // resource fence: nothing linked, no trigger item.
        ("resource fence", fence_trigger(), true),
        (
            "resource fence with a rule",
            with(fence_trigger(), "rule", json!("pack_logic")),
            false,
        ),
        // every stage: the rule is a token.
        (
            "a rule that is not a token",
            with(task_trigger(), "rule", json!("Pack-Logic")),
            false,
        ),
        // any token code is a trigger's code: no code table is read.
        (
            "an unlisted code",
            with(
                task_trigger(),
                "failure_code",
                json!("neutral_unlisted_failure"),
            ),
            true,
        ),
        (
            "waiting for performance release",
            with(task_trigger(), "start", json!("after_performance_release")),
            true,
        ),
    ];
    for (case, trigger, valid) in cases {
        assert_eq!(accepted(started(trigger)), valid, "{case}");
    }
}

#[test]
fn d1_l1_a_rung_plan_has_one_to_three_distinct_actions_and_a_reason_exactly_when_skipped() {
    let plan = |rungs: Value| with(started(task_trigger()), "rungs", rungs);
    let cases = [
        (
            "configured order",
            json!([
                {"rung": "application_restart", "state": "pending"},
                {"rung": "return_home", "state": "skipped", "reason": "before_start_rung"},
            ]),
            true,
        ),
        (
            "one rung",
            json!([{"rung": "emulator_restart", "state": "pending"}]),
            true,
        ),
        ("no rung", json!([]), false),
        (
            "an action twice",
            json!([
                {"rung": "return_home", "state": "pending"},
                {"rung": "return_home", "state": "pending"},
            ]),
            false,
        ),
        (
            "skipped without a reason",
            json!([
                {"rung": "return_home", "state": "skipped"},
            ]),
            false,
        ),
        (
            "pending with a reason",
            json!([
                {"rung": "return_home", "state": "pending", "reason": "no_recovery_package"},
            ]),
            false,
        ),
        (
            "skipped as the device is not ready",
            json!([
                {"rung": "return_home", "state": "skipped", "reason": "device_not_ready"},
            ]),
            true,
        ),
    ];
    for (case, rungs, valid) in cases {
        assert_eq!(accepted(plan(rungs)), valid, "{case}");
    }
}

#[test]
fn d1_l1_a_started_ladder_names_its_configuration_digest_and_an_optional_profile() {
    let cases = [
        ("a digest", started(task_trigger()), true),
        (
            "a short digest",
            with(started(task_trigger()), "config_digest", json!("0123abcd")),
            false,
        ),
        (
            "an upper-case digest",
            with(
                started(task_trigger()),
                "config_digest",
                json!(DIGEST.to_ascii_uppercase()),
            ),
            false,
        ),
        (
            "a profile",
            with(started(task_trigger()), "profile", json!("handset")),
            true,
        ),
        (
            "a profile that is not a token",
            with(started(task_trigger()), "profile", json!("Hand Set")),
            false,
        ),
    ];
    for (case, value, valid) in cases {
        assert_eq!(accepted(value), valid, "{case}");
    }
}

#[test]
fn d1_l1_preparation_suppression_and_end_records_keep_their_rules() {
    let preparation = |stage: &str| {
        json!({
            "kind": "instance_preparation_finished",
            "stage": stage,
            "capture": "passed",
            "input": "passed",
        })
    };
    let suppressed = |rule: Value| {
        with(
            json!({"kind": "recovery_ladder_suppressed", "reason": "cooldown", "until_unix_ms": 9}),
            "rule",
            rule,
        )
    };
    let finished = |rungs_tried: u8| {
        json!({
            "kind": "recovery_ladder_finished",
            "ladder_result": "recovered",
            "rungs_tried": rungs_tried,
        })
    };
    let cases = [
        (
            "a connection preparation",
            preparation("connection_preparation"),
            true,
        ),
        ("a task preparation", preparation("task"), false),
        (
            "a preparation of a warning burst",
            preparation("stability_warnings"),
            false,
        ),
        (
            "a suppression naming its rule",
            suppressed(json!("pack_logic")),
            true,
        ),
        (
            "a suppression naming no rule",
            suppressed(Value::Null),
            true,
        ),
        (
            "a suppression naming a rule that is not a token",
            suppressed(json!("")),
            false,
        ),
        ("three rungs tried", finished(3), true),
        ("four rungs tried", finished(4), false),
    ];
    for (case, value, valid) in cases {
        assert_eq!(accepted(value), valid, "{case}");
    }
}
