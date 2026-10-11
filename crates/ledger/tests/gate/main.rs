// SPDX-License-Identifier: AGPL-3.0-only

//! The gate tests of the ledger that use only its public API (test plan v2.2 §2.1: one gate
//! test binary per crate, one module per work item).

use std::time::Duration;

/// The bound of every wait in these gate tests (test plan v2.2 §2.1, guard 3).
const GATE_WAIT: Duration = Duration::from_secs(120);

mod deferred_append;
mod durability;
