// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-7a2 (register §1.234, since 0.12.35; [contracts/player-hold.md], "The one
//! order of the control gates"): the host's side of the one order of the control gates (R-7a
//! §2.6). The contract crate writes the order ([`ControlGates::evaluate`]) and names a gate's
//! effective code in its vocabulary `control_gate_code`, whose words are the four runtime-host
//! codes spelled exactly; the host maps the word back to its code here and emits it. Every host
//! site that decides why a dispatch does not happen (the evaluation's position 1, the fact
//! refresh's holds, the policy admission) reads its inputs at one moment
//! (`SchedulingPauseTable::control_gates`) and takes the code from [`effective_code`]; none writes
//! an order of its own. The player hold's dimension stays false until R-7b.
//!
//! [contracts/player-hold.md]: ../../../contracts/player-hold.md

use crate::codes::HostCode;
use actingcommand_contract::{ControlGateCode, ControlGates};

/// The runtime-host code a control gate's effective code spells (R-7a §2.6 O-1). A rename on
/// either side fails the word-by-word test (`gate_429_r7a2`), so both change together.
pub(crate) const fn host_code(code: ControlGateCode) -> HostCode {
    match code {
        ControlGateCode::DispatchPlayerHeld => HostCode::DispatchPlayerHeld,
        ControlGateCode::DispatchPausedInstance => HostCode::DispatchPausedInstance,
        ControlGateCode::DispatchPausedGlobal => HostCode::DispatchPausedGlobal,
        ControlGateCode::DispatchResourceFenced => HostCode::DispatchResourceFenced,
    }
}

/// The effective code of `gates` as the host emits it: its first dispatch gate's (R-7a §2.6
/// O-1); `None` with no control gate in effect.
pub(crate) fn effective_code(gates: &ControlGates) -> Option<HostCode> {
    gates.code().map(host_code)
}
