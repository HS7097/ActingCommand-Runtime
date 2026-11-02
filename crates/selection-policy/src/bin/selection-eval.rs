// SPDX-License-Identifier: AGPL-3.0-only

//! Offline debugging shell around the selection evaluator.
//!
//! It reads three JSON files, calls the pure evaluator once, and prints one JSON envelope on
//! standard output: the decision on success, a typed error otherwise. Exit code 0 carries a
//! decision, exit code 2 carries an error. It is a debugging aid, not a Runtime entry point;
//! it acquires nothing, writes nothing, and reaches no device, ledger, or network.
//!
//! ```text
//! selection-eval --policy <file> --candidates <file> --facts <file> [--now-unix-ms <n>] [--table <file>]...
//! ```
//!
//! `--table` (Workflow #357 DB-2) gives one shared table, by its file, and may repeat. A document
//! that declares a table not given is refused with `dangling_reference`.
//!
//! `--now-unix-ms` defaults to the fact snapshot's own `snapshot_at_unix_ms`, so the tool
//! never reads a clock either.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use actingcommand_contract::parse_standard_pack_table;
use actingcommand_selection_policy::{
    Candidate, LoadedTable, MAX_DOCUMENT_BYTES, SelectionError, SelectionErrorCode,
    SelectionFactSnapshot, SelectionPolicy, SelectionTables, evaluate_with_tables,
    parse_canonical_json,
};
use serde::{Deserialize, Serialize};

const USAGE: &str = "selection-eval --policy <file> --candidates <file> --facts <file> [--now-unix-ms <n>] [--table <file>]...";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateSet {
    candidates: Vec<Candidate>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Envelope {
    Decision {
        decision: Box<actingcommand_selection_policy::SelectionDecision>,
    },
    Error {
        error: SelectionError,
    },
}

fn main() -> ExitCode {
    let envelope = match run() {
        Ok(decision) => Envelope::Decision {
            decision: Box::new(decision),
        },
        Err(error) => Envelope::Error { error },
    };
    let rendered = serde_json::to_string(&envelope).unwrap_or_else(|error| {
        format!(r#"{{"kind":"error","error":{{"code":"invalid_json","detail":"{error}"}}}}"#)
    });
    println!("{rendered}");
    match envelope {
        Envelope::Decision { .. } => ExitCode::SUCCESS,
        Envelope::Error { .. } => ExitCode::from(2),
    }
}

fn run() -> Result<actingcommand_selection_policy::SelectionDecision, SelectionError> {
    let arguments = parse_arguments()?;
    let policy: SelectionPolicy = read_document(&arguments.policy)?;
    let candidates: CandidateSet = read_document(&arguments.candidates)?;
    let facts: SelectionFactSnapshot = read_document(&arguments.facts)?;
    let now_unix_ms = arguments.now_unix_ms.unwrap_or(facts.snapshot_at_unix_ms);
    let tables = read_tables(&arguments.tables, now_unix_ms)?;
    let tables = SelectionTables::bind(&policy, &tables).map_err(|refusal| {
        SelectionError::new(SelectionErrorCode::TypeMismatch, refusal.to_string())
    })?;
    evaluate_with_tables(
        &policy,
        &tables,
        &candidates.candidates,
        &facts,
        now_unix_ms,
    )
}

/// Each `--table` file, read and checked as the Runtime reads a table, fixed at the instant.
fn read_tables(
    paths: &[PathBuf],
    now_unix_ms: u64,
) -> Result<Vec<Arc<LoadedTable>>, SelectionError> {
    paths
        .iter()
        .map(|path| {
            let bytes = fs::read(path).map_err(|error| {
                SelectionError::new(
                    SelectionErrorCode::ReadFailed,
                    format!("{}: {error}", path.display()),
                )
            })?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let table = parse_standard_pack_table(&bytes, None, name).map_err(|issue| {
                SelectionError::new(
                    SelectionErrorCode::InvalidArguments,
                    format!("{}: {issue}", path.display()),
                )
            })?;
            Ok(Arc::new(LoadedTable::load(table, now_unix_ms)))
        })
        .collect()
}

struct Arguments {
    policy: PathBuf,
    candidates: PathBuf,
    facts: PathBuf,
    now_unix_ms: Option<u64>,
    tables: Vec<PathBuf>,
}

fn parse_arguments() -> Result<Arguments, SelectionError> {
    let mut policy = None;
    let mut candidates = None;
    let mut facts = None;
    let mut now_unix_ms = None;
    let mut tables = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(flag) = arguments.next() {
        let value = arguments.next().ok_or_else(|| {
            SelectionError::new(
                SelectionErrorCode::InvalidArguments,
                format!("`{flag}` needs a value\n{USAGE}"),
            )
        })?;
        match flag.as_str() {
            "--policy" => policy = Some(PathBuf::from(value)),
            "--candidates" => candidates = Some(PathBuf::from(value)),
            "--facts" => facts = Some(PathBuf::from(value)),
            "--table" => tables.push(PathBuf::from(value)),
            "--now-unix-ms" => {
                now_unix_ms = Some(value.parse::<u64>().map_err(|error| {
                    SelectionError::new(
                        SelectionErrorCode::InvalidArguments,
                        format!("--now-unix-ms: {error}"),
                    )
                })?);
            }
            other => {
                return Err(SelectionError::new(
                    SelectionErrorCode::InvalidArguments,
                    format!("unknown flag `{other}`\n{USAGE}"),
                ));
            }
        }
    }
    let required = |value: Option<PathBuf>, flag: &str| {
        value.ok_or_else(|| {
            SelectionError::new(
                SelectionErrorCode::InvalidArguments,
                format!("`{flag}` is required\n{USAGE}"),
            )
        })
    };
    Ok(Arguments {
        policy: required(policy, "--policy")?,
        candidates: required(candidates, "--candidates")?,
        facts: required(facts, "--facts")?,
        now_unix_ms,
        tables,
    })
}

fn read_document<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T, SelectionError> {
    let bytes = fs::read(path).map_err(|error| {
        SelectionError::new(
            SelectionErrorCode::ReadFailed,
            format!("{}: {error}", path.display()),
        )
    })?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(SelectionError::new(
            SelectionErrorCode::DocumentTooLarge,
            format!("{}: {} bytes", path.display(), bytes.len()),
        ));
    }
    // Reject floats, duplicate keys, and non-string keys before the typed decode runs.
    parse_canonical_json(&bytes)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        SelectionError::new(
            SelectionErrorCode::InvalidJson,
            format!("{}: {error}", path.display()),
        )
    })
}
