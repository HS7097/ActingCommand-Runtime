// SPDX-License-Identifier: AGPL-3.0-only

// #412 Lab B-3 S2 (frozen design rd-1021/train-0129/lab-b3-design.md §1.3 and §5 "S2", T2-7;
// register §1.162; apps key 92): `ac_lab_do` takes `operation`, a text passed to actinglab as
// `--operation`, with `seed` read with it; `operation` together with target, tap, swipe,
// after_page, after_timeout_ms, record, tap_rect, projection_sequence, projection_hash, scene,
// sample_scenes or dry_run, or without `capture: true`, is refused before the install is located
// and before any child runs: `arguments_invalid {argument, argument_problem: conflicting}`,
// class usage.
//
// The tools run in a context whose install cannot be located, so a call that passed MCP's own
// checks stops at `Program::locate`; an `arguments_invalid` answer with no cause is MCP's own
// refusal, given before any child was started. The line a call would run is read with
// `command_line`.

use super::super::{DO, act, command_line};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::mcp::jobs::Jobs;
use crate::mcp::protocol::Era;
use crate::mcp::runtime::RuntimeAccess;
use crate::mcp::tests::gate_mcpt1::unreached_deadline;
use crate::mcp::tools::{ToolContext, ToolOutcome};
use serde_json::{Map, Value, json};
use std::sync::atomic::AtomicBool;

fn arguments(members: Value) -> Map<String, Value> {
    members.as_object().cloned().unwrap_or_default()
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// `ac_lab_do` with `arguments`, in a context whose install cannot be located.
fn call(arguments: &Map<String, Value>) -> ToolOutcome {
    let runtime = RuntimeAccess::new(None, None);
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: unreached_deadline(),
        era: Era::Modern,
        session: 1,
        jobs: &jobs,
    };
    act(&context, arguments)
}

/// The answer is MCP's own refusal `arguments_invalid {argument, argument_problem:
/// conflicting}`, class usage, with nothing relayed from a child.
#[track_caller]
fn assert_conflicting(answer: ToolOutcome, argument: &str) {
    let Err(error) = answer else {
        panic!("{argument} was not refused");
    };
    let shown = error.to_value();
    assert_eq!(error.class(), "usage", "{argument}: {shown}");
    let outcome = error.outcome_ref();
    assert!(
        outcome.is(ActingctlCode::ArgumentsInvalid),
        "{argument}: {shown}"
    );
    assert_eq!(outcome.value("argument"), Some(&json!(argument)), "{shown}");
    assert_eq!(
        outcome.value("argument_problem"),
        Some(&json!(ArgumentProblem::Conflicting.as_str())),
        "{shown}"
    );
    assert!(outcome.causes().is_empty(), "{argument}: {shown}");
}

#[test]
fn b3_s2_t2_7_ac_lab_do_passes_operation_and_refuses_its_conflicts_before_the_child() {
    // `operation` is `--operation <id>` among the valued flags in the tool's flag order, with
    // `seed` read with it; the switches follow them.
    let line = command_line(
        &DO,
        &arguments(json!({
            "operation": "op_a",
            "capture": true,
            "zip": "neutral.zip",
            "seed": 23,
        })),
    )
    .unwrap_or_else(|error| panic!("ac_lab_do with operation was refused: {}", error.to_value()));
    assert_eq!(
        line,
        words(&[
            "do",
            "--zip",
            "neutral.zip",
            "--operation",
            "op_a",
            "--seed",
            "23",
            "--capture",
        ])
    );

    let base = || {
        arguments(json!({
            "operation": "op_a",
            "capture": true,
            "zip": "neutral.zip",
        }))
    };
    let conflicts: [(&str, Value); 12] = [
        ("target", json!("element_a")),
        ("tap", json!("1,1")),
        ("swipe", json!("1,1,2,2,100")),
        ("after_page", json!("neutral/page_b")),
        ("after_timeout_ms", json!(1000)),
        ("record", json!(true)),
        ("tap_rect", json!("1,1,2,2")),
        ("projection_sequence", json!(3)),
        ("projection_hash", json!("a".repeat(64))),
        ("scene", json!("scene.png")),
        ("sample_scenes", json!(["first.png"])),
        ("dry_run", json!(true)),
    ];
    for (argument, value) in conflicts {
        let mut members = base();
        members.insert(argument.to_owned(), value);
        assert_conflicting(call(&members), argument);
    }
    // Without capture.
    let mut offline = base();
    offline.remove("capture");
    assert_conflicting(call(&offline), "operation");
}
