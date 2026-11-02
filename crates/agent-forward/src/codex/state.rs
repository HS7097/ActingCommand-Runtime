// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/desktop.rs. Copyright (c) 2026 HS7097. MIT: see ../../LICENSE-codex-goal-lifecycle.txt.

//! A Codex task's state as its owner's snapshot shows it (Workflow #432; `contracts/agent-forward.md`).
//! The parse is strict: a snapshot of a shape this build does not know is a changed protocol, never a
//! guess.

use serde_json::Value;
use std::path::Path;

/// What one send needs to know about a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskState {
    /// The desktop client that owns the task, as a UUID in lower case.
    pub(super) owner: String,
    /// The task's working directory, an absolute path.
    pub(super) cwd: String,
    /// A turn is under way, so a message steers it; otherwise a message starts a turn.
    pub(super) running: bool,
    /// The task waits for an approval or the person's input.
    pub(super) waiting: bool,
}

/// Why a snapshot gives no state to send on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StateProblem {
    /// The owner is still loading the task: `resuming` or `needs_resume`.
    Loading(&'static str),
    /// A snapshot of a shape this build does not know.
    Shape(&'static str),
}

/// The state in an owner's `conversationState`. The rollout files the source also reads to confirm
/// an idle task are not read: a message does not need them.
pub(super) fn parse(state: &Value, owner: &str) -> Result<TaskState, StateProblem> {
    let requests = state["requests"]
        .as_array()
        .ok_or(StateProblem::Shape("the snapshot has no requests array"))?;
    let turns = state["turns"]
        .as_array()
        .ok_or(StateProblem::Shape("the snapshot has no turns array"))?;
    let waiting = state
        .get("threadGoalResumeConfirmation")
        .is_some_and(|value| !value.is_null())
        || requests.iter().any(|request| request["completed"] != true);
    match state["resumeState"].as_str() {
        Some("resumed") => {}
        Some("resuming") => return Err(StateProblem::Loading("resuming")),
        Some("needs_resume") => return Err(StateProblem::Loading("needs_resume")),
        _ => {
            return Err(StateProblem::Shape(
                "the snapshot has an unknown resume state",
            ));
        }
    }
    let mut running = match state["threadRuntimeStatus"]["type"].as_str() {
        Some("active") => true,
        Some("idle") => false,
        _ => {
            return Err(StateProblem::Shape(
                "the snapshot has an unknown runtime status",
            ));
        }
    };
    for turn in turns {
        match turn["status"].as_str() {
            Some("inProgress") => running = true,
            Some("completed" | "failed" | "interrupted") => {}
            _ => {
                return Err(StateProblem::Shape(
                    "the snapshot has an unknown turn status",
                ));
            }
        }
    }
    let cwd = state["cwd"]
        .as_str()
        .or_else(|| turns.last().and_then(|turn| turn["cwd"].as_str()))
        .ok_or(StateProblem::Shape("the snapshot has no working directory"))?;
    if !Path::new(cwd).is_absolute() {
        return Err(StateProblem::Shape(
            "the task's working directory is not absolute",
        ));
    }
    Ok(TaskState {
        owner: owner.to_owned(),
        cwd: cwd.to_owned(),
        running,
        waiting,
    })
}
