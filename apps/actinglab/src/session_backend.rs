// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-L (design `rd-1031/backend-switch/model-backend-switch.md` §2.2, §3.1 and §7 row HS-L;
//! register §1.275): the Lab's entry to an instance's touch and capture backends, the one setting
//! the CLI, MCP and the console change too.
//!
//! - `--instance <alias> session backend [--touch <name>] [--capture <name>] [--clear]` sends one
//!   `ConfigureSettings` on the Lab channel (with `via` when a counter forwarded the work), through
//!   the `runtime-client` call the CLI uses, which takes the receipt wait of a backend key itself.
//!   The request carries the instance's backend keys the line names, both in the one request
//!   when both are named; `--clear` returns both to the file's values, which is itself a switch.
//!   The answer is the receipt: each changed key in effect and the settings' `config_version`. A
//!   refusal is the Runtime's outcome with its problem word, and the command exits non-zero.
//! - There is no Lab-only backend: the Runtime keeps one state per instance (Alice 10-07).
//! - The Lab keeps no Runtime frame between two commands: every online `do` observes the
//!   instance before it presses (the Lab operation's before frame), so after a switch no frame of
//!   the closed session is pressed on; a frame reference a client still holds from it is refused
//!   `input_frame_unavailable` by the Runtime.
//! - The instance listing's `runtime_backends` column: the backends in effect as the Runtime
//!   reports them (`config.parameters.instance`), never the Lab file's.

use crate::state_roots::runtime_state_root;
use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions, runtime_slice_cli};
use actingcommand_contract::{
    ConfigSettingChange, FactScalar, FactValue, InstanceId, RuntimeFactScope,
    config_parameters_fact_key,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The two members of an instance's backend keys, `instance.<instance id>.<member>`.
const TOUCH_MEMBER: &str = "touch_backend";
const CAPTURE_MEMBER: &str = "capture_backend";
/// The options `session backend` takes besides the global `--instance`.
const OPTIONS: [&str; 3] = ["--touch", "--capture", "--clear"];
/// The Runtime's refusals of a request's own keys or values, reported as a validation failure
/// (exit 2); every other refusal is the instance's (exit 4).
const REQUEST_REFUSALS: [&str; 3] = [
    "config_value_invalid",
    "config_key_unknown",
    "config_key_not_settable",
];
/// The outcome values a refusal's details repeat beside the whole outcome.
const REFUSAL_VALUES: [&str; 3] = [
    "config_key",
    "config_value_problem",
    "backend_switch_problem",
];

/// What one `session backend` line asks for.
struct BackendChange {
    touch: Option<String>,
    capture: Option<String>,
    clear: bool,
}

impl BackendChange {
    /// The line's options: `--touch <name>` and `--capture <name>` (one or both, each once), or
    /// `--clear` alone. A backend name is the Runtime's to judge (`config_value_invalid`).
    fn parse(args: &[String]) -> CliOutcome<Self> {
        let flags = FlagArgs::parse(args)?;
        if let Some(word) = flags.positionals.first() {
            return Err(CliError::usage(format!(
                "session backend takes no positional argument, got {word}"
            )));
        }
        if let Some(option) = flags
            .flags
            .keys()
            .find(|option| !OPTIONS.contains(&option.as_str()))
        {
            return Err(CliError::usage(format!(
                "session backend does not take {option}; it takes --touch <name>, \
                 --capture <name> or --clear"
            )));
        }
        if let Some(option) = OPTIONS.iter().find(|option| flags.count(option) > 1) {
            return Err(CliError::usage(format!("{option} may be given once")));
        }
        let named = |option: &str| match flags.optional(option) {
            Some(value) if value == "true" => {
                Err(CliError::usage(format!("{option} requires a backend name")))
            }
            value => Ok(value),
        };
        let touch = named("--touch")?;
        let capture = named("--capture")?;
        let clear = match flags.optional("--clear") {
            None => false,
            Some(value) if value == "true" => true,
            Some(value) => {
                return Err(CliError::usage(format!(
                    "--clear takes no value, got {value}"
                )));
            }
        };
        match (clear, touch.is_some() || capture.is_some()) {
            (true, true) => Err(CliError::usage(
                "--clear returns both backend keys to the file's values; it takes neither \
                 --touch nor --capture",
            )),
            (false, false) => Err(CliError::usage(
                "session backend requires --touch <name>, --capture <name> or --clear",
            )),
            _ => Ok(Self {
                touch,
                capture,
                clear,
            }),
        }
    }

    /// The changes of the one request for the instance whose id is `instance`: the touch key
    /// first, then the capture key; `--clear` names both without a value.
    fn changes(&self, instance: &str) -> Vec<ConfigSettingChange> {
        let key = |member: &str| backend_key(instance, member);
        if self.clear {
            return [TOUCH_MEMBER, CAPTURE_MEMBER]
                .into_iter()
                .map(|member| ConfigSettingChange {
                    key: key(member),
                    value: None,
                })
                .collect();
        }
        [(TOUCH_MEMBER, &self.touch), (CAPTURE_MEMBER, &self.capture)]
            .into_iter()
            .filter_map(|(member, name)| {
                name.as_ref().map(|name| ConfigSettingChange {
                    key: key(member),
                    value: Some(Value::String(name.clone())),
                })
            })
            .collect()
    }

    fn shown(&self) -> Value {
        json!({"touch": self.touch, "capture": self.capture, "clear": self.clear})
    }
}

/// `instance.<instance id>.<member>`, one of an instance's two backend keys.
fn backend_key(instance: &str, member: &str) -> String {
    format!("instance.{instance}.{member}")
}

/// `session backend`: the instance's backend change, one `ConfigureSettings`, refused under
/// `--dry-run` before any Runtime connection.
pub(super) fn run_session_backend(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let change = BackendChange::parse(args)?;
    let alias = global
        .instance
        .clone()
        .filter(|alias| !alias.trim().is_empty())
        .ok_or_else(|| {
            CliError::usage(
                "session backend requires --instance <alias>, the Runtime instance whose \
                 backends change",
            )
        })?;
    crate::dry_run_gate::refuse(
        global,
        "session backend",
        Some("session instance list"),
        json!({"instance": alias, "change": change.shown()}),
    )?;
    let client = RuntimeClient::connect(crate::lab_client_config(runtime_state_root()?))
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let status = client
        .status()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let registered = status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == alias)
        .ok_or_else(|| {
            CliError::instance(format!("the Runtime has no registered instance {alias}"))
        })?;
    let instance_id = instance_id_text(&registered.instance_id())?;
    match client.configure_settings(change.changes(&instance_id), None) {
        Ok(configured) => {
            let settings = serde_json::to_value(configured.settings).map_err(|error| {
                CliError::device(format!("failed to encode the Runtime's settings: {error}"))
            })?;
            Ok(json!({
                "instance": alias,
                "instance_id": instance_id,
                "settings": settings,
                "config_version": configured.config_version,
            }))
        }
        Err(error) => Err(refusal(&alias, error)),
    }
}

/// The Runtime refused the change with a receipt: its top code is the error's code, its problem
/// word and key are repeated in the details beside the whole outcome and the receipt's state. A
/// failure without a receipt (the Runtime unreachable, the receipt never read) is mapped as every
/// Runtime-backed Lab command maps it.
fn refusal(alias: &str, error: RuntimeClientError) -> CliError {
    let Some(outcome) = error.receipt_outcome() else {
        return runtime_slice_cli::map_runtime_error(error);
    };
    let code = outcome.code().as_str().to_owned();
    let mut details = Map::new();
    details.insert("instance".to_owned(), json!(alias));
    details.insert("outcome".to_owned(), json!(code));
    for key in REFUSAL_VALUES {
        if let Some(value) = outcome.value(key) {
            details.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(receipt) = error.received_receipt() {
        details.insert("receipt_state".to_owned(), json!(receipt.state()));
    }
    match serde_json::to_value(outcome) {
        Ok(value) => details.insert("runtime_outcome".to_owned(), value),
        Err(encode) => details.insert(
            "runtime_outcome_unencoded".to_owned(),
            json!(encode.to_string()),
        ),
    };
    let problem = ["config_value_problem", "backend_switch_problem"]
        .into_iter()
        .find_map(|key| outcome.value(key).and_then(Value::as_str));
    let message = match problem {
        Some(problem) => {
            format!("the Runtime refused the backend change of {alias}: {code} ({problem})")
        }
        None => format!("the Runtime refused the backend change of {alias}: {code}"),
    };
    let class = if REQUEST_REFUSALS.contains(&code.as_str()) {
        ErrorKind::UsageValidation
    } else {
        ErrorKind::DeviceInstance
    };
    CliError::new(class, code, message, &[]).with_details(Value::Object(details))
}

/// The JSON text of a Runtime instance id, the `<instance id>` of its configuration keys.
fn instance_id_text(instance_id: &InstanceId) -> CliOutcome<String> {
    match serde_json::to_value(instance_id) {
        Ok(Value::String(text)) => Ok(text),
        Ok(other) => Err(CliError::device(format!(
            "a Runtime instance id encodes as {other}, not as a string"
        ))),
        Err(error) => Err(CliError::device(format!(
            "failed to encode a Runtime instance id: {error}"
        ))),
    }
}

/// One registered instance's two backend keys as the Runtime has them in effect.
struct InstanceBackends {
    instance_id: String,
    /// `touch_backend` and `capture_backend`, each `{value, source}`, or null when the Runtime
    /// reports no row of the key (a fixture, or a host started without a configuration).
    members: Map<String, Value>,
}

/// The backends in effect of the Runtime's registered instances, by alias, and the settings'
/// version they were read at.
struct RuntimeBackends {
    config_version: Option<u64>,
    instances: BTreeMap<String, InstanceBackends>,
}

/// #402 HS-L: the instance listing's read of the Runtime. A Runtime that cannot be read leaves the
/// listing as the file has it and says why (`runtime_backends_read`).
pub(super) struct RuntimeBackendRead(CliOutcome<RuntimeBackends>);

impl RuntimeBackendRead {
    /// Reads the Runtime of this process's state root now: `Status`, then `RuntimeFactSnapshot`
    /// with the settings' version (two general reads on the Lab channel, which record nothing).
    pub(super) fn now() -> Self {
        Self(read_runtime_backends())
    }

    /// The listing's `runtime_backends_read`: `{read: true, config_version}`, or
    /// `{read: false, error: {code, message}}` when the Runtime could not be read.
    pub(super) fn summary(&self) -> Value {
        match &self.0 {
            Ok(read) => json!({"read": true, "config_version": read.config_version}),
            Err(error) => json!({
                "read": false,
                "error": {"code": error.code, "message": error.message},
            }),
        }
    }

    /// The `runtime_backends` column of the listed instance `alias`: null when the Runtime was
    /// not read; `{registered: false}` when the Runtime has no instance of that alias; otherwise
    /// `{registered: true, instance_id, touch_backend, capture_backend}`.
    pub(super) fn column(&self, alias: &str) -> Value {
        let Ok(read) = &self.0 else {
            return Value::Null;
        };
        match read.instances.get(alias) {
            None => json!({"registered": false}),
            Some(backends) => {
                let mut column = backends.members.clone();
                column.insert("registered".to_owned(), Value::Bool(true));
                column.insert(
                    "instance_id".to_owned(),
                    Value::String(backends.instance_id.clone()),
                );
                Value::Object(column)
            }
        }
    }
}

fn read_runtime_backends() -> CliOutcome<RuntimeBackends> {
    let client = RuntimeClient::connect(crate::lab_client_config(runtime_state_root()?))
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let status = client
        .status()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let (snapshot, config_version) = client
        .runtime_fact_snapshot_versioned()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let section = config_parameters_fact_key("instance");
    let rows: Vec<&BTreeMap<String, FactScalar>> = match snapshot
        .records
        .iter()
        .find(|record| record.scope == RuntimeFactScope::Runtime && record.key == section)
    {
        None => Vec::new(),
        Some(record) => match &record.value {
            FactValue::RecordList(rows) => rows.iter().collect(),
            _ => {
                return Err(CliError::device(format!(
                    "the Runtime's {section} is not a record list"
                )));
            }
        },
    };
    let mut instances = BTreeMap::new();
    for registered in status.instances() {
        let instance_id = instance_id_text(&registered.instance_id())?;
        let mut members = Map::new();
        for member in [TOUCH_MEMBER, CAPTURE_MEMBER] {
            let key = backend_key(&instance_id, member);
            let row = rows.iter().find(
                |row| matches!(row.get("key"), Some(FactScalar::String(named)) if *named == key),
            );
            let in_effect = match row {
                None => Value::Null,
                Some(row) => json!({
                    "value": row_text(row.get("value"), &key)?,
                    "source": row_text(row.get("source"), &key)?,
                }),
            };
            members.insert(member.to_owned(), in_effect);
        }
        instances.insert(
            registered.instance_alias().to_owned(),
            InstanceBackends {
                instance_id,
                members,
            },
        );
    }
    Ok(RuntimeBackends {
        config_version,
        instances,
    })
}

/// A text member of the Runtime's row of `key`.
fn row_text(scalar: Option<&FactScalar>, key: &str) -> CliOutcome<String> {
    match scalar {
        Some(FactScalar::String(text)) => Ok(text.clone()),
        _ => Err(CliError::device(format!(
            "the Runtime's row of {key} carries no text value and source"
        ))),
    }
}
