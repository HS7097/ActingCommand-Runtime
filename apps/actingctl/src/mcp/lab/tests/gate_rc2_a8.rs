// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A8 and A14 (v0.12.1-rc.2; register §1.100; apps key 73): MCP refuses, before the
// install is located and before any actinglab child runs, the Lab calls it can already tell are
// wrong, naming the argument.
//
// - A8-1: ac_lab_do and ac_lab_observe with capture, without dry_run and with neither package nor
//   zip would reach actinglab's `PackageInput::open` with no locator (every online Lab request
//   carries its package, contained-lab-operation.md): `arguments_invalid {argument: package,
//   argument_problem: required}`, class usage. With a locator, without capture, or with dry_run
//   the call goes on to actinglab as before.
// - A14-1: ac_lab_do with target `tap` and tap (target is a Runtime element id): `arguments_invalid
//   {argument: tap, argument_problem: conflicting}`, class usage.
//
// The tools run in a context whose install cannot be located, so a call that passed MCP's own
// checks would stop at `Program::locate` (`lab_tool_unavailable`); and every answer of an
// actinglab child is `lab_tool_failed`, `lab_process_failed` or `lab_process_timeout`
// (`interpret`, `child_failure`). An `arguments_invalid` answer with no cause is therefore MCP's
// own refusal, given before the install was located and before any child was started.

use super::super::{act, observe, opens_package_without_locator};
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

#[test]
fn a8_ac_lab_do_with_capture_and_no_package_is_arguments_invalid_naming_package() {
    let answer = call(
        act,
        &arguments(json!({"instance": ALIAS, "tap": TAP, "capture": true})),
    );
    assert_refused(
        answer,
        "package",
        ArgumentProblem::Required,
        "ac_lab_do with capture and no package",
    );
}

#[test]
fn a8_ac_lab_observe_with_capture_and_no_package_is_refused_the_same() {
    let answer = call(
        observe,
        &arguments(json!({"instance": ALIAS, "capture": true})),
    );
    assert_refused(
        answer,
        "package",
        ArgumentProblem::Required,
        "ac_lab_observe with capture and no package",
    );
}

#[test]
fn a8_a_locator_no_capture_or_dry_run_passes_the_package_check() {
    let lab = TempDir::new().expect("tempdir");
    let package = lab.path().join("neutral-package");
    let zip = lab.path().join("neutral-package.zip");
    let package = package.to_string_lossy();
    let zip = zip.to_string_lossy();
    for (case, members) in [
        (
            "capture with package",
            json!({"instance": ALIAS, "tap": TAP, "capture": true, "package": package}),
        ),
        (
            "capture with zip",
            json!({"instance": ALIAS, "tap": TAP, "capture": true, "zip": zip}),
        ),
        ("no capture", json!({"instance": ALIAS, "tap": TAP})),
        (
            "capture false",
            json!({"instance": ALIAS, "tap": TAP, "capture": false}),
        ),
        (
            "capture with dry_run",
            json!({"instance": ALIAS, "tap": TAP, "capture": true, "dry_run": true}),
        ),
    ] {
        assert!(
            !opens_package_without_locator(&arguments(members)),
            "{case} is refused for a missing package"
        );
    }
}

#[test]
fn a14_ac_lab_do_with_target_tap_and_tap_is_arguments_invalid_naming_tap() {
    let lab = TempDir::new().expect("tempdir");
    let package = lab.path().join("neutral-package");
    let answer = call(
        act,
        &arguments(json!({
            "instance": ALIAS,
            "target": "tap",
            "tap": TAP,
            "capture": true,
            "package": package.to_string_lossy(),
        })),
    );
    assert_refused(
        answer,
        "tap",
        ArgumentProblem::Conflicting,
        "ac_lab_do with target tap and tap",
    );
}
