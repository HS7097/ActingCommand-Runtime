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
//! - **A line over the limit after its work was done** (Workflow #434 LAB-5G3, work order point
//!   10): its job keeps it, and `ac_run_show` of the job's handle answers its summary
//!   ([`job_summary`]) beside `output_budget_exceeded`.
//! - **The export** ([`export_directory`], [`write_export`]): today's server's directory (the
//!   coordinator's ruling 18, board #434 6775192250) and the writer it shares with
//!   `actingctl observe --export` (`lab_ops::material_export::write`).
//! - **A run's line** (Workflow #434 LW-1, register §1.435; [`run_line`]): the line of a run's
//!   job, `run start`'s, carries every ledger record of the run's correlation (`events`, the
//!   forensic projection with payloads), 100 KB to 500 KB for a run that went far. This server
//!   answers it without them, counted, saying what it left out and which calls read it, wherever it
//!   gives that line: `ac_run_start` with `wait` and the `job` of `ac_run_show`.

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

/// Workflow #434 LAB-5G3 (work order point 10): what `ac_run_show` gives of a job whose line fits
/// the limit neither way, `{outcome, members, warnings_total, truncated?}`: the line's own outcome
/// (whether its work was done), the names of its other members, how many warnings it carries and,
/// when the serving code cut it, its `truncated`. The line itself stays with the job, unsent.
pub(crate) fn job_summary(line: &Value) -> Value {
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
    let mut summary = Map::new();
    if let Some(outcome) = line.get("outcome") {
        summary.insert("outcome".to_owned(), outcome.clone());
    }
    summary.insert("members".to_owned(), json!(members));
    summary.insert("warnings_total".to_owned(), json!(warnings_of(line).len()));
    if let Some(truncated) = line.get("truncated") {
        summary.insert("truncated".to_owned(), truncated.clone());
    }
    Value::Object(summary)
}

/// Workflow #434 LW-1: the member of `run start`'s line that holds every ledger record of the run's
/// correlation (`RuntimeFlowOutput::events`, the forensic projection with payloads).
const RUN_EVENTS: &str = "events";
/// Workflow #434 LW-1: how many records the answer left out of `events`.
const RUN_EVENTS_TOTAL: &str = "events_total";
/// Workflow #434 LW-1: the run's official OCR reads, kept while the answer fits.
const RUN_OCR: [&str; 2] = ["official_ocr_projection", "official_ocr_fields_projection"];
/// Workflow #434 LW-1: what the answer left out, as JSON pointers into the line (the overview's
/// `truncated` names its cut blocks so).
const TRUNCATED: &str = "truncated";
/// Workflow #434 LW-1: the calls that read what the answer left out.
const REST: &str = "rest";
/// Workflow #434 LW-1: the tools of those calls: a run's steps page by page, and its records.
const RUN_EVIDENCE_TOOL: &str = "ac_run_evidence";
const EVENTS_TOOL: &str = "ac_events";

/// Workflow #434 LW-1 (register §1.435): the line of a run's job, `run start`'s (the receipt, every
/// ledger record of the run's correlation under `events`, the official OCR reads when the run made
/// them, and the outcome), as this server answers it: every member but `events`, which is left out
/// and counted (`events_total`), and but the OCR reads unless `keep_ocr`; `truncated` lists what
/// was left out as JSON pointers into the line, and `rest` the calls that read it: `ac_run_evidence`
/// with the run's request id (`request_id`), its steps page by page, and `ac_events` with the
/// receipt's correlation id (the request id when the receipt names none) and `detail` `payload`,
/// its records as the line carried them. A line without `events` (a refusal, a client failure
/// that came back without the run's records) is answered as it is.
pub(crate) fn run_line(line: &Value, request_id: &str, keep_ocr: bool) -> Value {
    let Some(members) = line.as_object() else {
        return line.clone();
    };
    let Some(events) = members.get(RUN_EVENTS).and_then(Value::as_array) else {
        return line.clone();
    };
    let mut shaped = Map::new();
    let mut left_out = vec![Value::String(format!("/{RUN_EVENTS}"))];
    for (name, value) in members {
        if name == RUN_EVENTS {
            continue;
        }
        if !keep_ocr && RUN_OCR.contains(&name.as_str()) {
            left_out.push(Value::String(format!("/{name}")));
            continue;
        }
        shaped.insert(name.clone(), value.clone());
    }
    shaped.insert(RUN_EVENTS_TOTAL.to_owned(), json!(events.len()));
    shaped.insert(TRUNCATED.to_owned(), Value::Array(left_out));
    let mut records = Map::new();
    match line
        .pointer("/receipt/correlation_id")
        .and_then(Value::as_str)
    {
        Some(correlation_id) => records.insert("correlation_id".to_owned(), json!(correlation_id)),
        None => records.insert("request".to_owned(), json!(request_id)),
    };
    records.insert("detail".to_owned(), json!("payload"));
    shaped.insert(
        REST.to_owned(),
        json!([
            {"tool": RUN_EVIDENCE_TOOL, "arguments": {"id": request_id}},
            {"tool": EVENTS_TOOL, "arguments": records},
        ]),
    );
    Value::Object(shaped)
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
