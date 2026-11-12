// SPDX-License-Identifier: AGPL-3.0-only

//! The answer budget and the export of a served call (Workflow #434 LAB-5G2b; register §1.287,
//! "Slice LAB-5G2b"; `contracts/lab-two-entries.md` §9.3; the survey's §4).
//!
//! - **The limits** are today's MCP server's, copied (`apps/actingctl/src/mcp/tools.rs`,
//!   `RESULT_LIMIT` and `PAYLOAD_LIMIT`): the whole `tools/call` result as sent, text and
//!   `structuredContent` with the 2026-07-28 members, takes at most 24 KiB.
//! - **The measure** ([`fits`]) the serving code asks about an answer it cuts or exports: the
//!   answer as the line sends it in the session's protocol version, its JSON as text and, from
//!   2025-06-18 on, again as `structuredContent`, with the room today's two limits leave beside
//!   each copy of a payload for the line's outcome and the warnings it adds ([`LINE_RESERVE`]).
//! - **The answer as sent** ([`carried`]; today's `carried` and `summary`, adapted to the line in
//!   place of the `{ok, result}` envelope): the line as text beside `structuredContent` when that
//!   fits; else, where the version has `structuredContent`, the line there alone beside a summary
//!   text; a line that fits neither way is `output_budget_exceeded` ([`exceeded`]) with the bytes
//!   it takes and the limit. Never cut silently, never sent over the limit.
//! - **The export** ([`export_directory`], [`write_export`]): today's server's directory (the
//!   coordinator's ruling 18, board #434 6775192250) and the writer it shares with
//!   `actingctl observe --export` (`lab_ops::material_export::write`).

use crate::protocol::{self, Era};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::material_export;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The whole `tools/call` result as sent: today's `RESULT_LIMIT`
/// (`apps/actingctl/src/mcp/tools.rs`), 24 KiB.
pub(crate) const RESULT_LIMIT: usize = 24 * 1024;
/// The payload today's server sizes an answer to, so that its text can repeat it beside
/// `structuredContent`: today's `PAYLOAD_LIMIT` (`apps/actingctl/src/mcp/tools.rs`), 11 KiB.
const PAYLOAD_LIMIT: usize = 11 * 1024;
/// What those two limits leave beside each copy of a payload of [`PAYLOAD_LIMIT`]: the room this
/// measure keeps, per copy, for the line's outcome and the warnings the line adds beside an
/// answer, which the serving code does not hand the measure.
const LINE_RESERVE: usize = RESULT_LIMIT / 2 - PAYLOAD_LIMIT;
/// The instruction of a summary text: today's `NARROW_HINT` (`apps/actingctl/src/mcp/tools.rs`).
const NARROW_HINT: &str =
    "only structuredContent carries the whole answer; narrow the call to have it here as text";

/// The `tools/call` result of `line` in `era`, `text` its text content: `structuredContent` the
/// line from 2025-06-18 on, `isError` as given.
pub(crate) fn tool_result(line: &Value, era: Era, text: String, is_error: bool) -> Value {
    let mut result = Map::new();
    result.insert(
        "content".to_owned(),
        json!([{"type": "text", "text": text}]),
    );
    if era.structured() {
        result.insert("structuredContent".to_owned(), line.clone());
    }
    result.insert("isError".to_owned(), Value::Bool(is_error));
    Value::Object(result)
}

/// The bytes the `tools/call` result `result` takes as sent in `era`, the 2026-07-28 members
/// included (`protocol::complete`).
fn sent_bytes(result: &Value, era: Era) -> usize {
    protocol::complete(era, result.clone()).to_string().len()
}

/// Whether `answer`, as the serving code holds it (the line's members without its outcome and
/// without the warnings the line adds), is sent within the limit in `era`: its JSON as text and,
/// where the version has it, as `structuredContent`, with [`LINE_RESERVE`] for each copy.
pub(crate) fn fits(answer: &Value, era: Era) -> bool {
    let copies = if era.structured() { 2 } else { 1 };
    sent_bytes(&tool_result(answer, era, answer.to_string(), false), era) + copies * LINE_RESERVE
        <= RESULT_LIMIT
}

/// The `tools/call` result carrying `line` in `era` within the limit (today's `carried`, adapted
/// to the line): the text repeating the line (beside `structuredContent` where the version has
/// it) when that fits; else, where the version has `structuredContent`, a summary text
/// ([`summary`]) beside it with as many of the line's warnings as fit, in order. When neither
/// fits, the bytes of the smaller one, the summary without warnings.
pub(crate) fn carried(line: &Value, era: Era, is_error: bool) -> Result<Value, usize> {
    let repeated = tool_result(line, era, line.to_string(), is_error);
    let bytes = sent_bytes(&repeated, era);
    if bytes <= RESULT_LIMIT {
        return Ok(repeated);
    }
    if !era.structured() {
        return Err(bytes);
    }
    let summarised = |kept: usize| {
        let result = tool_result(line, era, summary(line, kept).to_string(), is_error);
        let result_bytes = sent_bytes(&result, era);
        (result, result_bytes)
    };
    let count = warnings_of(line).len();
    let (every, every_bytes) = summarised(count);
    if every_bytes <= RESULT_LIMIT {
        return Ok(every);
    }
    let (mut kept, fewest_bytes) = summarised(0);
    if fewest_bytes > RESULT_LIMIT {
        return Err(fewest_bytes.min(bytes));
    }
    for more in 1..count {
        let (result, more_bytes) = summarised(more);
        if more_bytes > RESULT_LIMIT {
            break;
        }
        kept = result;
    }
    Ok(kept)
}

/// The text content beside a `structuredContent` too large to repeat (today's `summary`, adapted
/// to the line): `{outcome, structured_content_only, warnings?, warnings_omitted?, truncated?,
/// narrow}`. That is the line's outcome; the names of its other members, whose whole only
/// `structuredContent` carries; its first `kept` warnings, and how many others there are; its
/// `truncated`, what an answer cut to the budget lost; and how to have the whole line as text.
fn summary(line: &Value, kept: usize) -> Value {
    let members = line
        .as_object()
        .map(|members| {
            members
                .keys()
                .filter(|name| name.as_str() != "outcome")
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut text = Map::new();
    if let Some(outcome) = line.get("outcome") {
        text.insert("outcome".to_owned(), outcome.clone());
    }
    text.insert("structured_content_only".to_owned(), json!(members));
    let warnings = warnings_of(line);
    let kept = kept.min(warnings.len());
    if kept > 0 {
        text.insert(
            "warnings".to_owned(),
            Value::Array(warnings[..kept].to_vec()),
        );
    }
    if kept < warnings.len() {
        text.insert("warnings_omitted".to_owned(), json!(warnings.len() - kept));
    }
    if let Some(truncated) = line.get("truncated") {
        text.insert("truncated".to_owned(), truncated.clone());
    }
    text.insert("narrow".to_owned(), json!(NARROW_HINT));
    Value::Object(text)
}

/// The warnings a line carries.
fn warnings_of(line: &Value) -> &[Value] {
    line.get("warnings")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// `output_budget_exceeded`: a line that takes `sent_bytes` as sent (`actual_bytes`), over the
/// limit (`limit_bytes`).
pub(crate) fn exceeded(sent_bytes: usize) -> Outcome {
    Outcome::new(ActingctlCode::OutputBudgetExceeded)
        .actual_bytes(sent_bytes)
        .limit_bytes(RESULT_LIMIT)
}

/// Where this server writes an answer whole, and a material or a frame (today's server's
/// directory, `apps/actingctl/src/mcp/observer.rs`, `export_directory`): `%TEMP%`,
/// `actingcommand-mcp`, `materials`.
pub(crate) fn export_directory() -> PathBuf {
    std::env::temp_dir()
        .join("actingcommand-mcp")
        .join("materials")
}

/// The export writer on `directory` (`lab_ops::material_export::write`): `bytes`, whose `sha256`
/// (`sha256:<hex>`) the caller knows, written as `<hex>.<extension>` through a partial file moved
/// into place; `{path, sha256, size}`, or the writer's `material_export_failed`.
pub(crate) fn write_export(
    directory: &Path,
    sha256: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<Value, Outcome> {
    material_export::write(directory, sha256, extension, bytes).map_err(|failure| failure.outcome())
}
