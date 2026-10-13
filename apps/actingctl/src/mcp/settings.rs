// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1b (frozen model v1.2 §5.6): `ac_config_set`, the MCP side of
//! `actingctl config set|clear`. Both send the same `ConfigureSettings` changes on a (Cli, Cli)
//! connection; which key exists, may change and takes the value is the Runtime's answer, passed
//! on unchanged. Until U-3 the tool is in the observer tier, which is always on: changing a
//! setting is not tiered, every change is recorded (model §5.6, Alice: 「只要有记录就不怕乱改」).

use super::operator::{CLI, record_provenance};
use super::runtime::client_error;
use super::tools::{
    Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument, out_of_range,
};
use crate::codes::ArgumentProblem;
use actingcommand_contract::{ConfigSettingChange, MAX_CONFIG_SETTING_CHANGES};
use serde_json::{Map, Value, json};

/// `ac_config_set {changes: [{key, value?}]}`: answers `{settings}`, each changed key as it is in
/// effect afterwards.
pub(super) fn config_set(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["changes"])?;
    let changes = changes(arguments.value("changes"))?;
    let connection = context.runtime.connect_fresh(CLI.0, CLI.1)?;
    let write = record_provenance(connection, "ac_config_set", None, CLI, None)?;
    let settings = write
        .client
        .configure_settings(changes)
        .map_err(|error| client_error(&error))?;
    Ok(ToolSuccess {
        result: json!({ "settings": settings }),
        warnings: write.warnings,
    })
}

/// The changes of one `ac_config_set` call, the same request `actingctl config set|clear`
/// sends: 1 to 64 objects `{key, value?}`, a missing or `null` value returning the key to its
/// configuration-file value. Anything else is `arguments_invalid` for `changes`: not an array
/// of objects (`wrong_type`), outside 1 to 64 changes (`out_of_range`), a change with another
/// member than key and value (`not_recognised`), a change without a non-empty string key
/// (`required`, or `wrong_type` for a key that is not a string).
pub(super) fn changes(value: Option<&Value>) -> Result<Vec<ConfigSettingChange>, ToolError> {
    let Some(Value::Array(items)) = value else {
        return Err(invalid_argument("changes", ArgumentProblem::WrongType));
    };
    if items.is_empty() || items.len() > MAX_CONFIG_SETTING_CHANGES {
        return Err(out_of_range(
            "changes",
            1,
            MAX_CONFIG_SETTING_CHANGES as u64,
        ));
    }
    items
        .iter()
        .map(|item| {
            let Value::Object(change) = item else {
                return Err(invalid_argument("changes", ArgumentProblem::WrongType));
            };
            if change.keys().any(|name| name != "key" && name != "value") {
                return Err(invalid_argument("changes", ArgumentProblem::NotRecognised));
            }
            let key = match change.get("key") {
                Some(Value::String(key)) if !key.is_empty() => key,
                Some(Value::String(_)) | None => {
                    return Err(invalid_argument("changes", ArgumentProblem::Required));
                }
                Some(_) => return Err(invalid_argument("changes", ArgumentProblem::WrongType)),
            };
            Ok(ConfigSettingChange {
                key: key.to_owned(),
                value: change
                    .get("value")
                    .filter(|value| !value.is_null())
                    .cloned(),
            })
        })
        .collect()
}
