// SPDX-License-Identifier: AGPL-3.0-only

//! The file layer of the rows this crate resolves (Workflow #387 model v1.2 §5.3): a key's value
//! is the configuration file's when the file names it (source `explicit`), otherwise its default
//! (source `default`) from the defaults file, the section-level key or code. A list or object
//! the file names replaces the default whole; elements are never merged.

use crate::codes::ConfigCode;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRow};
use crate::value::parse_value;
use actingcommand_contract::{ConfigParameter, ConfigParameterSource};
use serde_json::Value;
use std::collections::BTreeMap;

/// The member of the configuration file a row's key names: at the key itself, at the row's
/// other path, or in the `instances[]` entry of `instance` (under the member's name, where a
/// dotted member names a nested one, Workflow #405 `scheduling.random_delay.*`, or at the row's
/// path in it). A `null` member is absent.
pub fn file_value<'a>(
    row: &KeyRow,
    key: &str,
    instance: Option<&str>,
    file: &'a Value,
) -> Option<&'a Value> {
    let entry = || {
        instance.and_then(|instance| {
            file.get("instances")?
                .as_array()?
                .iter()
                .find(|entry| entry.get("instance_id").and_then(Value::as_str) == Some(instance))
        })
    };
    let found = match (row.file_path, row.name) {
        (FilePath::Key, _) => member(file, key),
        (FilePath::At(path), _) => member(file, path),
        (FilePath::InstanceMember, KeyName::PerInstance(name)) => member(entry()?, name),
        (FilePath::InstanceMemberAt(path), KeyName::PerInstance(_)) => member(entry()?, path),
        (FilePath::InstanceMember | FilePath::InstanceMemberAt(_), KeyName::Exact(_))
        | (FilePath::Absent, _) => None,
    };
    found.filter(|value| !value.is_null())
}

/// The member at a dotted path.
pub fn member<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |value, segment| value.get(segment))
}

/// The value of every key of `rows` and its source (§5.3, §5.5): the exact keys in row order,
/// then each instance of `instances` in turn with its per-instance keys in row order, so that one
/// instance's members stand together. `code_default` gives a `default = code` row's value; a
/// `default = none` key the file does not name is left out. A default that is missing, or a
/// value outside the key's type or range, is `config_value_invalid` for that key.
pub fn render_parameters(
    rows: &[KeyRow],
    instances: &[String],
    defaults: &Value,
    file: &Value,
    code_default: &dyn Fn(&KeyRow) -> Option<Value>,
) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut resolved = BTreeMap::<String, (Value, ConfigParameterSource)>::new();
    // (0 for an exact key, 1 + the instance's position otherwise; the row's position; the row;
    // the key).
    let mut ordered = Vec::new();
    // Section-level keys first, so that a `section:` default can read them.
    for pass_sections in [false, true] {
        for (row_at, row) in rows.iter().enumerate() {
            if matches!(row.default, DefaultSource::Section(_)) != pass_sections {
                continue;
            }
            let targets: Vec<(usize, Option<&str>)> = match row.name {
                KeyName::Exact(_) => vec![(0, None)],
                KeyName::PerInstance(_) => instances
                    .iter()
                    .enumerate()
                    .map(|(at, id)| (at + 1, Some(id.as_str())))
                    .collect(),
            };
            for (slot, instance) in targets {
                let Some(key) = row.key_for(instance) else {
                    continue;
                };
                let value = match file_value(row, &key, instance, file) {
                    Some(value) => Some((value.clone(), ConfigParameterSource::Explicit)),
                    None => default_value(row, &key, defaults, &resolved, code_default)?
                        .map(|value| (value, ConfigParameterSource::Default)),
                };
                if let Some(value) = value {
                    resolved.insert(key.clone(), value);
                    ordered.push((slot, row_at, row, key));
                }
            }
        }
    }
    ordered.sort_by_key(|(slot, row_at, _, _)| (*slot, *row_at));
    let mut parameters = Vec::with_capacity(ordered.len());
    for (_, _, row, key) in &ordered {
        let (value, source) = resolved
            .get(key)
            .ok_or_else(|| ConfigFailure::new(ConfigCode::ConfigValueInvalid, key.clone()))?;
        parameters.push(ConfigParameter {
            key: key.clone(),
            value: parse_value(row, key, value)?,
            source: *source,
            settable: row.settable.is_settable(),
        });
    }
    Ok(parameters)
}

/// The default of `key` (§5.5 `default`); `None` for a row without one.
fn default_value(
    row: &KeyRow,
    key: &str,
    defaults: &Value,
    resolved: &BTreeMap<String, (Value, ConfigParameterSource)>,
    code_default: &dyn Fn(&KeyRow) -> Option<Value>,
) -> Result<Option<Value>, ConfigFailure> {
    let missing = || ConfigFailure::new(ConfigCode::ConfigValueInvalid, key);
    match row.default {
        DefaultSource::None => Ok(None),
        DefaultSource::File => member(defaults, key)
            .filter(|value| !value.is_null())
            .cloned()
            .map(Some)
            .ok_or_else(missing),
        DefaultSource::Section(section_key) => resolved
            .get(section_key)
            .map(|(value, _)| Some(value.clone()))
            .ok_or_else(missing),
        DefaultSource::Code => code_default(row).map(Some).ok_or_else(missing),
    }
}
