// SPDX-License-Identifier: AGPL-3.0-only

//! The gate tests of actingcommand-contract (test plan §2.1: one gate test binary per crate,
//! one module per work item). They use the crate's public API only.

use actingcommand_contract::outcome::{Outcome, install_diagnostic_sink};
use std::sync::Once;

// #411 B5, slice BT-B5 (contract key K37): a stage's end pages in the static walk.
mod gate_411_bt_b5;

mod a2a;
mod c1;
mod lab_l3;
mod rd1a;

// #412 Lab B-3 S0 (contract keys K39 to K41): the LR-10 records of a declared Lab operation.
mod gate_412_b3_s0;

// #411 B16 part 1, slice BT-KEY (contract key K47): a keyed cap in the static walk.
mod gate_411_bt_key;

/// Installs this binary's one outcome diagnostic sink, which drops the text. Cutting an
/// envelope sends the cut text to the sink, and a debug build panics when none is installed.
fn diagnostic_sink() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        install_diagnostic_sink(Box::new(|_: &Outcome, _: &str| {}))
            .expect("install the gate binary's diagnostic sink");
    });
}
