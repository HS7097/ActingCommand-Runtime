// SPDX-License-Identifier: AGPL-3.0-only

//! The emulator operations (Workflow #434 LAB-1b; register §1.287), moved from actingctl's
//! `main.rs` and MCP's `ac_emulator`. Each sends its request on the connection it is given and
//! answers the Runtime's result unchanged:
//!
//! - `discover` (`actingctl emulator discover`): `DiscoverInstances`;
//! - `emu start|stop|restart` (`actingctl emulator start|stop|restart`, MCP `ac_emulator`):
//!   `ControlEmulatorInstance`; the CLI may bound its wait (Workflow #389 MCP-21);
//! - `emu selfcheck` (`actingctl selfcheck`, Workflow #317 sc3 (d)): `SelfCheckInstance`, the
//!   manual reconnect and self-check of one physical instance.

use crate::codes::ActingctlCode;
use crate::wait::{Waited, wait_at_most};
use crate::with_cause;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    CorrelationId, EmulatorInstanceAction, RuntimeInstanceDiscovery, RuntimeResult,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use serde_json::{Value, json};
use std::time::Duration;

/// The control a word names, as the CLI's action and MCP's `action` give it (`start`, `stop`,
/// `restart`); `None` for any other word.
pub fn action(word: &str) -> Option<EmulatorInstanceAction> {
    match word {
        "start" => Some(EmulatorInstanceAction::Start),
        "stop" => Some(EmulatorInstanceAction::Stop),
        "restart" => Some(EmulatorInstanceAction::Restart),
        _ => None,
    }
}

/// `discover`: the instances the Runtime discovers.
pub fn discover(client: &RuntimeClient) -> Result<RuntimeInstanceDiscovery, RuntimeClientError> {
    client.discover_instances()
}

/// `emu start|stop|restart`: one control of the emulator of the instance `alias`, its result as
/// the Runtime gives it.
pub fn control(
    client: &RuntimeClient,
    alias: &str,
    action: EmulatorInstanceAction,
) -> Result<RuntimeResult, RuntimeClientError> {
    client.control_emulator_instance(alias, action)
}

/// MCP's answer of a control: `{controlled}`.
pub fn controlled_answer(controlled: &RuntimeResult) -> Value {
    json!({"controlled": controlled})
}

/// What a control bounded by a wait ended with.
pub enum Bounded {
    /// The control ended within the wait, with the Runtime's result (boxed: a result is much
    /// larger than the expiry).
    Controlled(Box<RuntimeResult>),
    /// The wait ended first. The control goes on in the Runtime, named by the correlation of
    /// its interaction (the client mints the request id only as it writes it).
    Expired {
        correlation_id: Option<CorrelationId>,
    },
}

/// `emu start|stop|restart` with a wait (Workflow #389 MCP-21): the control on an interaction of
/// its own, whose correlation names the request before its receipt, waited for at most `wait`.
pub fn control_within(
    client: &RuntimeClient,
    alias: String,
    action: EmulatorInstanceAction,
    wait: Duration,
) -> Result<Bounded, RuntimeClientError> {
    let interaction = client.begin_interaction()?;
    let correlation_id = interaction.correlation_id();
    match wait_at_most(wait, move || {
        interaction.control_emulator_instance(&alias, action)
    }) {
        Waited::Done(result) => result.map(|controlled| Bounded::Controlled(Box::new(controlled))),
        Waited::Expired => Ok(Bounded::Expired { correlation_id }),
    }
}

/// `emu selfcheck`: reconnect and self-check the physical instance `alias` now, its result as the
/// Runtime gives it.
pub fn self_check(
    client: &RuntimeClient,
    alias: &str,
) -> Result<RuntimeResult, RuntimeClientError> {
    client.self_check_instance(alias)
}

/// A self-check that failed is no success: `instance_self_check_failed`, caused by the check's
/// failure code when it has one (the result is answered beside it); `None` for a check that
/// passed.
pub fn self_check_failure(result: &RuntimeResult) -> Option<Outcome> {
    match result {
        RuntimeResult::InstanceSelfChecked { selfcheck, .. }
            if selfcheck.failure_code.is_some() || !selfcheck.capture.ok || !selfcheck.touch.ok =>
        {
            let check_failed = Outcome::new(ActingctlCode::InstanceSelfCheckFailed);
            Some(match selfcheck.failure_code.as_deref() {
                Some(spelling) => with_cause(check_failed, Outcome::relayed(spelling)),
                None => check_failed,
            })
        }
        _ => None,
    }
}
