// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl ladder-drill --instance <selector> [--device]` and MCP `ac_ladder_drill`
//! (Workflow #390 DRILL-1, forced compromise T-08): a person's drill of one instance's
//! stuck-recovery ladder, an option-two command. Both sides resolve the selector (alias,
//! instance_id or ADB port) with one status read, send one `DrillRecoveryLadder` and answer the
//! same object, the drill the Runtime admitted, `{instance_alias, mode, ladder_request_id}`: the
//! CLI prints it as its line beside the outcome `recovery_ladder_drill_queued`, MCP as its result.
//! Without `--device` (`device` false) the drill is a plan and never acts on the device; its
//! records, tagged `drill`, are read with `events --request-id <ladder_request_id>`.

use crate::instance_selector::{self, SelectorMiss};
use actingcommand_contract::{RecoveryDrillMode, RecoveryLadderDrill};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};

/// The mode `--device` (`device`) asks for: a plan unless the request says to act on the device.
pub(crate) const fn mode(device: bool) -> RecoveryDrillMode {
    if device {
        RecoveryDrillMode::Device
    } else {
        RecoveryDrillMode::Plan
    }
}

/// Why a drill was not admitted.
pub(crate) enum Failure {
    /// The selector names no instance of the Runtime's status, or more than one.
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    /// The Runtime refused the drill (its receipt's outcome on top), or did not answer.
    Runtime(RuntimeClientError),
}

/// Resolves `selector` with one status read and asks the Runtime for a drill of that instance's
/// ladder in `mode`.
pub(crate) fn drill(
    client: &RuntimeClient,
    selector: &str,
    mode: RecoveryDrillMode,
) -> Result<RecoveryLadderDrill, Failure> {
    let status = client.status().map_err(Failure::Runtime)?;
    let alias = instance_selector::select(status.instances(), selector)
        .map_err(|miss| Failure::Selector {
            miss,
            selector: selector.to_owned(),
        })?
        .instance_alias()
        .to_owned();
    client
        .drill_recovery_ladder(&alias, mode)
        .map_err(Failure::Runtime)
}
