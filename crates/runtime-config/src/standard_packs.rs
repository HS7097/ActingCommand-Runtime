// SPDX-License-Identifier: AGPL-3.0-only

//! The `standard_packs` section (Workflow #373 NAV-6, model M-373 v1.1 §4.3): `root`, the
//! directory that holds one root per game, `<root>/<game>/`, written once by the installer. A
//! relative path resolves against the configuration file's directory (actingd's assembly); an
//! empty one is refused. Without the section no standard-pack root is read. Workflow #387 D1c
//! moves the section here from actingd's own configuration (NAV-6 deviation 5): a key the
//! start-up assembly reads once, reported in `config.parameters.standard_packs` as written.
//!
//! ROUT-L RL-1b (frozen model v1.0 §3.8, R12 「绝不硬编码」): the scheduler workbench's two keys,
//! each read at every check of the workbench and changeable while the Runtime runs:
//! `check_interval_ms` (`1000..=3600000`, default 60 000), the least time between two checks of
//! the games in use, and `cached_generations` (`0..=8`, default 1), how many composed generations
//! older than the active one stay in memory.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The directory of the installed standard packages' game roots, as the file writes it.
pub const ROOT: &str = "standard_packs.root";
/// ROUT-L RL-1b: the least time between two checks of the games in use, in milliseconds.
pub const CHECK_INTERVAL_MS: &str = "standard_packs.check_interval_ms";
/// ROUT-L RL-1b: the composed generations older than the active one kept in memory.
pub const CACHED_GENERATIONS: &str = "standard_packs.cached_generations";

/// The section as the configuration file writes it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StandardPacksFile {
    pub root: PathBuf,
    /// ROUT-L RL-1b; absent means the defaults file's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_interval_ms: Option<u64>,
    /// ROUT-L RL-1b; absent means the defaults file's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_generations: Option<u64>,
}

/// ROUT-L RL-1b: the section's member of the defaults file, which names no root.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardPacksDefaultsFile {
    #[serde(default)]
    pub check_interval_ms: Option<u64>,
    #[serde(default)]
    pub cached_generations: Option<u64>,
}

/// ROUT-L RL-1b: the scheduler workbench's keys in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardPackWorkbenchConfig {
    pub check_interval_ms: u64,
    pub cached_generations: usize,
}

const CHECK_INTERVAL: KeyRange = KeyRange::Integer {
    min: 1_000,
    max: 3_600_000,
};
/// At most 8 older generations, the bound the memo had before the key (model §3.8).
const CACHED: KeyRange = KeyRange::Integer { min: 0, max: 8 };

pub(crate) static ROWS: &[KeyRow] = &[
    KeyRow {
        name: KeyName::Exact(ROOT),
        file_path: FilePath::Key,
        value_type: ValueType::String,
        range: KeyRange::Any,
        // No default: without the key no root is read.
        default: DefaultSource::None,
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "NAV-6",
        since: "0.12.0",
    },
    KeyRow {
        name: KeyName::Exact(CHECK_INTERVAL_MS),
        file_path: FilePath::Key,
        value_type: ValueType::DurationMs,
        range: CHECK_INTERVAL,
        default: DefaultSource::File,
        settable: Settable::Yes,
        reader: "RL-1b",
        since: "0.12.13",
    },
    KeyRow {
        name: KeyName::Exact(CACHED_GENERATIONS),
        file_path: FilePath::Key,
        value_type: ValueType::Integer,
        range: CACHED,
        default: DefaultSource::File,
        settable: Settable::Yes,
        reader: "RL-1b",
        since: "0.12.13",
    },
];

/// ROUT-L RL-1b: the workbench's keys in effect, read from `value`.
pub(crate) fn workbench(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<StandardPackWorkbenchConfig, ConfigFailure> {
    let check_interval_ms = match value(CHECK_INTERVAL_MS) {
        Some(FactScalar::DurationMs(ms)) => ms,
        _ => {
            return Err(ConfigFailure::invalid_value(
                CHECK_INTERVAL_MS,
                ConfigValueProblem::WrongType,
            ));
        }
    };
    let cached_generations = match value(CACHED_GENERATIONS) {
        Some(FactScalar::Integer(count)) => usize::try_from(count).map_err(|_| {
            ConfigFailure::invalid_value(CACHED_GENERATIONS, ConfigValueProblem::OutOfRange)
        })?,
        _ => {
            return Err(ConfigFailure::invalid_value(
                CACHED_GENERATIONS,
                ConfigValueProblem::WrongType,
            ));
        }
    };
    Ok(StandardPackWorkbenchConfig {
        check_interval_ms,
        cached_generations,
    })
}
