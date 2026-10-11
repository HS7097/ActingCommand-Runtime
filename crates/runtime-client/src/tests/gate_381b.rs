// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 B, test plan C-3 (F4.7, coordinator answer D3): the receipt wait of one
//! emulator control action. A restart is a Stop phase (at most 210 s) and then a Start phase (at
//! most 220 s), so its wait is 440 s with the IO margin; start and stop keep 230 s.

use crate::client::emulator_control_response_timeout;
use actingcommand_contract::EmulatorInstanceAction;
use std::time::Duration;

#[test]
fn gate_the_emulator_control_receipt_wait_is_440_s_for_a_restart_and_230_s_otherwise() {
    for (action, seconds) in [
        (EmulatorInstanceAction::Restart, 440),
        (EmulatorInstanceAction::Start, 230),
        (EmulatorInstanceAction::Stop, 230),
    ] {
        assert_eq!(
            emulator_control_response_timeout(action),
            Duration::from_secs(seconds),
            "{action:?}"
        );
    }
}
