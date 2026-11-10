// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl routine-summary (--hour | --day) [--instance <alias|instance_id|ADB port>]
//! [--since <revision>] --state-root <path>` (Workflow #429 R-12; register §1.264), the CLI
//! original of MCP `ac_routine_summary` (`mcp::observer::routine_summary`): the Runtime's
//! `RoutineSummary`, the health statistics of the policy runs and direct runs it settled since it
//! started, per instance, by hour or by calendar day of its local clock, read from its memory. It
//! only reads: the Runtime reads and records nothing in the ledger.
//!
//! - `--hour` or `--day`, exactly one, is the bucket width;
//! - `--instance` (alias, instance_id or ADB port) narrows the page to that instance, resolved
//!   to its alias with one status read; without it every instance answers;
//! - `--since` is the `revision` of an earlier page of the same `owner_epoch`: only the buckets
//!   changed after it are answered. A page of another `owner_epoch` means the Runtime restarted
//!   and the cursor is dropped.
//!
//! The line is the page's members, `{owner_epoch, granularity, revision, instances}`, and its
//! outcome the completed receipt's code (`request_routine_summary_completed`), as `perf-preset`
//! and `remedy` print theirs.

use crate::budget::{self, Budget, Fitted};
use crate::cli_line;
use crate::codes::ArgumentProblem;
use crate::instance_selector::{self, SelectorMiss};
use actingcommand_contract::codes::ReceiptCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{RoutineSummaryGranularity, RoutineSummaryPage};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

pub const COMMAND: &str = "routine-summary";
/// The flag of the hour buckets.
pub const HOUR: &str = "--hour";
/// The flag of the day buckets.
pub const DAY: &str = "--day";
/// The flag of the cursor, a page's `revision`.
pub const SINCE: &str = "--since";
/// The two width flags, as a usage outcome names them when neither is given.
const WIDTH: &str = "--hour|--day";
/// The members an answer written whole to its caller's export keeps beside `export` (Workflow
/// #434 LAB-5D: moved from MCP's `ac_routine_summary`).
const EXPORTED_HEADER: [&str; 3] = ["owner_epoch", "granularity", "revision"];
const USAGE: &str = "usage: actingctl routine-summary (--hour | --day) [--instance <alias|instance_id|ADB port>] [--since <revision>] --state-root <path>";

/// Why a routine summary read answered nothing.
pub enum Failure {
    /// The selector names no instance of the Runtime's status, or more than one.
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    /// The Runtime refused the read (its receipt's outcome on top), or did not answer.
    Runtime(RuntimeClientError),
}

impl Failure {
    /// The failure as one outcome (Workflow #378 A5).
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Selector { miss, selector } => miss.outcome(selector),
            Self::Runtime(error) => error.outcome(),
        }
    }
}

/// The page `RoutineSummary` answers on `client`: for the one instance `selector` names
/// (resolved to its alias with one status read), or every instance without it; by
/// `granularity`; with `since_revision` only the buckets changed after it.
pub fn read(
    client: &RuntimeClient,
    selector: Option<&str>,
    granularity: RoutineSummaryGranularity,
    since_revision: Option<u64>,
) -> Result<RoutineSummaryPage, Failure> {
    let alias = match selector {
        Some(selector) => {
            let status = client.status().map_err(Failure::Runtime)?;
            Some(
                instance_selector::select(status.instances(), selector)
                    .map_err(|miss| Failure::Selector {
                        miss,
                        selector: selector.to_owned(),
                    })?
                    .instance_alias()
                    .to_owned(),
            )
        }
        None => None,
    };
    client
        .routine_summary(alias.as_deref(), granularity, since_revision)
        .map_err(Failure::Runtime)
}

/// Workflow #434 LAB-5D: the answer of `summary`, the page's JSON `answer`, within `budget`, the
/// one way of the reads that export ([`budget::exported`]): whole without a budget, when it fits
/// or for a caller with no export; otherwise written whole through the caller's export and
/// answered as `{owner_epoch, granularity, revision, export}` with the warning `answer_exported`.
pub fn within_budget(answer: Value, budget: Option<&Budget<'_>>) -> Result<Fitted, Outcome> {
    budget::exported(answer, &EXPORTED_HEADER, budget)
}

// ---- the CLI

/// One parsed command line.
struct Parsed {
    granularity: RoutineSummaryGranularity,
    instance: Option<String>,
    since_revision: Option<u64>,
    state_root: PathBuf,
}

/// Why the CLI ended without an answer.
pub enum Ended {
    Usage(Outcome),
    Failure(Failure),
}

/// The `routine-summary` command, when it is the first word.
pub fn dispatch(arguments: &[OsString], detail: Detail) -> Option<cli_line::Exit> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    Some(match answer(arguments, detail) {
        Ok(members) => {
            cli_line::success(members, &Outcome::new(ReceiptCode::RoutineSummaryCompleted))
        }
        Err(Ended::Usage(outcome)) => cli_line::failure(&outcome, Some(USAGE)),
        // A refused read prints the Runtime's receipt beside its outcome and exits non-zero.
        Err(Ended::Failure(Failure::Runtime(error))) => match error.received_receipt() {
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
        Err(Ended::Failure(failure)) => cli_line::refused(&failure.outcome(), None),
    })
}

/// The members the line prints beside its outcome for `arguments` (`routine-summary` and its
/// flags): the page's, read on a connection with the CLI's (Cli, Cli) origin; or why it ended.
pub fn answer(arguments: &[OsString], detail: Detail) -> Result<Map<String, Value>, Ended> {
    let parsed = parse(arguments).map_err(Ended::Usage)?;
    let config = crate::front::client_config(&parsed.state_root);
    let config = match detail {
        Detail::Codes => config.with_detail(Detail::Codes),
        Detail::Full => config,
    };
    let client =
        RuntimeClient::connect(config).map_err(|error| Ended::Failure(Failure::Runtime(error)))?;
    let page = read(
        &client,
        parsed.instance.as_deref(),
        parsed.granularity,
        parsed.since_revision,
    )
    .map_err(Ended::Failure)?;
    Ok(cli_line::members(json!(page), COMMAND))
}

/// `routine-summary` and the flags `flags_of` names for it, each once: exactly one of `--hour`
/// and `--day`, which take no value; `--instance`, `--since` (a revision, a whole number) and
/// `--state-root` (required) with a value. Any other word is `unknown`.
fn parse(arguments: &[OsString]) -> Result<Parsed, Outcome> {
    let takes = crate::flags_of(COMMAND, None).unwrap_or_default();
    let mut granularity = None;
    let mut values = BTreeMap::<&str, String>::new();
    let mut words = arguments.get(1..).unwrap_or_default().iter();
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
        let width = match flag {
            HOUR => Some(RoutineSummaryGranularity::Hour),
            DAY => Some(RoutineSummaryGranularity::Day),
            _ => None,
        };
        if let Some(width) = width {
            match granularity.replace((flag, width)) {
                Some((given, _)) if given == flag => {
                    return Err(cli_line::usage_invalid(
                        Some(flag),
                        ArgumentProblem::Repeated,
                    ));
                }
                Some(_) => {
                    return Err(cli_line::usage_invalid(
                        Some(flag),
                        ArgumentProblem::Conflicting,
                    ));
                }
                None => continue,
            }
        }
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
    let since_revision = match values.remove(SINCE) {
        Some(revision) => Some(
            revision
                .parse::<u64>()
                .map_err(|_| cli_line::usage_invalid(Some(SINCE), ArgumentProblem::WrongType))?,
        ),
        None => None,
    };
    let (_, granularity) = granularity
        .ok_or_else(|| cli_line::usage_invalid(Some(WIDTH), ArgumentProblem::Required))?;
    Ok(Parsed {
        granularity,
        instance: values.remove("--instance"),
        since_revision,
        state_root,
    })
}
