// SPDX-License-Identifier: AGPL-3.0-only

//! `actingledger codes [--code <code>] [--json | --markdown]` (Workflow #378 A8b; model-378
//! §5.3, "Readers"): the codes of the outcome catalog this build embeds
//! (`outcome::catalog`), read offline: no `--state-root`, no ledger and no Runtime.
//! `contracts/runtime-outcomes.md`, "Reading the catalog", is the contract.
//!
//! Every form lists the codes in name order, or the one code `--code` names:
//! - the default form, one tab-separated line per code: code, category, ladder class, review,
//!   status and description;
//! - `--markdown`, a line naming the catalog's SHA-256, then a table of the same columns;
//! - `--json`, one line `{schema_version, catalog_sha256, codes}`: `codes` holds each code's
//!   entry as the catalog holds it, and `catalog_sha256` is the SHA-256 of the embedded file,
//!   the content half of the catalog's version (the Runtime release is the other half; it lives
//!   only in the release tag, so no binary prints it).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;

use actingcommand_contract::codes::SharedCode;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::codes::ActingledgerLocation;
use crate::{CliError, invalid_arguments, output_error, serialization_error};

/// The `--json` document's schema.
const SCHEMA_VERSION: &str = "actingcommand.actingledger.codes.v1";
/// The text members of an entry a text or Markdown row reads, in column order after the name.
const ROW_MEMBERS: [&str; 5] = [
    "category",
    "ladder_class",
    "review",
    "status",
    "description",
];

/// How `codes` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodesForm {
    Text,
    Json,
    Markdown,
}

/// One `codes` call: every code, or only the one `--code` names, in one form.
#[derive(Debug)]
pub(crate) struct CodesRequest {
    only: Option<String>,
    form: CodesForm,
}

/// `[--code <code>] [--json | --markdown]`, each option at most once.
pub(crate) fn parse<I>(mut args: I) -> Result<CodesRequest, CliError>
where
    I: Iterator<Item = OsString>,
{
    let mut only = None;
    let mut form = None;
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| invalid_arguments("codes option is not valid UTF-8"))?;
        match option.as_str() {
            "--code" if only.is_none() => only = Some(crate::next_value(&mut args, "--code")?),
            "--json" | "--markdown" if form.is_none() => {
                form = Some(if option == "--json" {
                    CodesForm::Json
                } else {
                    CodesForm::Markdown
                });
            }
            "--code" => return Err(invalid_arguments("duplicate codes option --code")),
            "--json" | "--markdown" => {
                return Err(invalid_arguments(
                    "codes accepts one of --json and --markdown, once",
                ));
            }
            _ => return Err(invalid_arguments(format!("unknown codes option {option}"))),
        }
    }
    Ok(CodesRequest {
        only,
        form: form.unwrap_or(CodesForm::Text),
    })
}

/// The `--json` document.
#[derive(Serialize)]
struct CodesDocument<'c> {
    schema_version: &'static str,
    catalog_sha256: String,
    codes: BTreeMap<&'c str, &'c Value>,
}

/// Prints what `request` asks for from the catalog this build embeds.
pub(crate) fn write<W: Write>(request: &CodesRequest, output: &mut W) -> Result<(), CliError> {
    let text = actingcommand_contract::outcome::catalog();
    let catalog = actingcommand_contract::outcome::catalog_json()
        .map_err(|error| unreadable(format!("the embedded catalog does not parse: {error}")))?;
    let catalog_sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
    render(catalog, &catalog_sha256, request, output)
}

/// Prints `request` from `catalog`, a parsed `actingcommand.outcome-codes.v1` document whose
/// file has the SHA-256 `catalog_sha256`.
fn render<W: Write>(
    catalog: &Value,
    catalog_sha256: &str,
    request: &CodesRequest,
    output: &mut W,
) -> Result<(), CliError> {
    let all = catalog
        .get("codes")
        .and_then(Value::as_object)
        .ok_or_else(|| unreadable("the embedded catalog has no codes table"))?;
    let codes = select(all, request.only.as_deref())?;
    match request.form {
        CodesForm::Json => {
            let document = CodesDocument {
                schema_version: SCHEMA_VERSION,
                catalog_sha256: catalog_sha256.to_owned(),
                codes,
            };
            serde_json::to_writer(&mut *output, &document).map_err(serialization_error)?;
            output.write_all(b"\n").map_err(output_error)?;
        }
        CodesForm::Text => {
            for (name, entry) in &codes {
                let cells = row(name, entry)?
                    .into_iter()
                    .map(one_line)
                    .collect::<Vec<_>>();
                writeln!(output, "{name}\t{}", cells.join("\t")).map_err(output_error)?;
            }
        }
        CodesForm::Markdown => {
            writeln!(output, "Outcome catalog SHA-256 `{catalog_sha256}`.")
                .map_err(output_error)?;
            writeln!(output).map_err(output_error)?;
            writeln!(
                output,
                "| Code | Category | Ladder class | Review | Status | Description |"
            )
            .map_err(output_error)?;
            writeln!(output, "|---|---|---|---|---|---|").map_err(output_error)?;
            for (name, entry) in &codes {
                let cells = row(name, entry)?
                    .into_iter()
                    .map(markdown_cell)
                    .collect::<Vec<_>>();
                writeln!(output, "| `{name}` | {} |", cells.join(" | ")).map_err(output_error)?;
            }
        }
    }
    output.flush().map_err(output_error)
}

/// Every code in name order, or the one `only` names (`outcome_code_unknown` when the catalog
/// does not list it).
fn select<'c>(
    all: &'c Map<String, Value>,
    only: Option<&str>,
) -> Result<BTreeMap<&'c str, &'c Value>, CliError> {
    match only {
        None => Ok(all
            .iter()
            .map(|(name, entry)| (name.as_str(), entry))
            .collect()),
        Some(spelling) => all
            .iter()
            .find(|(name, _)| name.as_str() == spelling)
            .map(|(name, entry)| BTreeMap::from([(name.as_str(), entry)]))
            .ok_or_else(|| {
                CliError::new(
                    SharedCode::OutcomeCodeUnknown.code(),
                    ActingledgerLocation::ReadOutcomeCatalog.location(),
                    format!("the embedded catalog lists no code {spelling}"),
                )
            }),
    }
}

/// The text cells of one code's row after its name; an entry that is not an object or lacks
/// one of them is `outcome_catalog_unreadable`.
fn row<'c>(name: &str, entry: &'c Value) -> Result<Vec<&'c str>, CliError> {
    ROW_MEMBERS
        .iter()
        .map(|member| {
            entry
                .get(*member)
                .and_then(Value::as_str)
                .ok_or_else(|| unreadable(format!("code {name} has no text member {member}")))
        })
        .collect()
}

/// A cell of one text row: on one line, each run of white space (tabs and line breaks
/// included) one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A cell of one Markdown table row: on one line, with `|` escaped.
fn markdown_cell(text: &str) -> String {
    one_line(text).replace('|', "\\|")
}

fn unreadable(detail: impl Into<String>) -> CliError {
    CliError::new(
        SharedCode::OutcomeCatalogUnreadable.code(),
        ActingledgerLocation::ReadOutcomeCatalog.location(),
        detail,
    )
}
