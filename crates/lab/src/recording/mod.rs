// SPDX-License-Identifier: AGPL-3.0-only

//! Lab recording core (Workflow #336): the recording state kept in the Lab state directory
//! next to the `record` command file, the five mark families with their mark-time
//! self-test, steps by serial index with transitions and remediation, the `--record`
//! frame and click hooks, and the per-instance recording lock (R20). The ActingLab CLI only
//! parses arguments and prints these results; nothing here touches the ledger.

pub(crate) mod frames;
pub(crate) mod lock;
pub(crate) mod marks;
pub(crate) mod model;
pub(crate) mod steps;
pub(crate) mod store;

pub use lock::RecordingLock;
pub use model::*;
