// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_schedule` (Workflow #386 SR-6b, MCP-19b; register §1.144 M19B-2), the MCP side of
//! `actingctl schedule` (`crate::schedule`): the Runtime's `ScheduleExplanation` (register
//! §1.143, §3 W5) with every candidate, the object the CLI prints without its `outcome`. An
//! observer tool that only reads: the Runtime answers from the policy host's memory and records
//! nothing. To fit the output budget it cuts candidate rows only, from the last row of the last
//! entry backwards (Workflow #434 LAB-5D: `schedule::within_budget`, with this server's budget).

use super::tools::{self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess};
use actingcommand_lab_ops::schedule::{self as operation, Failure, explanation};
use serde_json::{Map, Value, json};

// Workflow #434 LAB-5D: the cut the gates of ac_schedule read, `schedule`'s now.
#[cfg(test)]
pub(super) use actingcommand_lab_ops::schedule::cut_to_fit;

pub(super) const DESCRIPTION: &str = "Why each task did not run, per instance, the same read and answer as actingctl schedule: the latest policy evaluation since the Runtime started (none before the first evaluated cycle; a restart empties it), read from the Runtime's memory; nothing is written. Answers {evaluated_at_unix_ms, catalog_hash, instances}, both members absent and instances empty before the first evaluation, and one entry per instance the evaluation decided for, in alias order: {instance_alias, next_wake_unix_ms (its earliest wake), selected {task_id, decision_id, dispatch_state (pending, intent, admitted, rejected or completed; a rejected one did not run either)}, not_running, candidates, candidates_total}. not_running is the instance's one not-running row {task_id, why, next_wake_unix_ms, since_unix_ms}: its first due task that did not run, or, when nothing is due, no task_id and why due_task_missing with the next wake (next_eligible_unix_ms). candidates are every other decision that did not run, due ones first: {task_id, state (deferred or blocked), eligibility (true, false or unknown), why (the outcome of its last reason, the code the ledger's dispatch records name why=), reasons (its last reason codes, at most 32, ending with why's; reasons_omitted counts the earlier ones), next_wake_unix_ms (its own wake), since_unix_ms (since when it has had this why), expected_runtime_ms (the time its dispatch would reserve: its expected duration, or its declared duration) and expected_runtime_source (declared, or runs:<n>: the mean of the latest n successful scheduled runs of its package on the instance plus the margin)}; candidates_total counts them all. instance (alias, instance_id or ADB port) narrows it to that instance's entry, resolved with one status read. An answer larger than the output budget loses candidate rows only, from the last row of the last entry backwards, each entry that lost rows marked candidates_truncated: true; evaluated_at_unix_ms, catalog_hash and each entry's instance_alias, next_wake_unix_ms, selected, not_running and candidates_total are never cut, and an answer that still does not fit is output_budget_exceeded: narrow it with instance.";

/// `{instance?}`: alias, instance_id or ADB port, as the CLI's `--instance`.
pub(super) fn input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": {
                "type": "string",
                "minLength": 1,
                "description": "Instance alias, instance_id or ADB port; without it every instance.",
            },
        },
        "additionalProperties": false,
    })
}

/// The answer's shape: `RuntimeScheduleExplanation`, with the cut marker.
pub(super) fn result() -> Value {
    let candidate = json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "state": {"enum": ["deferred", "blocked"]},
            "eligibility": {"enum": ["true", "false", "unknown"]},
            "why": {"type": "object"},
            "reasons": {"type": "array", "items": {"type": "string"}},
            "reasons_omitted": {"type": "integer"},
            "next_wake_unix_ms": {"type": "integer"},
            "since_unix_ms": {"type": "integer"},
            "expected_runtime_ms": {"type": "integer"},
            "expected_runtime_source": {"type": "string"},
        },
        "required": ["task_id", "state", "eligibility", "why", "reasons", "since_unix_ms"],
    });
    json!({
        "type": "object",
        "properties": {
            "evaluated_at_unix_ms": {"type": "integer"},
            "catalog_hash": {"type": "string"},
            "instances": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "instance_alias": {"type": "string"},
                        "next_wake_unix_ms": {"type": "integer"},
                        "selected": {
                            "type": "object",
                            "properties": {
                                "task_id": {"type": "string"},
                                "decision_id": {"type": "string"},
                                "dispatch_state": {
                                    "enum": ["pending", "intent", "admitted", "rejected", "completed"],
                                },
                            },
                            "required": ["task_id", "decision_id", "dispatch_state"],
                        },
                        "not_running": {
                            "type": "object",
                            "properties": {
                                "task_id": {"type": "string"},
                                "why": {"type": "object"},
                                "next_wake_unix_ms": {"type": "integer"},
                                "since_unix_ms": {"type": "integer"},
                            },
                            "required": ["why", "since_unix_ms"],
                        },
                        "candidates": {"type": "array", "items": candidate},
                        "candidates_total": {"type": "integer"},
                        "candidates_truncated": {"const": true},
                    },
                    "required": ["instance_alias", "not_running", "candidates_total"],
                },
            },
        },
        "required": ["instances"],
    })
}

/// `ac_schedule {instance?}`: one ScheduleExplanation read with candidates, after one status
/// read that resolves `instance` to its alias when it is given.
/// Workflow #434 LAB-1b: the reads are `schedule::explanation`'s.
pub(super) fn schedule(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let selector = arguments.string("instance")?;
    let connected = context.runtime.connect()?;
    let explained = explanation(&connected.client, selector).map_err(|failure| match failure {
        Failure::Runtime(error) => context.runtime.failure(&connected, &error),
        Failure::Selector { miss, selector } => {
            ToolError::outcome("usage", miss.outcome(&selector))
        }
    })?;
    Ok(within_budget(json!(explained)))
}

/// The answer cut to the output budget (Workflow #434 LAB-5D: `schedule::within_budget`, whose
/// cut is `schedule::cut_to_fit`, with this server's budget); one that still does not fit
/// answers `output_budget_exceeded` (`tools::call_result`).
pub(super) fn within_budget(answer: Value) -> ToolSuccess {
    ToolSuccess::new(operation::within_budget(
        answer,
        Some(&tools::answer_budget()),
    ))
}
