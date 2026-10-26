// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl evidence confirm-read (--artifact <id> | --run <id>)` (#422 LN-7c C;
//! `rd-1026/ln7c/final-query.md` §4; Alice's ruling 70), the CLI original of MCP
//! `ac_evidence_confirm_read` (`mcp::evidence_read`). A person or an agent confirms that it read
//! one evidence frame, or a run (every frame captured in it before the confirmation). The
//! confirmation is one `client.action`: kind `Command`, control `evidence_read_frame` or
//! `evidence_read_run`, the artifact id or the run id as its path-safe value, surface `cli` here and
//! `mcp` there; there is no other record and no new record type.
//!
//! The Runtime records it with the connection's origin: the CLI's (Cli, Cli), with the identity
//! card `actingctl` declared in the same correlation first, or MCP's (Agent, Adapter). It refuses a
//! target the ledger does not hold (`client_action_invalid`) and records nothing then. The
//! ledger's frame index links the record to its target (`read_confirmed`); nothing is removed in
//! this slice. Opening or reading a frame (`ac_run_evidence`, `ac_observe`) is no confirmation.
//! Both sides answer `{target, event_id, sequence}`, `target` being `{artifact_id}` or `{run_id}`;
//! the CLI line's outcome is the completed receipt's code, `request_record_client_action_completed`.

use crate::cli_line;
use crate::codes::ArgumentProblem;
use actingcommand_contract::codes::ReceiptCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    ArtifactId, ClientActionKind, ClientActionRecord, ClientActionValue,
    EVIDENCE_READ_FRAME_CONTROL, EVIDENCE_READ_RUN_CONTROL, EventActor, EventSource,
    GovernanceIdentityCard, RunId, SanitizationError,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) const COMMAND: &str = "evidence";
pub(crate) const CONFIRM_READ: &str = "confirm-read";
const USAGE: &str = "usage: actingctl evidence confirm-read (--artifact <artifact_id> | --run <run_id>) --state-root <path>";
/// The `surface_id` of the confirmations the CLI records.
const CLI_SURFACE: &str = "cli";
/// The identity card the CLI declares before it records a confirmation.
const CLI_CLIENT: &str = "actingctl";

/// What a confirmation names: one capture frame by its artifact id, or a run.
#[derive(Clone, Copy)]
pub(crate) enum Target {
    Frame(ArtifactId),
    Run(RunId),
}

impl Target {
    /// A frame by the canonical text of its artifact id; `None` for any other text.
    pub(crate) fn frame(text: &str) -> Option<Self> {
        identifier(text).map(Self::Frame)
    }

    /// A run by the canonical text of its run id; `None` for any other text.
    pub(crate) fn run(text: &str) -> Option<Self> {
        identifier(text).map(Self::Run)
    }

    /// The confirmation's control and its value, the target's canonical text.
    fn control(self) -> (&'static str, String) {
        match self {
            Self::Frame(artifact) => (EVIDENCE_READ_FRAME_CONTROL, id_text(&artifact)),
            Self::Run(run) => (EVIDENCE_READ_RUN_CONTROL, id_text(&run)),
        }
    }

    /// The answer's `target`: `{artifact_id}` or `{run_id}`.
    fn answer(self) -> Value {
        match self {
            Self::Frame(artifact) => json!({"artifact_id": artifact}),
            Self::Run(run) => json!({"run_id": run}),
        }
    }
}

/// Why a confirmation was not recorded.
pub(crate) enum Failure {
    /// The Runtime refused the request (its receipt's outcome on top), or did not answer.
    Runtime(RuntimeClientError),
    /// The contract refuses the record.
    Record(SanitizationError),
}

impl Failure {
    /// The failure as one outcome (Workflow #378 A5).
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Runtime(error) => error.outcome(),
            Self::Record(error) => Outcome::new(error.code()),
        }
    }
}

/// Records the confirmation of `target` on `client`, an interaction begun on the caller's
/// connection: one `client.action` of `surface` with the connection's origin. Answers `{target,
/// event_id, sequence}` of the recorded `client.action`.
pub(crate) fn confirm(
    client: &RuntimeClient,
    surface: &str,
    target: Target,
) -> Result<Value, Failure> {
    let (control, value) = target.control();
    let action = ClientActionRecord::new(
        surface,
        control,
        ClientActionKind::Command,
        None,
        Some(ClientActionValue::PathSafeString(value)),
    )
    .map_err(Failure::Record)?;
    let recorded = client
        .record_client_action(action)
        .map_err(Failure::Runtime)?;
    Ok(json!({
        "target": target.answer(),
        "event_id": recorded.event_id,
        "sequence": recorded.sequence,
    }))
}

/// An identifier from its canonical text; `None` for any other text.
fn identifier<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(Value::String(text.to_owned())).ok()
}

/// An identifier as its canonical text.
fn id_text(id: &impl serde::Serialize) -> String {
    match json!(id) {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

// ---- the CLI

/// The `evidence` command, when it is the first word.
pub(crate) fn dispatch(arguments: &[OsString], detail: Detail) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    let (state_root, target) = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(outcome) => return Some(cli_line::failure(&outcome, Some(USAGE))),
    };
    Some(match run(&state_root, target, detail) {
        Ok(answer) => cli_line::success(
            cli_line::members(answer, COMMAND),
            &Outcome::new(ReceiptCode::RecordClientActionCompleted),
        ),
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

/// `evidence confirm-read` and the flags `flags_of` names for it, each once with a value: exactly
/// one of `--artifact` and `--run`, and `--state-root`. Any other word is `unknown`; an id that is
/// not one is `not_recognised`.
fn parse(arguments: &[OsString]) -> Result<(PathBuf, Target), Outcome> {
    match arguments.get(1).and_then(|word| word.to_str()) {
        Some(CONFIRM_READ) => {}
        Some(word) => {
            return Err(cli_line::usage_invalid(
                Some(word),
                ArgumentProblem::NotRecognised,
            ));
        }
        None => return Err(cli_line::usage_invalid(None, ArgumentProblem::Required)),
    }
    let takes = crate::flags_of(COMMAND, Some(CONFIRM_READ)).unwrap_or_default();
    let mut values = BTreeMap::<&str, String>::new();
    let mut words = arguments[2..].iter();
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
        if values.contains_key(flag) {
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
        values.insert(flag, value.to_owned());
    }
    let state_root = values
        .remove("--state-root")
        .map(PathBuf::from)
        .ok_or_else(|| cli_line::usage_invalid(Some("--state-root"), ArgumentProblem::Required))?;
    let refused = |flag| cli_line::usage_invalid(Some(flag), ArgumentProblem::NotRecognised);
    let target = match (values.remove("--artifact"), values.remove("--run")) {
        (Some(text), None) => Target::frame(&text).ok_or_else(|| refused("--artifact"))?,
        (None, Some(text)) => Target::run(&text).ok_or_else(|| refused("--run"))?,
        (Some(_), Some(_)) => {
            return Err(cli_line::usage_invalid(
                Some("--run"),
                ArgumentProblem::Conflicting,
            ));
        }
        (None, None) => {
            return Err(cli_line::usage_invalid(
                Some("--artifact"),
                ArgumentProblem::Required,
            ));
        }
    };
    Ok((state_root, target))
}

/// Connects with the CLI's (Cli, Cli) origin, begins an interaction, declares the identity card
/// `actingctl` in it (a refused card fails the command, with the Runtime's receipt) and records
/// the confirmation.
fn run(state_root: &Path, target: Target, detail: Detail) -> Result<Value, Failure> {
    let config = RuntimeClientConfig::new(
        state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    );
    let config = match detail {
        Detail::Codes => config.with_detail(Detail::Codes),
        Detail::Full => config,
    };
    let client = RuntimeClient::connect(config)
        .and_then(|connection| connection.begin_interaction())
        .map_err(Failure::Runtime)?;
    client
        .declare_governance_identity(&GovernanceIdentityCard {
            client: CLI_CLIENT.to_owned(),
            client_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            instance: None,
        })
        .map_err(Failure::Runtime)?;
    confirm(&client, CLI_SURFACE, target)
}
