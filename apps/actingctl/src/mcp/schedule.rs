// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_schedule` (Workflow #386 SR-6b, MCP-19b; register §1.144 M19B-2), the MCP side of
//! `actingctl schedule` (`crate::schedule`): the Runtime's `ScheduleExplanation` (register
//! §1.143, §3 W5) with every candidate, the object the CLI prints without its `outcome`. An
//! observer tool that only reads: the Runtime answers from the policy host's memory and records
//! nothing. To fit the output budget it cuts candidate rows only, from the last row of the last
//! entry backwards ([`cut_to_fit`]).

use super::observer::select_instance;
use super::tools::{self, Arguments, ToolContext, ToolOutcome, ToolSuccess};
use serde_json::{Map, Value, json};

pub(super) const DESCRIPTION: &str = "Why each task did not run, per instance, the same read and answer as actingctl schedule: the latest policy evaluation since the Runtime started (none before the first evaluated cycle; a restart empties it), read from the Runtime's memory; nothing is written. Answers {evaluated_at_unix_ms, catalog_hash, instances}, both members absent and instances empty before the first evaluation, and one entry per instance the evaluation decided for, in alias order: {instance_alias, next_wake_unix_ms (its earliest wake), selected {task_id, decision_id, dispatch_state (pending, intent, admitted, rejected or completed; a rejected one did not run either)}, not_running, candidates, candidates_total}. not_running is the instance's one not-running row {task_id, why, next_wake_unix_ms, since_unix_ms}: its first due task that did not run, or, when nothing is due, no task_id and why due_task_missing with the next wake (next_eligible_unix_ms). candidates are every other decision that did not run, due ones first: {task_id, state (deferred or blocked), eligibility (true, false or unknown), why (the outcome of its last reason, the code the ledger's dispatch records name why=), reasons (its last reason codes, at most 32, ending with why's; reasons_omitted counts the earlier ones), next_wake_unix_ms (its own wake), since_unix_ms (since when it has had this why)}; candidates_total counts them all. instance (alias, instance_id or ADB port) narrows it to that instance's entry, resolved with one status read. An answer larger than the output budget loses candidate rows only, from the last row of the last entry backwards, each entry that lost rows marked candidates_truncated: true; evaluated_at_unix_ms, catalog_hash and each entry's instance_alias, next_wake_unix_ms, selected, not_running and candidates_total are never cut, and an answer that still does not fit is output_budget_exceeded: narrow it with instance.";

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
pub(super) fn schedule(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let selector = arguments.string("instance")?;
    let connected = context.runtime.connect()?;
    let alias = match selector {
        Some(selector) => {
            let status = connected
                .client
                .status()
                .map_err(|error| context.runtime.failure(&connected, &error))?;
            Some(
                select_instance(status.instances(), selector)?
                    .instance_alias()
                    .to_owned(),
            )
        }
        None => None,
    };
    let explanation = connected
        .client
        .schedule_explanation(alias.as_deref(), true)
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    Ok(within_budget(json!(explanation)))
}

/// The answer cut to the output budget ([`cut_to_fit`]); one that still does not fit answers
/// `output_budget_exceeded` (`tools::call_result`).
pub(super) fn within_budget(mut answer: Value) -> ToolSuccess {
    cut_to_fit(&mut answer, tools::fits_success);
    ToolSuccess::new(answer)
}

/// Cuts the fewest candidate rows, from the last row of the last entry backwards, after which
/// `fits` takes `answer`; each entry that lost rows is marked `candidates_truncated: true`.
/// Nothing else is cut: when `fits` takes no answer, every candidate row is cut and the answer is
/// left so. Cutting a row more never makes the answer larger (a row is larger than the mark its
/// entry gains), so the search halves the range.
pub(super) fn cut_to_fit(answer: &mut Value, fits: impl Fn(&Value) -> bool) {
    if fits(answer) {
        return;
    }
    let rows = answer
        .get("instances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("candidates").and_then(Value::as_array))
        .map(Vec::len)
        .sum::<usize>();
    let all_cut = cut(answer, rows);
    if !fits(&all_cut) {
        *answer = all_cut;
        return;
    }
    // Cutting `low - 1` rows does not fit, cutting `high` does.
    let (mut low, mut high) = (1, rows);
    while low < high {
        let middle = usize::midpoint(low, high);
        if fits(&cut(answer, middle)) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    *answer = cut(answer, high);
}

/// `answer` without its last `rows` candidate rows, counted from the last row of the last entry
/// backwards, each entry that lost one marked `candidates_truncated: true`.
fn cut(answer: &Value, rows: usize) -> Value {
    let mut shortened = answer.clone();
    let mut left = rows;
    let entries = shortened
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .rev();
    for entry in entries {
        if left == 0 {
            break;
        }
        let Some(members) = entry.as_object_mut() else {
            continue;
        };
        let Some(candidates) = members.get_mut("candidates").and_then(Value::as_array_mut) else {
            continue;
        };
        let taken = left.min(candidates.len());
        if taken == 0 {
            continue;
        }
        candidates.truncate(candidates.len() - taken);
        left -= taken;
        members.insert("candidates_truncated".to_owned(), json!(true));
    }
    shortened
}
