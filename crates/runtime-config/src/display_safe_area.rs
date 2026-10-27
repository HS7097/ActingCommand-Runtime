// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-4b-D (Alice's ruling 69 of 2026-10-26: 「只有安全区适配不做，但是需要留出来声明式的安全区
//! 接口，也就是说可以依照声明的文件直接切换安全区形态」, 「一份文件对应一个机型或多个机型，并且修改设置后实时生效」;
//! `contracts/display-adapter.md` section 9): the `display_safe_area` section, where the declared
//! safe areas live and which device model an instance declares.
//!
//! - `display_safe_area.directory` (default `display-safe-area`): the directory of the
//!   declaration files (`actingcommand.display-safe-area.v1`); a relative path resolves against
//!   the Runtime's state root. An empty path is refused (`out_of_range`).
//! - `display_safe_area.check_interval_ms` (default 1 000, `0..=3600000`): the least time between
//!   two reads of the directory; 0 reads it at every frame that needs the safe area.
//! - `display_safe_area.model` (default empty: none): the device model every instance declares
//!   unless it names its own.
//! - per instance, `instance.<instance id>.display_safe_area.model` (in the file the
//!   `display_safe_area.model` member of the `instances[]` entry, default the section's key): the
//!   device model the instance declares; empty declares none.
//!
//! Every key is settable: the Runtime reads them at every frame of a run whose game's display
//! declaration measures on the safe area, so an instruction takes effect from the next frame.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name.
pub const SECTION: &str = "display_safe_area";
/// The directory of the declaration files.
pub const DIRECTORY: &str = "display_safe_area.directory";
/// The least time between two reads of the directory, in milliseconds.
pub const CHECK_INTERVAL_MS: &str = "display_safe_area.check_interval_ms";
/// The device model an instance declares (also per instance); empty declares none.
pub const MODEL: &str = "display_safe_area.model";

/// The bounds of `check_interval_ms`: every frame to one hour. Schema, not a threshold.
const CHECK_INTERVAL_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: 3_600_000,
};

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 15 (contracts/component-interfaces.md): a Runtime
// before DA-4b-D refuses a file that names the section, because the file denies unknown members;
// a file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySafeAreaFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    check_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<Value>,
}

/// The `display_safe_area` member of an `instances[]` entry: the instance's own model only.
// Interface anchor `actingd-config` revision 15: a Runtime before DA-4b-D refuses an entry that
// names it, because the entry denies unknown members.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceDisplaySafeAreaFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<Value>,
}

/// One section-level key of the section: its default is the defaults file's.
const fn section_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Read at every frame that needs the safe area, so an instruction takes effect at the
        // next one.
        settable: Settable::Yes,
        reader: "DA-4b-D",
        since: "0.12.16",
    }
}

/// One row per section-level key of the section.
pub(crate) static ROWS: &[KeyRow] = &[
    section_row(DIRECTORY, ValueType::String, KeyRange::Any),
    section_row(
        CHECK_INTERVAL_MS,
        ValueType::DurationMs,
        CHECK_INTERVAL_RANGE,
    ),
    section_row(MODEL, ValueType::String, KeyRange::Any),
];

/// The per-instance row of the section, which `instance.rs` lists among its own: the instance's
/// model, default the section's.
pub(crate) const INSTANCE_ROW: KeyRow = KeyRow {
    name: KeyName::PerInstance(MODEL),
    file_path: FilePath::InstanceMember,
    value_type: ValueType::String,
    range: KeyRange::Any,
    default: DefaultSource::Section(MODEL),
    settable: Settable::Yes,
    reader: "DA-4b-D",
    since: "0.12.16",
};

/// The section's keys in effect for one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySafeAreaConfig {
    /// The directory of the declaration files, as configured (a relative path resolves against
    /// the state root).
    pub directory: String,
    /// The least time between two reads of the directory, in milliseconds; 0 reads it at every
    /// frame.
    pub check_interval_ms: u64,
    /// The device model the instance declares; `None` when the value in effect is empty.
    pub model: Option<String>,
    /// The key the model was read from: the instance's own, or the section's.
    pub model_key: String,
}

/// The section's keys in effect, checked whole: the directory a non-empty string, the interval
/// within its range, the model a string.
pub(crate) fn section(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<DisplaySafeAreaConfig, ConfigFailure> {
    parse(value, MODEL.to_owned())
}

/// The keys in effect for the instance whose registry id is `instance_id`: the model the
/// instance's own key (`instance.<instance_id>.display_safe_area.model`) when the configuration
/// carries it, otherwise the section's; checked whole.
pub(crate) fn instance(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<DisplaySafeAreaConfig, ConfigFailure> {
    let own = crate::table::instance_key(instance_id, MODEL);
    let model_key = if value(&own).is_some() {
        own
    } else {
        MODEL.to_owned()
    };
    parse(value, model_key)
}

fn parse(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    model_key: String,
) -> Result<DisplaySafeAreaConfig, ConfigFailure> {
    let directory = match value(DIRECTORY) {
        Some(FactScalar::String(directory)) if !directory.is_empty() => directory,
        Some(FactScalar::String(_)) => {
            return Err(ConfigFailure::invalid_value(
                DIRECTORY,
                ConfigValueProblem::OutOfRange,
            ));
        }
        _ => {
            return Err(ConfigFailure::invalid_value(
                DIRECTORY,
                ConfigValueProblem::WrongType,
            ));
        }
    };
    let invalid_interval = |problem| {
        ConfigFailure::invalid_value(CHECK_INTERVAL_MS, problem).with_range(CHECK_INTERVAL_RANGE)
    };
    let check_interval_ms = match value(CHECK_INTERVAL_MS) {
        Some(FactScalar::DurationMs(ms)) => ms,
        Some(FactScalar::Integer(number)) => {
            u64::try_from(number).map_err(|_| invalid_interval(ConfigValueProblem::OutOfRange))?
        }
        _ => return Err(invalid_interval(ConfigValueProblem::WrongType)),
    };
    let within = match CHECK_INTERVAL_RANGE {
        KeyRange::Integer { min, max } => {
            i64::try_from(check_interval_ms).is_ok_and(|signed| (min..=max).contains(&signed))
        }
        KeyRange::Any | KeyRange::Tokens(_) => true,
    };
    if !within {
        return Err(invalid_interval(ConfigValueProblem::OutOfRange));
    }
    let model = match value(&model_key) {
        Some(FactScalar::String(model)) => (!model.is_empty()).then_some(model),
        _ => {
            return Err(ConfigFailure::invalid_value(
                model_key,
                ConfigValueProblem::WrongType,
            ));
        }
    };
    Ok(DisplaySafeAreaConfig {
        directory,
        check_interval_ms,
        model,
        model_key,
    })
}
