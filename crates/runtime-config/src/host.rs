// SPDX-License-Identifier: AGPL-3.0-only

//! The `host` section (RB-2, Alice's ruling of 2026-10-29 10:2x, 「设回正常」): the memory
//! priority and the I/O priority actingd gives its own process at start.
//!
//! - `process_memory_priority`: `normal` (the default) or `low`;
//! - `process_io_priority`: `normal` (the default) or `low`.
//!
//! A process the watchdog's scheduled task starts inherits the task's memory priority 2 and I/O
//! priority Low, so the operating system evicts the pages it reads first (the 2026-10-29 05:23
//! reboot: the ledger's pages never stayed cached and its reads went to disk for hours).
//! actingd applies the two keys to itself right after it read its configuration, before the
//! ledger opens; the CPU priority class is not one of them and stays as the process was
//! started. So an instruction may not change either key (it would change nothing already
//! applied); the configuration file does, at the next start. Both defaults live in the defaults
//! file. A value that is no string is `config_value_invalid` (`wrong_type`), a string other than
//! the two words `config_value_invalid` (`out_of_range`), each naming its key.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "host";
/// The memory priority actingd gives its own process at start.
pub const PROCESS_MEMORY_PRIORITY: &str = "host.process_memory_priority";
/// The I/O priority actingd gives its own process at start.
pub const PROCESS_IO_PRIORITY: &str = "host.process_io_priority";

/// The words a priority key allows, in [`ProcessPriorityLevel`] order. Schema.
pub const PRIORITY_TOKENS: &[&str] = &["normal", "low"];
/// The range of both keys. Schema.
pub const PRIORITY_RANGE: KeyRange = KeyRange::Tokens(PRIORITY_TOKENS);

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 20 (contracts/component-interfaces.md): a Runtime
// before RB-2 refuses a file that names the section, because the file denies unknown members; a
// file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_memory_priority: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_io_priority: Option<Value>,
}

const fn row(name: &'static str) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type: ValueType::String,
        range: PRIORITY_RANGE,
        default: DefaultSource::File,
        // Applied by actingd to its own process before the ledger opens, where no instruction
        // value is known yet.
        settable: Settable::No(NotSettable::BeforeLedgerOpen),
        reader: "RB-2",
        since: "0.12.28",
    }
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[row(PROCESS_MEMORY_PRIORITY), row(PROCESS_IO_PRIORITY)];

/// The level a priority key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessPriorityLevel {
    /// The operating system's normal level for a process.
    Normal,
    /// A lower level: the level a process of a below-normal scheduled task gets.
    Low,
}

impl ProcessPriorityLevel {
    /// The key's word for the level.
    pub const fn token(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
}

/// The `host` section in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostProcessConfig {
    /// `host.process_memory_priority`.
    pub memory_priority: ProcessPriorityLevel,
    /// `host.process_io_priority`.
    pub io_priority: ProcessPriorityLevel,
}

/// The level `key` holds: a string that is one of [`PRIORITY_TOKENS`]; `config_value_invalid`
/// for `key` otherwise, `wrong_type` for a value that is no string and `out_of_range` for
/// another word.
fn level(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key: &str,
) -> Result<ProcessPriorityLevel, ConfigFailure> {
    let invalid = |problem| ConfigFailure::invalid_value(key, problem).with_range(PRIORITY_RANGE);
    match value(key) {
        Some(FactScalar::String(text)) if text == ProcessPriorityLevel::Normal.token() => {
            Ok(ProcessPriorityLevel::Normal)
        }
        Some(FactScalar::String(text)) if text == ProcessPriorityLevel::Low.token() => {
            Ok(ProcessPriorityLevel::Low)
        }
        Some(FactScalar::String(_)) => Err(invalid(ConfigValueProblem::OutOfRange)),
        _ => Err(invalid(ConfigValueProblem::WrongType)),
    }
}

impl HostProcessConfig {
    /// The section from `value` (the configuration in effect); `config_value_invalid` for the
    /// first key that is not one of the two words.
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        Ok(Self {
            memory_priority: level(value, PROCESS_MEMORY_PRIORITY)?,
            io_priority: level(value, PROCESS_IO_PRIORITY)?,
        })
    }
}
