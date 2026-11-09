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
use crate::configure::{self, ChangesProblem};
use actingcommand_contract::{ConfigSettingChange, MAX_CONFIG_SETTING_CHANGES};
use serde_json::{Map, Value};

/// `ac_config_set {changes: [{key, value?}]}`: answers `{settings, config_version?}`, each changed
/// key as it is in effect afterwards and (Workflow #429 R-4) the settings' version after the
/// change, as `actingctl config set|clear` prints them. Workflow #434 LAB-1b: the change and its
/// answer are `configure`'s.
pub(super) fn config_set(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["changes"])?;
    let changes = changes(arguments.value("changes"))?;
    let connection = context.runtime.connect_fresh(CLI.0, CLI.1)?;
    let write = record_provenance(connection, "ac_config_set", None, CLI, None)?;
    let configured =
        configure::configure(&write.client, changes).map_err(|error| client_error(&error))?;
    Ok(ToolSuccess {
        result: Value::Object(
            configure::configured_answer(&configured).map_err(super::packs::encode_failed)?,
        ),
        warnings: write.warnings,
    })
}

/// #402 HS-2: whether the call's changes name an instance's backend key (a call whose `changes`
/// is malformed names none; the tool refuses it as before).
pub(super) fn names_backend_key(arguments: &Map<String, Value>) -> bool {
    arguments
        .get("changes")
        .and_then(Value::as_array)
        .is_some_and(|changes| {
            changes.iter().any(|change| {
                change
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(actingcommand_contract::is_instance_backend_key)
            })
        })
}

/// The changes of one `ac_config_set` call, the same request `actingctl config set|clear`
/// sends: 1 to 64 objects `{key, value?}`, a missing or `null` value returning the key to its
/// configuration-file value. Anything else is `arguments_invalid` for `changes`: not an array
/// of objects (`wrong_type`), outside 1 to 64 changes (`out_of_range`), a change with another
/// member than key and value (`not_recognised`), a change without a non-empty string key
/// (`required`, or `wrong_type` for a key that is not a string). Workflow #434 LAB-1b: the changes
/// are `configure::json_changes`'.
pub(super) fn changes(value: Option<&Value>) -> Result<Vec<ConfigSettingChange>, ToolError> {
    configure::json_changes(value).map_err(|problem| match problem {
        ChangesProblem::Argument(problem) => invalid_argument("changes", problem),
        ChangesProblem::OutOfRange => out_of_range("changes", 1, MAX_CONFIG_SETTING_CHANGES as u64),
    })
}
