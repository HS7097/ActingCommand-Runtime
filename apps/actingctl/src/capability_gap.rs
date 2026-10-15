// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl capability-gap` (Workflow #410 item 6; board #410, Alice 2026-10-15), the CLI
//! original of MCP `ac_capability_gap` (`mcp::capability_gap`): an agent, or a person, records a
//! capability it needed and no command offered, with a short why, as material for deciding what
//! to build next ("缺口跟着实际使用走").
//!
//! The record is local only and is never sent anywhere: one JSON line appended to the user's
//! log `%LOCALAPPDATA%\ActingCommand\actingctl\capability-gaps.jsonl` (`APPDATA` when
//! `LOCALAPPDATA` is not set), `{schema_version, recorded_at_unix_ms, via, capability, why}`,
//! `via` `cli` or `mcp`. It is not a ledger record: a missing capability is not something the
//! Runtime did (iron rules 3 and 13), the client action record retires at U-6, and the log works
//! without actingd. Nothing reads the log back; a person reads it. Both sides check the two
//! texts alike and answer `{path, gap}`, the log's path and the line written; stdout is one line
//! with the outcome `capability_gap_recorded` (Workflow #378 A5).

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::outcome::{IoOp, Outcome};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const COMMAND: &str = "capability-gap";
const USAGE: &str = "usage: actingctl capability-gap --capability <text> --why <text>";
/// The schema of one line of the log.
const GAP_SCHEMA: &str = "actingcommand.capability-gap.v1";
/// The log below the user's local application data.
const LOG_DIRECTORY: [&str; 2] = ["ActingCommand", "actingctl"];
const LOG_FILE: &str = "capability-gaps.jsonl";
/// The longest `capability`, in characters: a short name of what was missing.
pub(crate) const MAX_CAPABILITY_CHARS: usize = 120;
/// The longest `why`, in characters: one or two sentences.
pub(crate) const MAX_WHY_CHARS: usize = 500;

/// The argument names, both sides (the CLI flags without `--`).
pub(crate) const CAPABILITY: &str = "capability";
pub(crate) const WHY: &str = "why";

/// What is wrong with a text argument: none when it is a non-blank single line of at most
/// `max` characters; `value_missing` when it is blank, `not_recognised` when it holds a control
/// character (a line break included), `out_of_range` when it is longer.
pub(crate) fn text_problem(text: &str, max: usize) -> Option<ArgumentProblem> {
    if text.trim().is_empty() {
        Some(ArgumentProblem::ValueMissing)
    } else if text.chars().any(char::is_control) {
        Some(ArgumentProblem::NotRecognised)
    } else if text.chars().count() > max * 2 {
        Some(ArgumentProblem::OutOfRange)
    } else {
        None
    }
}

/// The log of this user: `%LOCALAPPDATA%\ActingCommand\actingctl\capability-gaps.jsonl`, or
/// below `APPDATA` without `LOCALAPPDATA`; `capability_gap_write_failed` without a path when
/// neither is set.
pub(crate) fn log_path() -> Result<PathBuf, Outcome> {
    let root = std::env::var_os("LOCALAPPDATA")
        .filter(|root| !root.is_empty())
        .or_else(|| std::env::var_os("APPDATA").filter(|root| !root.is_empty()))
        .ok_or_else(|| Outcome::new(ActingctlCode::CapabilityGapWriteFailed))?;
    let mut path = PathBuf::from(root);
    path.extend(LOG_DIRECTORY);
    path.push(LOG_FILE);
    Ok(path)
}

/// Appends one gap to `log` as one line, `via` `cli` or `mcp`, and answers `{path, gap}`.
/// A failure is `capability_gap_write_failed` with the log's path, caused by the failure.
pub(crate) fn record(log: &Path, capability: &str, why: &str, via: &str) -> Result<Value, Outcome> {
    let failed = |error: &std::io::Error, op: IoOp, at: &Path| {
        Outcome::new(ActingctlCode::CapabilityGapWriteFailed)
            .path(log)
            .caused_by(Outcome::from_io_error(error, op).path(at))
    };
    let recorded_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(|| Outcome::new(ActingctlCode::ClockUnavailable))?;
    let gap = json!({
        "schema_version": GAP_SCHEMA,
        "recorded_at_unix_ms": recorded_at_unix_ms,
        "via": via,
        CAPABILITY: capability,
        WHY: why,
    });
    let mut line = gap.to_string();
    line.push(' ');
    if let Some(directory) = log.parent() {
        fs::create_dir_all(directory)
            .map_err(|error| failed(&error, IoOp::CreateDir, directory))?;
    }
    // One write of the whole line in append mode, so two writers never interleave a line.
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|error| failed(&error, IoOp::Open, log))?
        .write_all(line.as_bytes())
        .map_err(|error| failed(&error, IoOp::Write, log))?;
    Ok(json!({"path": log.display().to_string(), "gap": gap}))
}

/// `capability-gap --capability <text> --why <text>`: the flags `flags_of` names, each exactly
/// once, with a value. Any other word, a repeated flag or a missing or malformed value is
/// `actingctl_usage_invalid` naming the word (`argument`) and what is wrong with it
/// (`argument_problem`).
fn parse(arguments: &[OsString]) -> Result<(String, String), Outcome> {
    let takes = crate::flags_of(COMMAND, None).unwrap_or_default();
    let mut words = arguments.iter();
    let mut capability = None;
    let mut why = None;
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
        let (slot, max) = match flag.trim_start_matches('-') {
            CAPABILITY => (&mut capability, MAX_CAPABILITY_CHARS),
            _ => (&mut why, MAX_WHY_CHARS),
        };
        if slot.is_some() {
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
        if let Some(problem) = text_problem(value, max) {
            return Err(cli_line::usage_invalid(Some(flag), problem));
        }
        *slot = Some(value.to_owned());
    }
    match (capability, why) {
        (Some(capability), Some(why)) => Ok((capability, why)),
        (None, _) => Err(cli_line::usage_invalid(
            Some("--capability"),
            ArgumentProblem::Required,
        )),
        (_, None) => Err(cli_line::usage_invalid(
            Some("--why"),
            ArgumentProblem::Required,
        )),
    }
}

/// The `capability-gap` command, when it is the first word.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    let (capability, why) = match parse(&arguments[1..]) {
        Ok(parsed) => parsed,
        Err(outcome) => return Some(cli_line::failure(&outcome, Some(USAGE))),
    };
    let answer = log_path().and_then(|log| record(&log, &capability, &why, "cli"));
    Some(match answer {
        Ok(answer) => cli_line::success(
            cli_line::members(answer, COMMAND),
            &Outcome::new(ActingctlCode::CapabilityGapRecorded),
        ),
        Err(outcome) => cli_line::failure(&outcome, None),
    })
}

// Test-only: Workflow #410 item 6, the log the record appends to.
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Two records append two lines, each one complete JSON object with the gap as given; the
    // answer names the log and the line written. Nothing else is in the log.
    #[test]
    fn gaplog_each_record_appends_one_line_with_the_gap_as_given() {
        let local = TempDir::new().expect("local application data");
        let log = local
            .path()
            .join("ActingCommand")
            .join("actingctl")
            .join(LOG_FILE);
        let first = record(
            &log,
            "batch lab actions",
            "had to call ac_lab_do twelve times to scroll one list",
            "cli",
        )
        .unwrap_or_else(|outcome| panic!("the first record failed: {outcome:?}"));
        let second = record(
            &log,
            "logcat on a crash",
            "the run failed without a trace",
            "mcp",
        )
        .unwrap_or_else(|outcome| panic!("the second record failed: {outcome:?}"));
        assert_eq!(first["path"], json!(log.display().to_string()));
        assert_eq!(second["path"], first["path"]);
        let text = fs::read_to_string(&log).expect("the log");
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2, "{text}");
        for (line, answer, via) in [(lines[0], &first, "cli"), (lines[1], &second, "mcp")] {
            let gap: Value = serde_json::from_str(line).expect("one JSON object per line");
            assert_eq!(gap, answer["gap"], "the answer is the line written");
            assert_eq!(gap["schema_version"], json!(GAP_SCHEMA));
            assert_eq!(gap["via"], json!(via));
            assert!(gap["recorded_at_unix_ms"].as_u64().is_some_and(|ms| ms > 0));
        }
        assert_eq!(
            serde_json::from_str::<Value>(lines[0]).expect("the first line")[CAPABILITY],
            json!("batch lab actions")
        );
        assert_eq!(
            serde_json::from_str::<Value>(lines[1]).expect("the second line")[WHY],
            json!("the run failed without a trace")
        );
    }

    // A text is refused, before anything is written, when it is blank, holds a line break or is
    // longer than its bound.
    #[test]
    fn gaplog_a_blank_multiline_or_overlong_text_is_refused() {
        assert_eq!(text_problem("swipe and hold", 120), None);
        assert_eq!(
            text_problem("   ", 120),
            Some(ArgumentProblem::ValueMissing)
        );
        assert_eq!(
            text_problem("first line\nsecond line", 500),
            Some(ArgumentProblem::NotRecognised)
        );
        assert_eq!(
            text_problem(&"x".repeat(121), 120),
            Some(ArgumentProblem::OutOfRange)
        );
        assert_eq!(text_problem(&"x".repeat(120), 120), None);
    }
}
