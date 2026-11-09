// SPDX-License-Identifier: AGPL-3.0-only

//! The configuration and monitor operations (Workflow #434 LAB-1b; register §1.287), moved from
//! actingctl's `main.rs` and MCP's `ac_config_set`. Each sends its request on the connection it is
//! given; which key exists, may change and takes the value is the Runtime's answer, passed on
//! unchanged:
//!
//! - `config set`, `config clear` (`actingctl config set|clear`, MCP `ac_config_set`; Workflow
//!   #387 D1b): one `ConfigureSettings` of every change, answered `{settings, config_version?}`;
//! - `monitor set`, `monitor clear` (`actingctl monitor-set|monitor-clear`): `ConfigureMonitor`
//!   and `ClearMonitor` of one instance.

use crate::codes::ArgumentProblem;
use actingcommand_contract::{
    ConfigSettingChange, MAX_CONFIG_SETTING_CHANGES, RuntimeMonitorInstanceStatus,
    RuntimeMonitorPolicy,
};
use actingcommand_runtime_client::{ConfiguredSettings, RuntimeClient, RuntimeClientError};
use serde_json::{Map, Value};

/// `monitor set` defaults: the interval between two checks and the page expected.
pub const DEFAULT_MONITOR_INTERVAL_MS: u64 = 30_000;
pub const DEFAULT_EXPECTED_PAGE: &str = "home";

/// The changes of `config set <key>=<json>…` (`set`) and `config clear <key>…` (`clear`), the
/// same `ConfigureSettings` changes MCP's `changes` give: a set carries its value (JSON, not
/// `null`), a clear none. `None` for no change or more than `MAX_CONFIG_SETTING_CHANGES`, a set
/// without `=` or whose value is no JSON or `null`, and another action.
pub fn word_changes(action: &str, items: &[String]) -> Option<Vec<ConfigSettingChange>> {
    if items.is_empty() || items.len() > MAX_CONFIG_SETTING_CHANGES {
        return None;
    }
    items
        .iter()
        .map(|item| match action {
            "set" => {
                let (key, json) = item.split_once('=')?;
                let value = serde_json::from_str::<Value>(json)
                    .ok()
                    .filter(|value| !value.is_null())?;
                Some(ConfigSettingChange {
                    key: key.to_owned(),
                    value: Some(value),
                })
            }
            "clear" => Some(ConfigSettingChange {
                key: item.clone(),
                value: None,
            }),
            _ => None,
        })
        .collect()
}

/// Why MCP's `changes` give no changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangesProblem {
    /// What is wrong with the argument.
    Argument(ArgumentProblem),
    /// It holds no change or more than `MAX_CONFIG_SETTING_CHANGES`.
    OutOfRange,
}

/// The changes of MCP's `changes`: 1 to `MAX_CONFIG_SETTING_CHANGES` objects `{key, value?}`, a
/// missing or `null` value returning the key to its configuration-file value. Anything else is
/// refused: not an array of objects (`wrong_type`), outside 1 to 64 changes (`OutOfRange`), a
/// change with another member than key and value (`not_recognised`), a change without a non-empty
/// string key (`required`, or `wrong_type` for a key that is not a string).
pub fn json_changes(value: Option<&Value>) -> Result<Vec<ConfigSettingChange>, ChangesProblem> {
    let Some(Value::Array(items)) = value else {
        return Err(ChangesProblem::Argument(ArgumentProblem::WrongType));
    };
    if items.is_empty() || items.len() > MAX_CONFIG_SETTING_CHANGES {
        return Err(ChangesProblem::OutOfRange);
    }
    items
        .iter()
        .map(|item| {
            let Value::Object(change) = item else {
                return Err(ChangesProblem::Argument(ArgumentProblem::WrongType));
            };
            if change.keys().any(|name| name != "key" && name != "value") {
                return Err(ChangesProblem::Argument(ArgumentProblem::NotRecognised));
            }
            let key = match change.get("key") {
                Some(Value::String(key)) if !key.is_empty() => key,
                Some(Value::String(_)) | None => {
                    return Err(ChangesProblem::Argument(ArgumentProblem::Required));
                }
                Some(_) => return Err(ChangesProblem::Argument(ArgumentProblem::WrongType)),
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

/// `config set`, `config clear`: every change in one `ConfigureSettings`, accepted whole or
/// refused (the Runtime's receipt then travels in the error).
pub fn configure(
    client: &RuntimeClient,
    changes: Vec<ConfigSettingChange>,
) -> Result<ConfiguredSettings, RuntimeClientError> {
    client.configure_settings(changes, None)
}

/// The answer of `config set` and `config clear`: `{settings, config_version?}`, each changed key
/// as it is in effect afterwards and (Workflow #429 R-4) the settings' version after the change,
/// when the Runtime answers one; a member that does not encode is the encoder's error.
pub fn configured_answer(
    configured: &ConfiguredSettings,
) -> Result<Map<String, Value>, serde_json::Error> {
    let mut answer = Map::new();
    answer.insert(
        "settings".to_owned(),
        serde_json::to_value(configured.settings.as_slice())?,
    );
    if let Some(version) = configured.config_version {
        answer.insert("config_version".to_owned(), Value::from(version));
    }
    Ok(answer)
}

/// The policy of `monitor set`: a check every `interval_ms` (default
/// [`DEFAULT_MONITOR_INTERVAL_MS`]) expecting `expected_page` (default [`DEFAULT_EXPECTED_PAGE`]),
/// recovering when `recover`; `None` for a policy the contract refuses.
pub fn monitor_policy(
    interval_ms: Option<u64>,
    expected_page: Option<String>,
    recover: bool,
) -> Option<RuntimeMonitorPolicy> {
    RuntimeMonitorPolicy::new(
        interval_ms.unwrap_or(DEFAULT_MONITOR_INTERVAL_MS),
        expected_page.unwrap_or_else(|| DEFAULT_EXPECTED_PAGE.to_owned()),
        recover,
    )
    .ok()
}

/// `monitor set`: `policy` for the instance `alias`; its monitor status.
pub fn set_monitor(
    client: &RuntimeClient,
    alias: &str,
    policy: RuntimeMonitorPolicy,
) -> Result<RuntimeMonitorInstanceStatus, RuntimeClientError> {
    client.configure_monitor(alias, policy)
}

/// `monitor clear`: no monitor for the instance `alias`; its monitor status.
pub fn clear_monitor(
    client: &RuntimeClient,
    alias: &str,
) -> Result<RuntimeMonitorInstanceStatus, RuntimeClientError> {
    client.clear_monitor(alias)
}
