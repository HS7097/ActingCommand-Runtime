// SPDX-License-Identifier: AGPL-3.0-only

//! Offline debugging shell around the selection evaluator.
//!
//! It reads three JSON files, calls the pure evaluator once, and prints one JSON envelope on
//! standard output: the decision on success, a typed error otherwise. Exit code 0 carries a
//! decision, exit code 2 carries an error. It is a debugging aid, not a Runtime entry point;
//! it acquires nothing, writes nothing, and reaches no device, ledger, or network.
//!
//! ```text
//! selection-eval --policy <file> --candidates <file> --facts <file> [--now-unix-ms <n>]
//! ```
//!
//! `--now-unix-ms` defaults to the fact snapshot's own `snapshot_at_unix_ms`, so the tool
//! never reads a clock either.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use actingcommand_selection_policy::{
    Candidate, MAX_DOCUMENT_BYTES, SelectionError, SelectionErrorCode, SelectionFactSnapshot,
    SelectionPolicy, evaluate, parse_canonical_json,
};
use serde::{Deserialize, Serialize};

const USAGE: &str =
    "selection-eval --policy <file> --candidates <file> --facts <file> [--now-unix-ms <n>]";

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
    evaluate(&policy, &candidates.candidates, &facts, now_unix_ms)
}

struct Arguments {
    policy: PathBuf,
    candidates: PathBuf,
    facts: PathBuf,
    now_unix_ms: Option<u64>,
}

fn parse_arguments() -> Result<Arguments, SelectionError> {
    let mut policy = None;
    let mut candidates = None;
    let mut facts = None;
    let mut now_unix_ms = None;
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
