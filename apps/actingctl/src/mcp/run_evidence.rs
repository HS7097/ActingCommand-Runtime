// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_run_evidence` (Workflow #389 MCP-6, MCP-0 command table v1.1 §4.7): `actingctl run
//! evidence` over MCP. Its arguments are the CLI's flags (R2; `handle` ↔ `--request-id`, as for
//! `ac_get_run`) and its result is the CLI's stdout JSON (`crate::run_evidence`), cut from its
//! last step to the output budget: `truncated`, with `next_step` after the last step kept
//! (§2.2). Before U-3 an observer tool, after it a general read (command table §3).

use super::runtime::Connected;
use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::ArgumentProblem;
use crate::run_evidence::{self, Evidence, EvidenceRequest, Failure};
use actingcommand_runtime_client::RunKey;
use serde_json::{Map, Value, json};

pub(super) const DESCRIPTION: &str = "What one contained run did, step by step, as actingctl run evidence answers it, read from the run's own ledger events (QueryEvents, which records nothing). Name the run by handle (its Runtime request_id, as ac_run_pack returns it) or by run_id; give exactly one. Returns {request_id, run_id, task_id, steps, final, next_step, overlay}. Each step, in the order the run started them: step_index; page, the page it started from; targets, the targets of the recognition it decided on, each {target, matched, box} as the recognition fact records it (the fact keeps no score); action, its first input {kind, point, rect}: point where a tap or long tap lands, rect the one box the point was drawn in (no action when the step made no input); frames, [{ref}] of the frame it decided on and of every frame captured while it ran. final is {page, frame_ref}: the terminal's page and the run's last frame. overlay {ref} is the run's task diagnostic stream, the JSON with every evaluated target's box and score per frame. Every ref is a materials ref for ac_material, which exports a large one whole. from_step (default 0) is the position of the first step answered, counted from 0; limit defaults to 20 (at most 100); next_step is present while steps remain after the page: pass it as from_step to read on. truncated true means the page was cut to the output budget and next_step continues after its last step. A run nobody has answers run_unknown.";

const ARGUMENTS: [&str; 4] = ["handle", "run_id", "from_step", "limit"];

pub(super) fn run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &ARGUMENTS)?;
    let run = match (arguments.string("handle")?, arguments.string("run_id")?) {
        (Some(handle), None) => RunKey::RequestId(
            run_evidence::parse_request_id(handle)
                .ok_or_else(|| invalid_argument("handle", ArgumentProblem::NotRecognised))?,
        ),
        (None, Some(run_id)) => RunKey::RunId(
            run_evidence::parse_run_id(run_id)
                .ok_or_else(|| invalid_argument("run_id", ArgumentProblem::NotRecognised))?,
        ),
        // Exactly one of handle and run_id.
        (Some(_), Some(_)) => {
            return Err(invalid_argument("run_id", ArgumentProblem::Conflicting));
        }
        (None, None) => return Err(invalid_argument("handle", ArgumentProblem::Required)),
    };
    let request = EvidenceRequest {
        run,
        from_step: arguments
            .integer("from_step", 0, run_evidence::MAX_FROM_STEP)?
            .unwrap_or(0),
        limit: arguments
            .integer("limit", 1, run_evidence::MAX_LIMIT)?
            .unwrap_or(run_evidence::DEFAULT_LIMIT),
    };
    let connected = context.runtime.connect()?;
    let evidence = run_evidence::read(&connected.client, &request)
        .map_err(|failure| tool_error(context, &connected, failure))?;
    Ok(within_budget(&evidence))
}

/// The page with as many of its steps, from the first, as the output budget lets through.
fn within_budget(evidence: &Evidence) -> ToolSuccess {
    let mut kept = evidence.page_len();
    loop {
        let result = evidence.render(kept);
        // A result that does not fit even without steps answers output_budget_exceeded.
        if kept == 0 || tools::fits_success(&result) {
            return ToolSuccess::new(result);
        }
        kept -= 1;
    }
}

/// The CLI's failure as a tool error: an argument is `arguments_invalid` here, with the CLI's
/// values; every other failure keeps the CLI's outcome.
fn tool_error(context: &ToolContext<'_>, connected: &Connected, failure: Failure) -> ToolError {
    match &failure {
        Failure::Argument {
            argument,
            range: Some((minimum, maximum)),
            ..
        } => tools::out_of_range(argument, *minimum, *maximum),
        Failure::Argument {
            argument, problem, ..
        } => invalid_argument(argument, *problem),
        Failure::RunUnknown { .. } => ToolError::outcome("usage", failure.outcome()),
        Failure::Output(_) => ToolError::outcome("runtime", failure.outcome()),
        Failure::Runtime(error) => context.runtime.failure(connected, error),
    }
}

pub(super) fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "handle": {
                "type": "string",
                "minLength": 1,
                "description": "The run's Runtime request_id (the handle ac_run_pack returns); or give run_id.",
            },
            "run_id": {
                "type": "string",
                "minLength": 1,
                "description": "The run's run_id; or give handle.",
            },
            "from_step": {
                "type": "integer",
                "minimum": 0,
                "maximum": run_evidence::MAX_FROM_STEP,
                "default": 0,
                "description": "The position of the first step answered, from 0 (next_step of the previous page).",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": run_evidence::MAX_LIMIT,
                "default": run_evidence::DEFAULT_LIMIT,
            },
        },
        "additionalProperties": false,
    })
}

pub(super) fn result_schema() -> Value {
    let text = json!({"type": "string"});
    let integer = json!({"type": "integer"});
    let reference = json!({
        "type": "object",
        "properties": {"ref": text},
        "required": ["ref"],
    });
    let rectangle = json!({
        "type": "object",
        "properties": {"x": integer, "y": integer, "width": integer, "height": integer},
        "required": ["x", "y", "width", "height"],
    });
    json!({
        "type": "object",
        "properties": {
            "request_id": text,
            "run_id": text,
            "task_id": text,
            "steps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "step_index": integer,
                        "page": text,
                        "targets": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "target": text,
                                    "matched": {"type": "boolean"},
                                    "box": rectangle,
                                },
                                "required": ["target", "matched"],
                            },
                        },
                        "action": {
                            "type": "object",
                            "properties": {
                                "kind": text,
                                "point": {
                                    "type": "object",
                                    "properties": {"x": integer, "y": integer},
                                    "required": ["x", "y"],
                                },
                                "rect": rectangle,
                            },
                            "required": ["kind"],
                        },
                        "frames": {"type": "array", "items": reference},
                    },
                    "required": ["step_index", "page", "targets", "frames"],
                },
            },
            "final": {
                "type": "object",
                "properties": {"page": text, "frame_ref": text},
            },
            "next_step": integer,
            "overlay": reference,
            "truncated": {"type": "boolean"},
        },
        "required": ["request_id", "steps", "final"],
    })
}

// Test-only: Workflow #389 MCP-6, the output budget of ac_run_evidence (command table §2.2).
#[cfg(test)]
mod tests {
    use super::*;

    // A run of 40 steps, each with one frame ref of about 700 bytes: far over the budget whole.
    fn evidence(from_step: usize, limit: usize) -> Evidence {
        let mut identity = Map::new();
        identity.insert(
            "request_id".to_owned(),
            json!("request_0123456789abcdef0123456789abcdef"),
        );
        let steps = (0..40_u32)
            .map(|index| {
                json!({
                    "step_index": index,
                    "page": "neutral/page",
                    "targets": [],
                    "frames": [{"ref": format!("{index}:{}", "f".repeat(700))}],
                })
            })
            .collect();
        Evidence {
            identity,
            steps,
            final_: json!({"page": "neutral/page"}),
            overlay: None,
            from_step,
            limit,
        }
    }

    #[test]
    fn a_page_over_the_output_budget_is_cut_to_whole_steps_with_truncated_and_next_step() {
        let whole = evidence(0, 20);
        let first = within_budget(&whole).result;
        let kept = first["steps"].as_array().map_or(0, Vec::len);
        // Command table §2.2: MCP cuts the page from its last step until it fits; what it keeps
        // is the page's first steps, unchanged, and it says so.
        assert!(
            kept > 0 && kept < 20,
            "{kept} steps kept of a page over the budget"
        );
        assert!(tools::fits_success(&first), "the cut page does not fit");
        assert_eq!(first["truncated"], json!(true));
        assert_eq!(first["steps"][0], whole.steps[0]);
        // §4.7: next_step continues after the last step kept, and the next page starts there.
        assert_eq!(first["next_step"], json!(kept));
        let second = within_budget(&evidence(kept, 20)).result;
        assert_eq!(second["steps"][0], whole.steps[kept]);

        // A page inside the budget is answered whole, without truncated.
        let small = within_budget(&evidence(0, 2)).result;
        assert_eq!(small["steps"].as_array().map(Vec::len), Some(2));
        assert!(small.get("truncated").is_none(), "{small}");
        assert_eq!(small["next_step"], json!(2));
    }
}
