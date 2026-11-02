// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/relay.rs. Copyright (c) 2026 HS7097. MIT: see ../../LICENSE-codex-goal-lifecycle.txt.

//! The one submission of a message to a Codex task's owner, and its one fallback (Workflow #432;
//! `contracts/agent-forward.md`). Everything here runs after the intent was committed, so nothing
//! answers `Deferred`: a write may have happened.

use super::client::{Failure, quoted, uuid_of};
use super::state::TaskState;
use crate::{DeliveryProblem, DeliveryProof, ForwardOutcome};
use serde_json::{Value, json};
use std::time::Instant;

/// How a message reaches the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// The task is idle: the message starts a turn.
    Start,
    /// A turn is under way: the message steers it.
    Steer,
}

impl Mode {
    fn word(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Steer => "steer",
        }
    }
}

/// One message, as the task's owner takes it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MessageRequest {
    pub(super) mode: Mode,
    pub(super) params: Value,
}

/// What a submission carries, the same for its fallback.
pub(super) struct Submission<'a> {
    /// The task, as a UUID in lower case.
    pub(super) task: &'a str,
    /// The message's id: the wake's, the same on every try and in the fallback.
    pub(super) message_id: &'a str,
    pub(super) text: &'a str,
    pub(super) created_at_unix_ms: u64,
}

/// What the owner's answer to a submission says.
enum Receipt {
    /// The owner returned a turn id.
    Accepted,
    /// The owner refused: a definite answer.
    Rejected(String),
    /// An answer of a shape this build does not know.
    Unsupported(&'static str),
}

impl Submission<'_> {
    /// The request for a task in this state: a steer while a turn runs, else a start.
    pub(super) fn request(&self, cwd: &str, running: bool) -> MessageRequest {
        let input = json!([{"type": "text", "text": self.text, "text_elements": []}]);
        if running {
            MessageRequest {
                mode: Mode::Steer,
                params: json!({
                    "conversationId": self.task,
                    "input": input,
                    "restoreMessage": {
                        "id": self.message_id,
                        "text": self.text,
                        "context": {
                            "prompt": self.text, "addedFiles": [], "fileAttachments": [],
                            "ideContext": null, "imageAttachments": [], "commentAttachments": [],
                            "workspaceRoots": [cwd]
                        },
                        "cwd": cwd,
                        "createdAt": self.created_at_unix_ms
                    },
                    "serviceTier": null, "attachments": [],
                    "clientUserMessageId": self.message_id, "additionalContext": null
                }),
            }
        } else {
            MessageRequest {
                mode: Mode::Start,
                params: json!({
                    "conversationId": self.task,
                    "turnStart": {
                        "request": {
                            "threadId": self.task, "clientUserMessageId": self.message_id,
                            "input": input, "cwd": cwd, "additionalContext": null
                        },
                        "context": {
                            "inheritThreadSettings": true, "usePermissionSelection": false,
                            "mcpAppModelContextAttachments": []
                        }
                    }
                }),
            }
        }
    }
}

impl MessageRequest {
    /// The request's method and version.
    pub(super) fn method(&self) -> (&'static str, u64) {
        match self.mode {
            Mode::Start => ("thread-follower-start-turn", 2),
            Mode::Steer => ("thread-follower-steer-turn", 1),
        }
    }

    fn receipt(&self, response: &Value, task: &str) -> Receipt {
        if response["resultType"] == "error" {
            return Receipt::Rejected(quoted(&response["error"], task));
        }
        if response["resultType"] != "success" {
            return Receipt::Unsupported("the owner's answer to the submission has another shape");
        }
        let pointer = match self.mode {
            Mode::Start => "/result/result/turn/id",
            Mode::Steer => "/result/result/turnId",
        };
        match response
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(uuid_of)
        {
            Some(turn) if turn.bytes().any(|byte| !matches!(byte, b'0' | b'-')) => {
                Receipt::Accepted
            }
            Some(_) => Receipt::Unsupported("the owner's acknowledgement has a nil turn id"),
            None => Receipt::Unsupported("the owner's acknowledgement has no turn id"),
        }
    }
}

/// What carries a request to the task's owner and reads the task again: the desktop's client, or a
/// test's script.
pub(super) trait Submitter {
    /// The task's state now, through a new subscription.
    fn task_state(&mut self, task: &str, deadline: Instant) -> Result<TaskState, Failure>;
    /// One request to the owner `owner`, and its answer.
    fn submit(
        &mut self,
        request: &MessageRequest,
        owner: &str,
        deadline: Instant,
    ) -> Result<Value, Failure>;
}

/// The owner's definite refusal of a steer because the turn it aimed at has just ended.
fn ended_turn_rejection(request: &MessageRequest, response: &Value, task: &str) -> bool {
    request.mode == Mode::Steer
        && response["resultType"] == "error"
        && response["error"].as_str()
            == Some(
                format!("Cannot steer conversation {task} because its active turn already ended")
                    .as_str(),
            )
}

/// Submits `first` once. Only the owner's definite refusal of a steer because its turn ended gets a
/// second submission: a start with the same message id, once the same owner shows the task idle in
/// the same directory. Every other refusal fails, and an answer that never came or cannot be read is
/// unknown. Nothing is tried a third time.
pub(super) fn deliver_prepared(
    submitter: &mut dyn Submitter,
    submission: &Submission<'_>,
    current: &TaskState,
    first: MessageRequest,
    deadline: Instant,
) -> ForwardOutcome {
    let task = submission.task;
    let mut request = first;
    let mut response = submitter.submit(&request, &current.owner, deadline);
    let mut recovered = None;
    if let Ok(value) = &response
        && ended_turn_rejection(&request, value, task)
    {
        let original = quoted(&value["error"], task);
        let fresh = match submitter.task_state(task, deadline) {
            Ok(fresh)
                if fresh.owner == current.owner
                    && fresh.cwd == current.cwd
                    && !fresh.running
                    && !fresh.waiting =>
            {
                fresh
            }
            Ok(_) => {
                return ForwardOutcome::Failed {
                    problem: DeliveryProblem::Rejected,
                    detail: format!(
                        "the owner refused the steer ({original}); no second submission: the task \
                         is not idle under the same owner in the same directory"
                    ),
                };
            }
            Err(failure) => {
                return ForwardOutcome::Failed {
                    problem: DeliveryProblem::Rejected,
                    detail: format!(
                        "the owner refused the steer ({original}); no second submission: the task \
                         could not be read again: {}",
                        failure.text()
                    ),
                };
            }
        };
        request = submission.request(&fresh.cwd, false);
        recovered = Some(original);
        response = submitter.submit(&request, &current.owner, deadline);
    }
    let before = match &recovered {
        Some(original) => format!(
            "the owner refused the steer ({original}) and one start with the same message id \
             followed: "
        ),
        None => String::new(),
    };
    let mode = request.mode.word();
    let value = match response {
        Ok(value) => value,
        Err(failure) => {
            return ForwardOutcome::Unknown {
                problem: failure.after_intent(),
                detail: format!(
                    "{before}no answer to the {mode} submission: {}; it may have reached the task",
                    failure.text()
                ),
            };
        }
    };
    match request.receipt(&value, task) {
        Receipt::Accepted => ForwardOutcome::Delivered {
            proof: DeliveryProof::Accepted,
            detail: format!(
                "{before}the task's owner accepted the {mode} submission and returned a turn id; \
                 this does not show the turn finished or acted"
            ),
        },
        Receipt::Rejected(text) => ForwardOutcome::Failed {
            problem: DeliveryProblem::Rejected,
            detail: format!("{before}the task's owner refused the {mode} submission: {text}"),
        },
        Receipt::Unsupported(what) => ForwardOutcome::Unknown {
            problem: DeliveryProblem::ProtocolChanged,
            detail: format!("{before}{what}; the {mode} submission may have reached the task"),
        },
    }
}
