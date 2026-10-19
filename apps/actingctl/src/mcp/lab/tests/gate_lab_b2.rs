// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab B-2 (G8 offline; plan-lab-l5 §2 B-2 R5; coordinator decision Q1 of 2026-10-19;
// register §1.131; apps gate key 78): MCP takes an offline frame set.
//
// - `ac_lab_observe` and `ac_lab_do` take `sample_scenes`, a list of 1 to 4 strings, none
//   flag-like: the earlier frames, oldest first, of `scene`'s set.
// - actinglab receives `--scene <s1> ... --scene <scene>`: the earlier frames before the current
//   one.
// - `sample_scenes` without `scene` is `arguments_invalid {argument: sample_scenes,
//   argument_problem: conflicting}` before the install is located and before any actinglab child
//   runs; so is a list outside 1..4 (`out_of_range`) or with a flag-like item (`not_recognised`).
//
// The tools run in a context whose install cannot be located, so a call that passed MCP's own
// checks stops where the install is located; an `arguments_invalid` answer with no cause is MCP's
// own refusal, given before any child was started.

use super::super::{DO, NOT_A_FLAG, OBSERVE, act, command_line, do_input, observe, observe_input};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::mcp::jobs::Jobs;
use crate::mcp::runtime::RuntimeAccess;
use crate::mcp::tests::gate_mcpt1::unreached_deadline;
use crate::mcp::tools::{ToolContext, ToolOutcome};
use serde_json::{Map, Value, json};
use std::sync::atomic::AtomicBool;

type Tool = fn(&ToolContext<'_>, &Map<String, Value>) -> ToolOutcome;

fn arguments(members: Value) -> Map<String, Value> {
    members.as_object().cloned().unwrap_or_default()
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
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
fn b2_ac_lab_observe_and_do_pass_sample_scenes_before_scene() {
    // The command line: every earlier frame, oldest first, then the current one.
    let observed = command_line(
        &OBSERVE,
        &arguments(json!({
            "scene": "current.png",
            "sample_scenes": ["first.png", "second.png"],
            "zip": "neutral.zip",
        })),
    )
    .unwrap_or_else(|error| {
        panic!(
            "ac_lab_observe with sample_scenes was refused: {}",
            error.to_value()
        )
    });
    assert_eq!(
        observed,
        words(&[
            "observe",
            "--scene",
            "first.png",
            "--scene",
            "second.png",
            "--scene",
            "current.png",
            "--zip",
            "neutral.zip",
        ])
    );
    let planned = command_line(
        &DO,
        &arguments(json!({
            "target": "element_a",
            "scene": "current.png",
            "sample_scenes": ["first.png"],
            "zip": "neutral.zip",
            "dry_run": true,
        })),
    )
    .unwrap_or_else(|error| {
        panic!(
            "ac_lab_do with sample_scenes was refused: {}",
            error.to_value()
        )
    });
    assert_eq!(
        planned,
        words(&[
            "do",
            "element_a",
            "--scene",
            "first.png",
            "--scene",
            "current.png",
            "--zip",
            "neutral.zip",
            "--dry-run",
        ])
    );

    // The schema: a list of 1 to 4 strings, none flag-like.
    for (tool, schema) in [
        ("ac_lab_observe", observe_input()),
        ("ac_lab_do", do_input()),
    ] {
        let samples = &schema["properties"]["sample_scenes"];
        assert_eq!(samples["type"], "array", "{tool}: {samples}");
        assert_eq!(samples["minItems"], 1, "{tool}: {samples}");
        assert_eq!(samples["maxItems"], 4, "{tool}: {samples}");
        assert_eq!(samples["items"]["type"], "string", "{tool}: {samples}");
        assert_eq!(samples["items"]["minLength"], 1, "{tool}: {samples}");
        assert_eq!(samples["items"]["pattern"], NOT_A_FLAG, "{tool}: {samples}");
    }

    // Without scene, sample_scenes is refused before any child runs; so are a list outside 1..4
    // and a flag-like item.
    for (tool, run, members) in [
        (
            "ac_lab_observe",
            observe as Tool,
            json!({"sample_scenes": ["first.png"], "zip": "neutral.zip"}),
        ),
        (
            "ac_lab_do",
            act as Tool,
            json!({"target": "element_a", "sample_scenes": ["first.png"],
                   "zip": "neutral.zip", "dry_run": true}),
        ),
    ] {
        assert_refused(
            call(run, &arguments(members.clone())),
            "sample_scenes",
            ArgumentProblem::Conflicting,
            &format!("{tool} with sample_scenes and no scene"),
        );
        for (samples, problem) in [
            (json!([]), ArgumentProblem::OutOfRange),
            (
                json!(["s1.png", "s2.png", "s3.png", "s4.png", "s5.png"]),
                ArgumentProblem::OutOfRange,
            ),
            (json!(["--capture"]), ArgumentProblem::NotRecognised),
        ] {
            let mut members = arguments(members.clone());
            members.insert("scene".to_owned(), json!("current.png"));
            members.insert("sample_scenes".to_owned(), samples.clone());
            assert_refused(
                call(run, &members),
                "sample_scenes",
                problem,
                &format!("{tool} with sample_scenes {samples}"),
            );
        }
    }
}
