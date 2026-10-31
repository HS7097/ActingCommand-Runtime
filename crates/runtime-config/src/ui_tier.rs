// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-4 (A.6 rule 8; register §1.239; since 0.12.36): the console's three tiers of
//! the configuration keys, kept in the Runtime's key table so that the console hard-codes none of
//! them; the console reads them through the pinned crate, and no wire shape changes.
//!
//! - `daily`: the second tier's raw keys. The console shows the first four through its
//!   performance preset (`performance.presets`), not one by one.
//! - `advanced`: the third tier, at most [`MAX_ADVANCED_KEYS`] keys. A key that is not settable
//!   is shown read-only there; the configuration file changes it (the two Lab options through
//!   `actingctl install`).
//! - `hidden`: every other key, a key the key table does not have included.
//!
//! The table names keys only. A command is never a key: the diagnostics export is a command and is
//! not in it. Neither are configuration-file members the key table does not have (the `vision`
//! section, an instance's `adb_path`): actingd reads them at assembly and reports no
//! `config.parameters` row of them.

use crate::table::{KeyName, find_row};
use crate::{frame_retention, host, idle, instance, lab_module, performance, scheduling};

/// The tier of one configuration key in the console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiTier {
    /// Not shown.
    Hidden,
    /// The second tier (daily control).
    Daily,
    /// The third tier (advanced settings).
    Advanced,
}

impl UiTier {
    /// The tier's spelling in the contract table.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Daily => "daily",
            Self::Advanced => "advanced",
        }
    }
}

/// At most this many keys are `advanced` (A.6 rule 8).
pub const MAX_ADVANCED_KEYS: usize = 8;

/// The MuMu root actingd discovers the emulator from (an assembly key, `existing.rs`).
const MUMU_ROOT: &str = "mumu_root";

/// Every key of a tier other than `hidden`, by its key-table name: a per-instance name stands for
/// that member of every instance. The order is the contract table's.
pub static UI_TIERS: &[(KeyName, UiTier)] = &[
    // The four keys of the performance preset.
    (KeyName::Exact(performance::pacing::MODE), UiTier::Daily),
    (
        KeyName::Exact(performance::pacing::MAX_CONCURRENT_RUNS),
        UiTier::Daily,
    ),
    (
        KeyName::Exact(performance::pacing::HEAVY_DISPATCH_LIMIT_NORMAL),
        UiTier::Daily,
    ),
    (KeyName::Exact(scheduling::PARALLEL_ENABLED), UiTier::Daily),
    // The quiet times, the host's and each instance's own.
    (KeyName::Exact(scheduling::BLACKOUT_HOST), UiTier::Daily),
    (
        KeyName::PerInstance(scheduling::INSTANCE_BLACKOUT),
        UiTier::Daily,
    ),
    // Each instance's emulator: started by the daemon, and what an idle instance does.
    (
        KeyName::PerInstance(instance::START_EMULATOR),
        UiTier::Daily,
    ),
    (KeyName::PerInstance(idle::ACTION), UiTier::Daily),
    // The third tier.
    (KeyName::Exact(MUMU_ROOT), UiTier::Advanced),
    (
        KeyName::Exact(frame_retention::EVIDENCE_MAX_AGE_DAYS),
        UiTier::Advanced,
    ),
    (
        KeyName::Exact(host::PROCESS_MEMORY_PRIORITY),
        UiTier::Advanced,
    ),
    (KeyName::Exact(host::PROCESS_IO_PRIORITY), UiTier::Advanced),
    (KeyName::Exact(lab_module::AUTHORING), UiTier::Advanced),
    (KeyName::Exact(lab_module::DEBUGGING), UiTier::Advanced),
];

/// The console tier of `key`, as `config.parameters` names it (a per-instance key
/// `instance.<instance id>.<member>`): its key-table row's entry in [`UI_TIERS`], otherwise
/// `hidden`, and `hidden` for a key the key table does not have.
pub fn ui_tier(key: &str) -> UiTier {
    let Some((row, _)) = find_row(key) else {
        return UiTier::Hidden;
    };
    UI_TIERS
        .iter()
        .find(|(name, _)| *name == row.name)
        .map_or(UiTier::Hidden, |(_, tier)| *tier)
}
