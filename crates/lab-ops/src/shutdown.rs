// SPDX-License-Identifier: AGPL-3.0-only

//! `shutdown` (Workflow #434 LAB-1b; register §1.287), moved from actingctl's `request-shutdown`:
//! `RequestShutdown`, answered by its receipt, and with a wait the observation that the accepted
//! owner closed its owner journal record and that its process exited
//! ([`crate::shutdown_wait`]).

use crate::shutdown_wait;
use actingcommand_contract::RuntimeReceipt;
use actingcommand_contract::outcome::Outcome;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// `shutdown`: the Runtime asked to shut down; its receipt.
pub fn request(client: &RuntimeClient) -> Result<RuntimeReceipt, RuntimeClientError> {
    client.request_shutdown()
}

/// `shutdown` with a wait, after [`request`] was accepted on `client`: `{state: "completed",
/// closed_at_unix_ms, elapsed_ms}` once the accepted owner of `state_root` closed and exited
/// within `wait`, else the wait's failure (`shutdown_wait_timeout`,
/// `shutdown_wait_journal_unreadable`). The connection is closed first, so the waiting client
/// leaves nothing for the Runtime to drain.
pub fn wait(client: RuntimeClient, state_root: &Path, wait: Duration) -> Result<Value, Outcome> {
    // `request_shutdown` verified that the accepted receipt names exactly this target.
    let target = client.runtime_info().shutdown_target();
    drop(client);
    shutdown_wait::wait_for_shutdown(state_root, &target, wait)
}
