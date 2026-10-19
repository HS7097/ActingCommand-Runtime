// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L1 (G7; plan-lab-backends §4 L1 R2, R3; register §1.118; apps gate key 76): MCP passes
// the Lab's Fail Loud refusals through its own pre-check form, before the install is located and
// before any actinglab child runs.
//
// - The arguments no path of actinglab `do` or `observe` reads are gone from the tools' schemas
//   (`ac_lab_do`: priority, lease_id, no_wait, recovery_timeout_ms, recovery_poll_ms;
//   `ac_lab_observe`: require_fresh), so a call naming one is `arguments_invalid {argument,
//   argument_problem: unknown}`.
// - `ac_lab_do` with capture and without dry_run does not read the destructive guard: a call
//   setting `destructive` or `allow_destructive` is `arguments_invalid {argument, argument_problem:
//   conflicting}`. Offline (scene) and with dry_run the guard is read and the call goes on.
//
// The tools run in a context whose install cannot be located, so a call that passed MCP's own
// checks stops where the install is located; an `arguments_invalid` answer with no cause is MCP's
// own refusal, given before any child was started.

use super::super::{act, do_input, observe, observe_input};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::mcp::jobs::Jobs;
use crate::mcp::runtime::RuntimeAccess;
use crate::mcp::tests::gate_mcpt1::unreached_deadline;
use crate::mcp::tools::{ToolContext, ToolOutcome};
use serde_json::{Map, Value, json};
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

const ALIAS: &str = "neutral.instance";
const TAP: &str = "10,20";

type Tool = fn(&ToolContext<'_>, &Map<String, Value>) -> ToolOutcome;

/// ac_lab_do's arguments no path of actinglab `do` reads, each with a value it took.
fn do_removed() -> [(&'static str, Value); 5] {
    [
        ("priority", json!("high")),
        ("lease_id", json!("lease_a")),
        ("no_wait", json!(true)),
        ("recovery_timeout_ms", json!(1000)),
        ("recovery_poll_ms", json!(100)),
    ]
}

fn arguments(members: Value) -> Map<String, Value> {
    members.as_object().cloned().unwrap_or_default()
}

/// `tool` with `arguments`, in a context whose install cannot be located.
fn call(tool: Tool, arguments: &Map<String, Value>) -> ToolOutcome {
    let runtime = RuntimeAccess::new(None, None);
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: unreached_deadline(),
        session: 1,
        jobs: &jobs,
    };
    tool(&context, arguments)
}

/// The answer is MCP's own refusal `arguments_invalid {argument, argument_problem}`, class usage,
/// with nothing relayed from a child.
#[track_caller]
fn assert_refused(answer: ToolOutcome, argument: &str, problem: ArgumentProblem, what: &str) {
    let Err(error) = answer else {
        panic!("{what} was not refused");
    };
    let shown = error.to_value();
    assert_eq!(error.class(), "usage", "{what}: {shown}");
    let outcome = error.outcome_ref();
    assert!(
        outcome.is(ActingctlCode::ArgumentsInvalid),
        "{what}: {shown}"
    );
    assert_eq!(
        outcome.value("argument"),
        Some(&json!(argument)),
        "{what}: {shown}"
    );
    assert_eq!(
        outcome.value("argument_problem"),
        Some(&json!(problem.as_str())),
        "{what}: {shown}"
    );
    assert!(
        outcome.causes().is_empty(),
        "{what}: a refusal before any child has no cause: {shown}"
    );
}

/// The call passed MCP's own argument checks: whatever it answered, it is not their refusal.
#[track_caller]
fn assert_passed_the_checks(answer: ToolOutcome, what: &str) {
    if let Err(error) = answer {
        let shown = error.to_value();
        assert!(
            !error.outcome_ref().is(ActingctlCode::ArgumentsInvalid),
            "{what} was refused by MCP's checks: {shown}"
        );
    }
}

#[test]
fn l1_ac_lab_do_and_ac_lab_observe_refuse_the_removed_arguments_as_unknown() {
    let lab = TempDir::new().expect("tempdir");
    let package = lab.path().join("neutral-package");
    let package = package.to_string_lossy();
    for (argument, value) in do_removed() {
        let mut members = arguments(json!({
            "instance": ALIAS,
            "tap": TAP,
            "capture": true,
            "package": package,
        }));
        members.insert(argument.to_owned(), value);
        assert_refused(
            call(act, &members),
            argument,
            ArgumentProblem::Unknown,
            &format!("ac_lab_do with {argument}"),
        );
    }
    let members = arguments(json!({
        "instance": ALIAS,
        "capture": true,
        "package": package,
        "require_fresh": true,
    }));
    assert_refused(
        call(observe, &members),
        "require_fresh",
        ArgumentProblem::Unknown,
        "ac_lab_observe with require_fresh",
    );
}

#[test]
fn l1_ac_lab_do_with_capture_refuses_the_destructive_guard_as_conflicting() {
    let lab = TempDir::new().expect("tempdir");
    let package = lab.path().join("neutral-package");
    let package = package.to_string_lossy();
    for argument in ["allow_destructive", "destructive"] {
        let mut members = arguments(json!({
            "instance": ALIAS,
            "tap": TAP,
            "capture": true,
            "package": package,
        }));
        members.insert(argument.to_owned(), json!(true));
        assert_refused(
            call(act, &members),
            argument,
            ArgumentProblem::Conflicting,
            &format!("ac_lab_do with capture and {argument}"),
        );
    }
}

#[test]
fn l1_the_destructive_guard_passes_offline_and_with_dry_run() {
    let lab = TempDir::new().expect("tempdir");
    let package = lab.path().join("neutral-package");
    let scene = lab.path().join("neutral-scene.png");
    let package = package.to_string_lossy();
    let scene = scene.to_string_lossy();
    for (case, members) in [
        (
            "offline with scene",
            json!({
                "instance": ALIAS,
                "target": "element_a",
                "scene": scene,
                "package": package,
                "dry_run": true,
                "destructive": true,
                "allow_destructive": true,
            }),
        ),
        (
            "capture with dry_run",
            json!({
                "instance": ALIAS,
                "target": "element_a",
                "capture": true,
                "dry_run": true,
                "package": package,
                "allow_destructive": true,
            }),
        ),
        (
            "capture with the guard unset",
            json!({
                "instance": ALIAS,
                "tap": TAP,
                "capture": true,
                "package": package,
                "destructive": false,
                "allow_destructive": false,
            }),
        ),
    ] {
        assert_passed_the_checks(call(act, &arguments(members)), case);
    }
}

#[test]
fn l1_the_tool_schemas_drop_the_removed_arguments() {
    let lab_do = do_input();
    for (argument, _) in do_removed() {
        assert!(
            lab_do["properties"].get(argument).is_none(),
            "ac_lab_do still takes {argument}"
        );
    }
    for argument in ["allow_destructive", "destructive"] {
        assert!(
            lab_do["properties"].get(argument).is_some(),
            "ac_lab_do no longer takes {argument}"
        );
    }
    assert_eq!(lab_do["additionalProperties"], json!(false));
    let lab_observe = observe_input();
    assert!(
        lab_observe["properties"].get("require_fresh").is_none(),
        "ac_lab_observe still takes require_fresh"
    );
    assert_eq!(lab_observe["additionalProperties"], json!(false));
}
