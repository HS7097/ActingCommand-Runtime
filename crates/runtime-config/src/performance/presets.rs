// SPDX-License-Identifier: AGPL-3.0-only

//! `performance.presets` (Workflow #429 R-2, design `429-A-runtime.md` A.1 契约 2; since
//! 0.12.31): the performance presets a person switches between, which the Runtime defines and the
//! three counters share (CLI `actingctl perf-preset`, MCP `ac_perf_preset`, the console).
//!
//! - **The key.** An object, preset name → `{mode, max_concurrent_runs?,
//!   heavy_dispatch_limit_normal, parallel_enabled}`; its default is in the defaults file
//!   (`standard`, `adaptive`, `light`); read once at assembly, not settable.
//! - **The mapping table** [`MEMBERS`]: the configuration key each member sets.
//! - **Applying a preset** is one `ConfigureSettings` of [`Preset::changes`]: every member at
//!   once, all changes or none, so `host.config_overrides` is recorded once. A member the preset
//!   leaves unwritten is a change without a value: it clears an online override of its key and
//!   never writes a value.
//! - **The current preset** ([`current_preset`]): the name of the preset whose every member the
//!   values in effect equal, else [`CUSTOM`]. A key without a value in effect (an unwritten
//!   `max_concurrent_runs`) compares as unwritten. No `config.parameters` row is derived from it.
//!
//! The wire names are the preset names and `custom`; the labels a person reads are the console's
//! (`contracts/runtime-config.md`, "The performance section").

use super::pacing::{FULL, HEAVY_DISPATCH_LIMIT_NORMAL, MAX_CONCURRENT_RUNS, MODE};
use super::{AT_START, Values, failure, row, rule};
use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::scheduling::PARALLEL_ENABLED;
use crate::table::{KeyRange, KeyRow, ValueType, find_row};
use crate::value::parse_value;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The key of the preset table.
pub const PRESETS: &str = "performance.presets";
/// The current preset's name when no preset's members equal the values in effect.
pub const CUSTOM: &str = "custom";
/// The longest preset name, in bytes.
const MAX_NAME_BYTES: usize = 64;

// Interface anchor `actingd-config` revision 22 (contracts/component-interfaces.md): a Runtime
// before Workflow #429 R-2 refuses a file that names the key, because the section denies unknown
// members; a file that does not name it keeps its revision.
pub(crate) static ROWS: &[KeyRow] = &[KeyRow {
    since: "0.12.31",
    ..row(PRESETS, ValueType::Object, KeyRange::Any, AT_START, "R-2")
}];

/// One member of a preset and the configuration key it sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetMember {
    /// The member's name in a preset.
    pub member: &'static str,
    /// The configuration key the member sets.
    pub key: &'static str,
    /// Whether a preset may leave the member unwritten; applying the preset then clears the
    /// key's online override.
    pub optional: bool,
}

/// The mapping table: each member of a preset and the key it sets, in the order a preset's
/// changes and a current-preset answer list them.
pub const MEMBERS: [PresetMember; 4] = [
    PresetMember {
        member: "mode",
        key: MODE,
        optional: false,
    },
    PresetMember {
        member: "max_concurrent_runs",
        key: MAX_CONCURRENT_RUNS,
        optional: true,
    },
    PresetMember {
        member: "heavy_dispatch_limit_normal",
        key: HEAVY_DISPATCH_LIMIT_NORMAL,
        optional: false,
    },
    PresetMember {
        member: "parallel_enabled",
        key: PARALLEL_ENABLED,
        optional: false,
    },
];

/// One preset: its name and each member's value as `config.parameters` reports its key, in
/// [`MEMBERS`] order; `None` for an unwritten optional member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    name: String,
    values: [Option<FactScalar>; 4],
}

impl Preset {
    /// The preset's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The value the preset gives `key`; `None` for a key it leaves unwritten or does not set.
    pub fn value(&self, key: &str) -> Option<&FactScalar> {
        MEMBERS
            .iter()
            .position(|member| member.key == key)
            .and_then(|at| self.values[at].as_ref())
    }

    /// The changes of the one `ConfigureSettings` that applies the preset, one per member in
    /// [`MEMBERS`] order: the member's value, or no value for a member the preset leaves
    /// unwritten (which clears that key's online override).
    pub fn changes(&self) -> Vec<ConfigSettingChange> {
        MEMBERS
            .iter()
            .zip(&self.values)
            .map(|(member, value)| ConfigSettingChange {
                key: member.key.to_owned(),
                value: value.as_ref().map(scalar_value),
            })
            .collect()
    }
}

/// The preset table of `performance.presets`, in name order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetTable {
    presets: Vec<Preset>,
}

impl PresetTable {
    /// Reads and checks the table (the whole-section check, at start and in `check-config`;
    /// `perf_config_invalid` naming [`PRESETS`]):
    /// - the value is an object; each member a preset whose name is 1 to 64 bytes of `a-z`, `0-9`
    ///   and `_` and not [`CUSTOM`] (`out_of_range`);
    /// - a preset is an object of exactly `mode`, `heavy_dispatch_limit_normal`,
    ///   `parallel_enabled` and, optionally, `max_concurrent_runs` (`wrong_type`);
    /// - each member is a value its key takes (`wrong_type`, `out_of_range`), and `mode` is not
    ///   `full`, which no instruction sets (`out_of_range`);
    /// - no two presets have the same members, so the current preset is one (`section_check`).
    pub fn parse(value: &Value) -> Result<Self, ConfigFailure> {
        let table = value
            .as_object()
            .ok_or_else(|| failure(PRESETS, ConfigValueProblem::WrongType))?;
        let mut presets = Vec::with_capacity(table.len());
        for (name, preset) in table {
            if !valid_name(name) {
                return Err(failure(PRESETS, ConfigValueProblem::OutOfRange));
            }
            let preset = Preset {
                name: name.clone(),
                values: preset_values(preset)?,
            };
            rule(
                PRESETS,
                presets
                    .iter()
                    .all(|other: &Preset| other.values != preset.values),
            )?;
            presets.push(preset);
        }
        presets.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(Self { presets })
    }

    /// The presets, in name order.
    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    /// The preset named `name`.
    pub fn get(&self, name: &str) -> Option<&Preset> {
        self.presets.iter().find(|preset| preset.name == name)
    }
}

/// A member's value as the current preset reads it: the key, its value in effect (`None` for a
/// key without one) and where that value comes from (`default` for a key without one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentMember {
    pub key: &'static str,
    pub value: Option<FactScalar>,
    pub source: ConfigParameterSource,
}

/// The current preset and the members it was judged by, in [`MEMBERS`] order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentPreset {
    /// A preset's name, or [`CUSTOM`].
    pub preset: String,
    pub members: Vec<CurrentMember>,
}

impl CurrentPreset {
    /// The answer of `actingctl perf-preset get` and `ac_perf_preset` `get`: `{preset, members:
    /// [{key, value, source}]}`, each value in its key's JSON (`null` for a key without one).
    pub fn answer(&self) -> Value {
        let members = self
            .members
            .iter()
            .map(|member| {
                json!({
                    "key": member.key,
                    "value": member.value.as_ref().map_or(Value::Null, scalar_value),
                    "source": member.source.as_str(),
                })
            })
            .collect::<Vec<_>>();
        json!({ "preset": self.preset, "members": members })
    }
}

/// The preset table in effect among `parameters` (a manifest's, or the rows of
/// `config.parameters.performance`): `None` when they have no [`PRESETS`] row; the row's value
/// read and checked otherwise.
pub fn table_of(parameters: &[ConfigParameter]) -> Option<Result<PresetTable, ConfigFailure>> {
    let parameter = parameters
        .iter()
        .find(|parameter| parameter.key == PRESETS)?;
    Some(match &parameter.value {
        FactScalar::String(text) => serde_json::from_str::<Value>(text)
            .map_err(|_| failure(PRESETS, ConfigValueProblem::WrongType))
            .and_then(|value| PresetTable::parse(&value)),
        _ => Err(failure(PRESETS, ConfigValueProblem::WrongType)),
    })
}

/// The current preset (Workflow #429 R-2): the first preset of `table`, in name order, whose
/// every member equals the value in effect of its key among `parameters`, else [`CUSTOM`]. A key
/// without a row has no value in effect and equals only an unwritten member. Pure: the CLI, MCP
/// and the console call it with the rows of `config.parameters.performance` and
/// `config.parameters.scheduling` (or a manifest's parameters).
pub fn current_preset(table: &PresetTable, parameters: &[ConfigParameter]) -> CurrentPreset {
    let members = MEMBERS
        .iter()
        .map(|member| {
            let parameter = parameters
                .iter()
                .find(|parameter| parameter.key == member.key);
            CurrentMember {
                key: member.key,
                value: parameter.map(|parameter| parameter.value.clone()),
                source: parameter
                    .map_or(ConfigParameterSource::Default, |parameter| parameter.source),
            }
        })
        .collect::<Vec<_>>();
    let preset = table
        .presets
        .iter()
        .find(|preset| {
            preset
                .values
                .iter()
                .zip(&members)
                .all(|(value, member)| *value == member.value)
        })
        .map_or(CUSTOM, Preset::name)
        .to_owned();
    CurrentPreset { preset, members }
}

/// One `config.parameters.<section>` row `{key, value, source, settable}` as a parameter;
/// `None` for a row of another shape.
pub fn parameter_of_row(row: &BTreeMap<String, FactScalar>) -> Option<ConfigParameter> {
    let FactScalar::String(key) = row.get("key")? else {
        return None;
    };
    let FactScalar::String(source) = row.get("source")? else {
        return None;
    };
    let FactScalar::Boolean(settable) = row.get("settable")? else {
        return None;
    };
    let source = [
        ConfigParameterSource::Explicit,
        ConfigParameterSource::Default,
        ConfigParameterSource::Discovered,
        ConfigParameterSource::Instruction,
    ]
    .into_iter()
    .find(|known| known.as_str() == source.as_str())?;
    Some(ConfigParameter {
        key: key.clone(),
        value: row.get("value")?.clone(),
        source,
        settable: *settable,
    })
}

/// The whole-section check's part of the table (`perf_config_invalid` naming [`PRESETS`]).
pub(super) fn check(values: &Values<'_>) -> Result<(), ConfigFailure> {
    PresetTable::parse(&values.json(PRESETS)?).map(|_| ())
}

/// A preset name: 1 to [`MAX_NAME_BYTES`] bytes of `a-z`, `0-9` and `_`, not [`CUSTOM`].
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && name != CUSTOM
}

/// A preset's members as their keys' scalars, in [`MEMBERS`] order.
fn preset_values(preset: &Value) -> Result<[Option<FactScalar>; 4], ConfigFailure> {
    let members: &Map<String, Value> = preset
        .as_object()
        .ok_or_else(|| failure(PRESETS, ConfigValueProblem::WrongType))?;
    if members
        .keys()
        .any(|name| !MEMBERS.iter().any(|member| member.member == name.as_str()))
    {
        return Err(failure(PRESETS, ConfigValueProblem::WrongType));
    }
    let mut values: [Option<FactScalar>; 4] = Default::default();
    for (slot, member) in values.iter_mut().zip(MEMBERS) {
        let Some(value) = members.get(member.member).filter(|value| !value.is_null()) else {
            if member.optional {
                continue;
            }
            return Err(failure(PRESETS, ConfigValueProblem::WrongType));
        };
        let (row, _) =
            find_row(member.key).ok_or_else(|| failure(PRESETS, ConfigValueProblem::WrongType))?;
        let scalar = parse_value(row, member.key, value).map_err(|refused| {
            failure(
                PRESETS,
                refused.problem().unwrap_or(ConfigValueProblem::WrongType),
            )
        })?;
        if member.key == MODE && scalar == FactScalar::String(FULL.to_owned()) {
            return Err(failure(PRESETS, ConfigValueProblem::OutOfRange));
        }
        *slot = Some(scalar);
    }
    Ok(values)
}

/// A member's scalar as the JSON an instruction sends: a boolean, an integer or a word (the
/// types of the four keys); a millisecond or timestamp value as its number.
fn scalar_value(value: &FactScalar) -> Value {
    match value {
        FactScalar::Boolean(flag) => Value::Bool(*flag),
        FactScalar::Integer(number) => Value::from(*number),
        FactScalar::String(text) => Value::String(text.clone()),
        FactScalar::TimestampMs(ms) | FactScalar::DurationMs(ms) => Value::from(*ms),
    }
}
