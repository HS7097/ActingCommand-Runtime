// SPDX-License-Identifier: AGPL-3.0-only

//! Codex tasks as channels (Workflow #432 AGT-T2; `contracts/agent-forward.md`): one message through
//! the desktop's local IPC, as a turn's start or a steer of the turn under way. A channel needs only
//! the task's id; there is no credential.
//!
//! - [`client`]: the frames, the desktop's requests and the readiness order of one send;
//! - [`state`]: the strict parse of a task's snapshot;
//! - [`request`]: the submission and its one fallback, after the intent.

mod client;
mod request;
mod state;

pub(crate) use client::CodexForward;
