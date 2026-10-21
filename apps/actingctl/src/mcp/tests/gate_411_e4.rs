// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 slice E4, T1 (B27; register §1.156 E4-1; apps key 87). `ac_events` given an
//! event type that does not exist refuses it as before: `arguments_invalid` (class `usage`) with
//! the argument and `argument_problem` `not_recognised`. The refusal also names the value it
//! received as `argument_value`: the text of `event_type`, and the element of `event_types` or
//! `exclude_event_types` that names no event type. The arguments are refused before any Runtime
//! read, so no Runtime runs here.

use super::super::jobs::Jobs;
use super::super::runtime::RuntimeAccess;
use super::super::tools::{ToolContext, find};
use super::gate_mcpt1::unreached_deadline;
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

/// An event type as a caller misspells it.
const MISSPELT: &str = "task.faild";

#[test]
fn e4_t1_ac_events_names_the_event_type_it_does_not_know() {
    let root = TempDir::new().expect("tempdir");
    let runtime = RuntimeAccess::new(None, Some(root.path().to_path_buf()));
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: unreached_deadline(),
        session: 1,
        jobs: &jobs,
    };
    let tool = find("ac_events").expect("ac_events is in the tool table");
    for (arguments, argument) in [
        (json!({"event_type": MISSPELT}), "event_type"),
        (
            json!({"event_types": ["task.failed", MISSPELT]}),
            "event_types",
        ),
        (
            json!({"exclude_event_types": [MISSPELT]}),
            "exclude_event_types",
        ),
    ] {
        let Value::Object(arguments) = arguments else {
            panic!("the arguments are an object");
        };
        let Err(error) = (tool.run)(&context, &arguments) else {
            panic!("ac_events {arguments:?} was accepted");
        };
        assert_eq!(error.class(), "usage", "{}", error.to_value());
        let outcome = error.outcome_ref();
        assert_eq!(
            outcome.code().as_str(),
            "arguments_invalid",
            "{}",
            error.to_value()
        );
        assert_eq!(
            outcome.value("argument"),
            Some(&json!(argument)),
            "{}",
            error.to_value()
        );
        assert_eq!(
            outcome.value("argument_problem"),
            Some(&json!("not_recognised")),
            "{}",
            error.to_value()
        );
        // B27: the value it received, as a typed value.
        assert_eq!(
            outcome.value("argument_value"),
            Some(&json!(MISSPELT)),
            "{argument}: {}",
            error.to_value()
        );
    }
}
