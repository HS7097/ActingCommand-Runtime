// SPDX-License-Identifier: AGPL-3.0-only

//! A request bounded by a wait (Workflow #389 MCP-21; MCP-0 command table v1.1 §4.22), moved
//! from actingctl's `main.rs` (Workflow #434 LAB-1b): `emu start|stop|restart` bounds its control
//! with it ([`crate::emulators::control_within`]), and actingctl's `task-run` its run. A request
//! that has not ended when the wait ends goes on in the Runtime; [`expired`] is what it answers.

use crate::run_evidence;
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{CorrelationId, RequestId};
use serde_json::{Map, Value};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

/// How a wait ended.
pub enum Waited<T> {
    /// The request ended within the wait, with this.
    Done(T),
    /// The wait ended first.
    Expired,
}

/// Runs `request` on a thread of its own and waits for it at most `wait`. A request that has not
/// ended by then goes on in the Runtime: the Runtime serves it on its own connection thread
/// whether its client still waits or not, and this process stops waiting. A request thread that
/// panicked panics here too.
pub fn wait_at_most<T: Send + 'static>(
    wait: Duration,
    request: impl FnOnce() -> T + Send + 'static,
) -> Waited<T> {
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        // The receiver is gone only when the wait already ended.
        let _ = sender.send(request());
    });
    match receiver.recv_timeout(wait) {
        Ok(done) => Waited::Done(done),
        Err(RecvTimeoutError::Timeout) => Waited::Expired,
        Err(RecvTimeoutError::Disconnected) => match worker.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            // A thread that ended sent its result first, which `recv_timeout` takes before it
            // sees the channel closed.
            Ok(()) => unreachable!("a request thread ended without its result"),
        },
    }
}

/// D11 as verified: the answer of a wait that ended before its request did, `{request_id?,
/// correlation_id?, state: "in_progress"}`, with `command_wait_expired` (`operation`,
/// `request_id` when the caller knows it, `waited_ms`). The request goes on in the Runtime; the
/// line that prints it exits 1.
pub fn expired(
    operation: ContractLocation,
    wait: Duration,
    request_id: Option<RequestId>,
    correlation_id: Option<CorrelationId>,
) -> (Map<String, Value>, Outcome) {
    let mut members = Map::new();
    let mut outcome = Outcome::new(SharedCode::CommandWaitExpired)
        .operation(operation)
        .waited_ms(wait);
    if let Some(request_id) = request_id {
        let text = run_evidence::id_text(&request_id);
        outcome = outcome.request_id(text.as_str());
        members.insert("request_id".to_owned(), Value::String(text));
    }
    if let Some(correlation_id) = correlation_id {
        members.insert(
            "correlation_id".to_owned(),
            Value::String(run_evidence::id_text(&correlation_id)),
        );
    }
    members.insert("state".to_owned(), Value::String("in_progress".to_owned()));
    (members, outcome)
}
