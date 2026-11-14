// SPDX-License-Identifier: AGPL-3.0-only

//! What this server does with a `serve` child's answer before it answers the call (Workflow #434
//! LAB-5G4b, its second part; register §1.287, "Slice LAB-5G4b"; `contracts/lab-two-entries.md`
//! §9.3, "Calls served by `serve` children"; the coordinator's rulings on board #434, 12 of
//! 6775192250 and those of 2026-11-13 on the first part's two open points, not the owner's
//! words): the two things today's MCP server does around its Lab child
//! (`apps/actingctl/src/mcp/lab.rs`), ported to the child's line.
//!
//! - **Which rows** ([`LAB_ROWS`], one table in one place): `record stop`, `observe` and `do`, the
//!   rows of today's `ac_record_stop`, `ac_lab_observe` and `ac_lab_do`, each with the parts its
//!   exported line keeps inline and whether a capture records a client action. The operation
//!   table cannot say either (see the table).
//! - **The client action after a capture** ([`client_action`]; today's `author` and
//!   `record_action`): once the child of a call of `observe` or `do` with `capture` true answered,
//!   whatever its `dry_run` says (the coordinator's ruling 2 of 2026-11-13: as today's server
//!   records it), one `RecordClientAction{surface mcp, control_id the tool, kind command, value
//!   the envelope's req_id}` on a connection of its own, after the identity card, through the
//!   steps a write's provenance takes (`runtime::record_provenance`). Its warnings, today's three:
//!   `client_action_without_req_id` when the envelope carries no `req_id` (recorded all the same,
//!   without a value), the recording's own (`mcp_identity_declaration_refused`), and
//!   `client_action_not_recorded` caused by the failure when the recording fails; the call's
//!   answer stands, and the Lab command is never run again. A call no child answered records
//!   nothing.
//! - **The Lab overflow rule** ([`overflow_rule`]; today's `finish`, `fits_as_job`,
//!   `keep_frame_parts`, `BINDING_PARTS` and `FRAME_PARTS`): a child's success line (`exit_code`
//!   0) with those warnings joined that does not fit this server's answer budget as sent in the
//!   call's era (`budget::fits`) is written whole (the child's line) by this server's export
//!   writer and answered `{req_id, export, overflowed: true}`, then the parts its row keeps, each
//!   while the answer still fits, its warnings `answer_exported` first (or the export's
//!   `material_export_failed`, and no `export`, when the file could not be written) and then the
//!   line's, and the line's `outcome`. A failure line is never exported. An answer that does not
//!   fit even so is the job's, as every line over the budget is (LAB-5G3, point 10).

use crate::children::Answer;
use crate::runtime;
use actingcommand_contract::ClientActionValue;
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::budget::{self as lab_budget, Budget, Writer};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::ActingctlCode;
use serde_json::{Map, Value};

/// The members of a line this module reads or writes.
const REQ_ID: &str = "req_id";
const EXPORT: &str = "export";
const OVERFLOWED: &str = "overflowed";
const WARNINGS: &str = "warnings";
const OUTCOME: &str = "outcome";
/// The parameters of a call this module reads: whether it captures, and the instance its client
/// action names, as the call gives it (today's `record_action`).
const CAPTURE: &str = "capture";
const INSTANCE: &str = "instance";
/// The members of the export reference (`{path, sha256, size}`) the warning names.
const PATH: &str = "path";
const SHA256: &str = "sha256";
const SIZE: &str = "size";
/// `record stop`'s member that holds its binding parts, and the parts (today's `BINDING_PARTS`;
/// Workflow #336 PKG-S5: record stop answers no package reference).
const LAB: &str = "lab";
const BINDING_PARTS: [&str; 4] = [
    "binding_example",
    "binding_requires",
    "prerequisite_entry_example",
    "catalog_on_failure_example",
];
/// The frame parts of an `observe` or `do` line, in the order they are kept (today's
/// `FRAME_PARTS`; Workflow #410 item 1): the frame's coordinate system, the paths of the frame
/// files, (#463 L1 fix round 1) a recorded step's next frame, the next screen, and the
/// after-action attachment.
const FRAME_PARTS: [&str; 5] = [
    "frame_coordinates",
    "frame_path",
    "preview",
    "next_frame",
    "after_attachment",
];

/// What a line written whole to the export keeps inline beside `{req_id, export, overflowed}`.
#[derive(Debug, Clone, Copy)]
enum Kept {
    /// These members of the line's member `member`, together under it, when the answer still fits
    /// with them (today's binding parts of `ac_record_stop`).
    Together {
        member: &'static str,
        parts: &'static [&'static str],
    },
    /// These members of the line, in this order, each while the answer still fits (today's frame
    /// parts of `ac_lab_observe` and `ac_lab_do`).
    EachInOrder(&'static [&'static str]),
}

/// One row of [`LAB_ROWS`].
#[derive(Debug)]
struct LabRow {
    /// The row's words.
    row: &'static str,
    /// What its line keeps inline when it is written whole.
    kept: Kept,
    /// Whether a call of it with `capture` true records the client action once its child
    /// answered.
    capture_recorded: bool,
}

/// The rows whose answers this server treats as today's server treats its Lab tools' (one table
/// in one place): `record stop` (today's `ac_record_stop`), `observe` (`ac_lab_observe`) and `do`
/// (`ac_lab_do`; this server's `ac_do`). The operation table cannot say what is here: it declares
/// a row's parameters, the kinds of request it sends and what it answers as a sentence
/// (`output`), but no member of the line it answers, so it cannot name the parts to keep beside
/// an export; and seven of its rows take `capture` (`observe`, `do`, `recover`, `ensure`, `wait`,
/// `record step`, `locate`), of which today's server records a client action after `observe` and
/// `do` alone, the Lab requests the Runtime records (`lab.request`).
static LAB_ROWS: [LabRow; 3] = [
    LabRow {
        row: "record stop",
        kept: Kept::Together {
            member: LAB,
            parts: &BINDING_PARTS,
        },
        capture_recorded: false,
    },
    LabRow {
        row: "observe",
        kept: Kept::EachInOrder(&FRAME_PARTS),
        capture_recorded: true,
    },
    LabRow {
        row: "do",
        kept: Kept::EachInOrder(&FRAME_PARTS),
        capture_recorded: true,
    },
];

/// The entry of [`LAB_ROWS`] of the row `row`, if it has one.
fn lab_row(row: &str) -> Option<&'static LabRow> {
    LAB_ROWS.iter().find(|entry| entry.row == row)
}

/// The line a call of the row `row`'s tool `tool` with `params` (the parameters
/// `calls::prepare` gave) answers, from the children's `answer`, within `budget` (this server's
/// measure in the call's era and its export writer): the client action after a capture
/// ([`client_action`]), its warnings joined after the line's own, then the Lab overflow rule
/// ([`overflow_rule`]). A call no child answered (refused past `--children`, or a child that gave
/// no answer) is answered as it is: no Lab command ran, nothing is recorded or exported.
pub(crate) fn answered(
    row: &str,
    tool: &str,
    params: &Map<String, Value>,
    answer: Answer,
    budget: &Budget<'_>,
) -> Exit {
    let Answer {
        exit,
        req_id,
        answered,
    } = answer;
    let Exit {
        line,
        developer,
        code,
    } = exit;
    let child_line = match line {
        Ok(Value::Object(child_line)) if answered => child_line,
        line => {
            return Exit {
                line,
                developer,
                code,
            };
        }
    };
    let added = if records_capture(row, params) {
        client_action(tool, params, req_id.as_deref())
    } else {
        Vec::new()
    };
    let line = overflow_rule(row, child_line, code, req_id.as_deref(), added, budget);
    Exit {
        line: Ok(line),
        developer,
        code,
    }
}

/// Whether a call of the row `row` with `params` records the client action once its child
/// answered: a row of [`LAB_ROWS`] that records one, called with `capture` true, whatever its
/// `dry_run` says (today's `author`).
fn records_capture(row: &str, params: &Map<String, Value>) -> bool {
    lab_row(row).is_some_and(|entry| entry.capture_recorded)
        && params.get(CAPTURE) == Some(&Value::Bool(true))
}

/// The client action of a call of the tool `tool` with `params` after its child answered (today's
/// `record_action`): recorded on a connection of its own to the Runtime of this server's state
/// root, naming the call's `instance` as it gives it and carrying `req_id` as a path-safe string
/// when there is one. Its warnings: `client_action_without_req_id` without one, the recording's
/// own, and `client_action_not_recorded` caused by the failure (an unlocated state root, the
/// client's failure, or `client_action_invalid`) when it was not recorded.
fn client_action(tool: &str, params: &Map<String, Value>, req_id: Option<&str>) -> Vec<Value> {
    let mut warnings = Vec::new();
    if req_id.is_none() {
        warnings.push(cli_line::warning(&Outcome::new(
            ActingctlCode::ClientActionWithoutReqId,
        )));
    }
    let value = req_id.map(|id| ClientActionValue::PathSafeString(id.to_owned()));
    let instance = params.get(INSTANCE).and_then(Value::as_str);
    let recorded = runtime::place().state_root.and_then(|state_root| {
        runtime::record_provenance(&state_root, tool, instance, value)
            .map_err(|failed| failed.outcome())
    });
    match recorded {
        Ok(write) => warnings.extend(write.warnings),
        Err(cause) => warnings.push(cli_line::warning(
            &Outcome::new(ActingctlCode::ClientActionNotRecorded).caused_by(cause),
        )),
    }
    warnings
}

/// The Lab overflow rule for the line `child_line` a child answered for the row `row` with the
/// exit code `code` and the request id `req_id`, `added` the warnings this server joins after the
/// line's own (today's `finish` and `keep_frame_parts`). The line with them joined is answered as
/// it is when it fits `budget`, when it is a failure (`code` not 0: a failure is never exported),
/// and when `budget` has no export writer. Else the child's line is written whole and answered
/// [`exported`].
fn overflow_rule(
    row: &str,
    child_line: Map<String, Value>,
    code: u8,
    req_id: Option<&str>,
    added: Vec<Value>,
    budget: &Budget<'_>,
) -> Value {
    let whole = Value::Object(joined(child_line.clone(), added));
    if code != 0 || (budget.fits)(&whole) {
        return whole;
    }
    let Some(export) = budget.export else {
        return whole;
    };
    exported(
        row,
        &Value::Object(child_line),
        &whole,
        req_id,
        export,
        budget.fits,
    )
}

/// `line` with `added` after its own warnings ([`listed`]); as it is when `added` is empty.
fn joined(mut line: Map<String, Value>, added: Vec<Value>) -> Map<String, Value> {
    if added.is_empty() {
        return line;
    }
    let mut warnings = listed(line.get(WARNINGS));
    warnings.extend(added);
    line.insert(WARNINGS.to_owned(), Value::Array(warnings));
    line
}

/// The warnings a line's `warnings` member holds: its items, a member that is no list as one
/// warning, none without it.
fn listed(warnings: Option<&Value>) -> Vec<Value> {
    match warnings {
        None => Vec::new(),
        Some(Value::Array(warnings)) => warnings.clone(),
        Some(other) => vec![other.clone()],
    }
}

/// The answer of a success line over the budget: `child_line` (the child's line) written whole as
/// JSON through `export` (`<its sha256>.json`), and `{req_id, export, overflowed: true}`, `req_id`
/// `null` when the child gave none, `export` the file's reference `{path, sha256, size}` (absent
/// when it could not be written); then the parts the row `row` keeps ([`parts`]), each tried in
/// turn and kept while the answer still `fits`; then `warnings`, `answer_exported` (the file) or
/// the export's `material_export_failed` first and `whole`'s warnings after it; then `whole`'s
/// `outcome`.
fn exported(
    row: &str,
    child_line: &Value,
    whole: &Value,
    req_id: Option<&str>,
    export: Writer<'_>,
    fits: &dyn Fn(&Value) -> bool,
) -> Value {
    let mut head = Map::new();
    head.insert(
        REQ_ID.to_owned(),
        req_id.map_or(Value::Null, |id| Value::String(id.to_owned())),
    );
    let first = match lab_budget::write_json(export, child_line) {
        Ok(reference) => {
            let note = cli_line::warning(&answer_exported(&reference));
            head.insert(EXPORT.to_owned(), reference);
            note
        }
        Err(failure) => cli_line::warning(&failure),
    };
    head.insert(OVERFLOWED.to_owned(), Value::Bool(true));
    let mut warnings = vec![first];
    warnings.extend(listed(whole.get(WARNINGS)));
    let outcome = whole.get(OUTCOME);
    let answer = |kept: &[(String, Value)]| {
        let mut line = head.clone();
        line.extend(kept.iter().cloned());
        line.insert(WARNINGS.to_owned(), Value::Array(warnings.clone()));
        if let Some(outcome) = outcome {
            line.insert(OUTCOME.to_owned(), outcome.clone());
        }
        Value::Object(line)
    };
    let mut kept = Vec::new();
    for part in parts(row, child_line) {
        kept.push(part);
        if !fits(&answer(kept.as_slice())) {
            kept.pop();
        }
    }
    answer(kept.as_slice())
}

/// The parts of `line` the row `row` keeps inline ([`LAB_ROWS`]), in the order they are tried:
/// for [`Kept::Together`] one member holding those of its parts `line`'s member has (none when it
/// has none of them); for [`Kept::EachInOrder`] each part `line` has, in the table's order; none
/// for a row the table does not name.
fn parts(row: &str, line: &Value) -> Vec<(String, Value)> {
    let picked = |from: &Value, names: &[&str]| {
        names
            .iter()
            .filter_map(|name| {
                from.get(*name)
                    .map(|part| ((*name).to_owned(), part.clone()))
            })
            .collect::<Vec<_>>()
    };
    match lab_row(row).map(|entry| entry.kept) {
        Some(Kept::Together { member, parts }) => {
            let together = line
                .get(member)
                .filter(|holder| holder.is_object())
                .map(|holder| picked(holder, parts))
                .unwrap_or_default();
            if together.is_empty() {
                return Vec::new();
            }
            vec![(
                member.to_owned(),
                Value::Object(together.into_iter().collect()),
            )]
        }
        Some(Kept::EachInOrder(parts)) => picked(line, parts),
        None => Vec::new(),
    }
}

/// The warning on a line written whole to the export `reference` (`{path, sha256, size}`),
/// today's: `answer_exported` with `path`, `actual_sha256` and `actual_bytes`.
fn answer_exported(reference: &Value) -> Outcome {
    let mut warning = Outcome::new(ActingctlCode::AnswerExported);
    if let Some(path) = reference.get(PATH).and_then(Value::as_str) {
        warning = warning.path(path);
    }
    if let Some(sha256) = reference.get(SHA256).and_then(Value::as_str) {
        warning = warning.actual_sha256(sha256);
    }
    if let Some(size) = reference.get(SIZE).and_then(Value::as_u64) {
        warning = warning.actual_bytes(size);
    }
    warning
}
