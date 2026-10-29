// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl perf-preset set <name> | get` (Workflow #429 R-2, design `429-A-runtime.md` A.1
//! 契约 2; register §1.220), the CLI original of MCP `ac_perf_preset` (`mcp::perf_preset`). Both
//! read the preset table and the members' values in effect from the Runtime's program facts
//! `config.parameters.performance` and `config.parameters.scheduling` (RuntimeFactSnapshot) and
//! judge them with the one shared function of `actingcommand-runtime-config`
//! (`performance::presets`), which the console uses too:
//!
//! - `get` answers `{preset, members: [{key, value, source}]}`: the current preset (a preset's
//!   name, or `custom`) and each member's key with its value in effect (`null` and `default` for a
//!   key without one) and its source;
//! - `set <name>` sends the preset's members as one `ConfigureSettings`, every key at once or none
//!   (a member the preset leaves unwritten clears that key's online override), and answers
//!   `{preset, settings}`, the receipt's settings. A name the table does not have is refused
//!   before anything is written.
//!
//! A snapshot without the two records or the table's row is `config_facts_missing`; a row that
//! does not read back is `invalid_fact_snapshot_record`. The CLI writes with its (Cli, Cli) origin;
//! the line's outcome is the completed receipt's code of the last request
//! (`request_runtime_fact_snapshot_completed`, `request_configure_settings_completed`).

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::codes::{ReceiptCode, SanitizationCode};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    ConfigParameter, EventActor, EventSource, FactValue, RuntimeFactScope,
    config_parameters_fact_key,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use actingcommand_runtime_config::performance::presets::{self, PresetTable};
use actingcommand_runtime_config::{performance, scheduling};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

pub(crate) const COMMAND: &str = "perf-preset";
pub(crate) const SET: &str = "set";
pub(crate) const GET: &str = "get";
const USAGE: &str = "usage: actingctl perf-preset set <preset> --state-root <path>\nusage: actingctl perf-preset get --state-root <path>";

/// Why a preset command answered nothing.
pub(crate) enum Failure {
    /// The Runtime refused the request (its receipt's outcome on top), or did not answer.
    Runtime(RuntimeClientError),
    /// The snapshot has no `config.parameters.performance` or `.scheduling` record, or no row of
    /// the preset table.
    Missing,
    /// A row of those records does not read back.
    Unreadable,
    /// The preset table has no preset of this name.
    Unknown(String),
}

impl Failure {
    /// The failure as one outcome (Workflow #378 A5); an unknown name is the CLI's usage error.
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Runtime(error) => error.outcome(),
            Self::Missing => Outcome::new(ActingctlCode::ConfigFactsMissing),
            Self::Unreadable => Outcome::new(SanitizationCode::InvalidFactSnapshotRecord),
            Self::Unknown(name) => {
                cli_line::usage_invalid(Some(name), ArgumentProblem::NotRecognised)
            }
        }
    }
}

/// The parameters in effect the current preset reads: every row of `config.parameters.performance`
/// and `config.parameters.scheduling`, from one RuntimeFactSnapshot.
fn in_effect(client: &RuntimeClient) -> Result<Vec<ConfigParameter>, Failure> {
    let snapshot = client.runtime_fact_snapshot().map_err(Failure::Runtime)?;
    let mut parameters = Vec::new();
    for section in [performance::SECTION, scheduling::SECTION] {
        let key = config_parameters_fact_key(section);
        let record = snapshot
            .records
            .iter()
            .find(|record| record.scope == RuntimeFactScope::Runtime && record.key == key)
            .ok_or(Failure::Missing)?;
        let FactValue::RecordList(rows) = &record.value else {
            return Err(Failure::Unreadable);
        };
        for row in rows {
            parameters.push(presets::parameter_of_row(row).ok_or(Failure::Unreadable)?);
        }
    }
    Ok(parameters)
}

/// The preset table among `parameters`.
fn table(parameters: &[ConfigParameter]) -> Result<PresetTable, Failure> {
    match presets::table_of(parameters) {
        None => Err(Failure::Missing),
        Some(Err(_)) => Err(Failure::Unreadable),
        Some(Ok(table)) => Ok(table),
    }
}

/// `get`: the current preset and the members it was judged by.
pub(crate) fn get(client: &RuntimeClient) -> Result<Value, Failure> {
    let parameters = in_effect(client)?;
    let table = table(&parameters)?;
    Ok(presets::current_preset(&table, &parameters).answer())
}

/// `set`: the preset `name`'s members as one `ConfigureSettings`; `{preset, settings}`.
pub(crate) fn set(client: &RuntimeClient, name: &str) -> Result<Value, Failure> {
    let parameters = in_effect(client)?;
    let changes = table(&parameters)?
        .get(name)
        .ok_or_else(|| Failure::Unknown(name.to_owned()))?
        .changes();
    let configured = client
        .configure_settings(changes, None)
        .map_err(Failure::Runtime)?;
    Ok(json!({ "preset": name, "settings": configured.settings }))
}

// ---- the CLI

/// One parsed command line.
struct Parsed {
    action: &'static str,
    preset: Option<String>,
    state_root: PathBuf,
}

/// The `perf-preset` command, when it is the first word.
pub(crate) fn dispatch(arguments: &[OsString], detail: Detail) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(outcome) => return Some(cli_line::failure(&outcome, Some(USAGE))),
    };
    Some(match run(&parsed, detail) {
        Ok((answer, outcome)) => cli_line::success(cli_line::members(answer, COMMAND), &outcome),
        Err(Failure::Unknown(name)) => {
            cli_line::failure(&Failure::Unknown(name).outcome(), Some(USAGE))
        }
        // A refused request prints the Runtime's receipt beside its outcome and exits non-zero.
        Err(Failure::Runtime(error)) => match error.received_receipt() {
            Some(receipt) => match serde_json::to_value(receipt) {
                Ok(receipt) => {
                    let mut members = Map::new();
                    members.insert("receipt".to_owned(), receipt);
                    cli_line::receipt(members, &error.outcome(), true)
                }
                Err(_) => cli_line::refused(&error.outcome(), Some(&error.to_string())),
            },
            None => cli_line::refused(&error.outcome(), Some(&error.to_string())),
        },
        Err(failure) => cli_line::refused(&failure.outcome(), None),
    })
}

/// `perf-preset set <preset>` or `perf-preset get`, then `--state-root <path>` once; any other
/// word is `unknown`.
fn parse(arguments: &[OsString]) -> Result<Parsed, Outcome> {
    let word = |at: usize| arguments.get(at).map(|word| word.to_str());
    let (action, rest) = match word(1) {
        Some(Some(SET)) => match word(2) {
            Some(Some(name)) if !name.starts_with("--") && !name.trim().is_empty() => {
                ((SET, Some(name.to_owned())), 3)
            }
            Some(None) => return Err(cli_line::usage_invalid(None, ArgumentProblem::NotUtf8)),
            _ => {
                return Err(cli_line::usage_invalid(
                    Some("preset"),
                    ArgumentProblem::Required,
                ));
            }
        },
        Some(Some(GET)) => ((GET, None), 2),
        Some(Some(other)) => {
            return Err(cli_line::usage_invalid(
                Some(other),
                ArgumentProblem::NotRecognised,
            ));
        }
        Some(None) => return Err(cli_line::usage_invalid(None, ArgumentProblem::NotUtf8)),
        None => return Err(cli_line::usage_invalid(None, ArgumentProblem::Required)),
    };
    let takes = crate::flags_of(COMMAND, Some(action.0)).unwrap_or_default();
    let mut state_root = None;
    let mut words = arguments[rest..].iter();
    while let Some(word) = words.next() {
        let Some(word) = word.to_str() else {
            return Err(cli_line::usage_invalid(None, ArgumentProblem::NotUtf8));
        };
        let Some(flag) = takes.iter().copied().find(|flag| *flag == word) else {
            return Err(cli_line::usage_invalid(
                Some(word),
                ArgumentProblem::Unknown,
            ));
        };
        if state_root.is_some() {
            return Err(cli_line::usage_invalid(
                Some(flag),
                ArgumentProblem::Repeated,
            ));
        }
        let value = words
            .next()
            .ok_or_else(|| cli_line::usage_invalid(Some(flag), ArgumentProblem::ValueMissing))?
            .to_str()
            .ok_or_else(|| cli_line::usage_invalid(Some(flag), ArgumentProblem::NotUtf8))?;
        if value.trim().is_empty() {
            return Err(cli_line::usage_invalid(
                Some(flag),
                ArgumentProblem::ValueMissing,
            ));
        }
        state_root = Some(PathBuf::from(value));
    }
    Ok(Parsed {
        action: action.0,
        preset: action.1,
        state_root: state_root.ok_or_else(|| {
            cli_line::usage_invalid(Some("--state-root"), ArgumentProblem::Required)
        })?,
    })
}

/// Connects with the CLI's (Cli, Cli) origin and runs the action: its answer and the line's
/// outcome.
fn run(parsed: &Parsed, detail: Detail) -> Result<(Value, Outcome), Failure> {
    let config = RuntimeClientConfig::new(
        &parsed.state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    );
    let config = match detail {
        Detail::Codes => config.with_detail(Detail::Codes),
        Detail::Full => config,
    };
    let client = RuntimeClient::connect(config).map_err(Failure::Runtime)?;
    match (parsed.action, parsed.preset.as_deref()) {
        (SET, Some(name)) => Ok((
            set(&client, name)?,
            Outcome::new(ReceiptCode::ConfigureSettingsCompleted),
        )),
        _ => Ok((
            get(&client)?,
            Outcome::new(ReceiptCode::RuntimeFactSnapshotCompleted),
        )),
    }
}
