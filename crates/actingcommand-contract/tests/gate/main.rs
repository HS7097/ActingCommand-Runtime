// SPDX-License-Identifier: AGPL-3.0-only

//! The gate tests of actingcommand-contract (test plan §2.1: one gate test binary per crate,
//! one module per work item). They use the crate's public API only.

use actingcommand_contract::outcome::{Outcome, install_diagnostic_sink};
use std::sync::Once;

mod a2a;
mod c1;
mod lab_l3;
mod rd1a;

/// Installs this binary's one outcome diagnostic sink, which drops the text. Cutting an
/// envelope sends the cut text to the sink, and a debug build panics when none is installed.
fn diagnostic_sink() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        install_diagnostic_sink(Box::new(|_: &Outcome, _: &str| {}))
            .expect("install the gate binary's diagnostic sink");
    });
}
