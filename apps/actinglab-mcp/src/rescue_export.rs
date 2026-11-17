// SPDX-License-Identifier: AGPL-3.0-only

//! A rescue row's success over the answer budget, written whole and answered its header (Workflow
//! #434 PP-1, 0.12.72; register §1.465; `apps/actinglab-mcp/MANUAL.md`, sections 4, 5 and 6).
//!
//! On Windows this server serves the rescue module's rows in its own process
//! (`calls::rescue_line`), and the module's code takes no answer budget: it neither cuts nor
//! exports. Before this slice its line went from there, unshaped, to the measure of the answer as
//! sent (`budget::carried`), which can only repeat a line or send it in `structuredContent` alone:
//! a line that fit neither way (a `place --plan` of 52 034 bytes beside the 24 KiB limit) answered
//! `output_budget_exceeded`, `ac_run_show` of its job gave the names of its members
//! (`budget::job_summary`), and nothing was written. [`answered`] gives such a line what the rows
//! of this process that export give theirs (`lab_ops::budget::exported`): the file, and its header
//! beside it.
//!
//! - **Which lines**: a success line (exit code 0) that the measure sends neither way. Every other
//!   line (a refusal, a failure, a line the measure sends, in `structuredContent` alone too) is
//!   answered as it is, byte for byte, and nothing is written for it.
//! - **The file**: the line whole, through this server's export writer
//!   (`lab_ops::budget::write_json`: `<sha256>.json` in the export directory), as a child's line is
//!   written under the Lab overflow rule (`crate::after_child`).
//! - **The header** ([`header`]): the parts [`HEADERS`] names for the row, each where the line holds
//!   it (`place`'s `schedule_changes` under `plan`), then `truncated` when a part was reduced or left
//!   to the file, then `export` (`{path, sha256, size}`), then `warnings`, `answer_exported` first
//!   and the line's own after it, then the line's `outcome`. A file that cannot be written is the
//!   first warning instead, `material_export_failed`, and the header has no `export`, as a child's
//!   overflow answers it (the manual, section 6). A header that does not fit even so is answered as
//!   every line over the budget is: `output_budget_exceeded` with its job's handle, the job keeping
//!   the header.
//! - **The forms of a part** ([`forms`]; one rule for every part of every row): whole; for an object,
//!   without the members that hold its rows ([`ROWS`]: `schedule_changes`' `packages`), each
//!   counted in its place as `<name>_total`; then with every list it holds counted so; for a list,
//!   its count `<name>_total` in its place. The first that fits beside what is already kept is kept,
//!   and what it leaves to the file is named in `truncated`, as JSON pointers into the line, which is
//!   the file (as a run's line names what it left out, `budget::run_line`, and a list of a record
//!   cut to its bound is counted as `<name>_total`); a part none of whose forms fits is left out
//!   and named there whole.

use crate::after_child;
use crate::budget;
use crate::protocol::Era;
use actingcommand_lab_ops::budget::{self as lab_budget, Budget, Writer};
use actingcommand_lab_ops::cli_line::{self, Exit};
use serde_json::{Map, Value, json};

/// The members of a header this module writes or reads.
const EXPORT: &str = "export";
const WARNINGS: &str = "warnings";
const OUTCOME: &str = "outcome";
const TRUNCATED: &str = "truncated";
/// What a list's name takes where its count stands in its place.
const TOTAL: &str = "_total";

/// What one rescue row's header keeps beside the export.
#[derive(Debug)]
struct HeaderRow {
    /// The row's words.
    row: &'static str,
    /// The parts of its line, as JSON pointers into the line, in the order they are tried.
    parts: &'static [&'static str],
}

/// The parts of a rescue row's line its header keeps beside the export (one table in one place:
/// the operation table names no member of a line). `place`, its plan form and a placement alike,
/// both of whose lines carry the plan: the plan's `schedule_changes`, the packages it makes enter
/// or leave scheduling (`enters`, `leaves`, `content_only`, `unread` and the rows `packages`).
/// `undo`: the steps its plan form would reverse (`reverses`) and its `schedule_changes`. `op
/// status`: the members the manual names as its answer (section 9), the unfinished operation, the
/// last operations and, with `op`, that operation's `journal`. Every other row, `deploy` and its
/// plan form among them (the manual names no member of that answer): no part, its outcome naming
/// what it did (`deploy --plan`'s `plan_id` and `commit_sha`).
static HEADERS: [HeaderRow; 3] = [
    HeaderRow {
        row: "place",
        parts: &["/plan/schedule_changes"],
    },
    HeaderRow {
        row: "undo",
        parts: &["/reverses", "/schedule_changes"],
    },
    HeaderRow {
        row: "op status",
        parts: &["/unfinished", "/last_operations", "/journal"],
    },
];

/// The members of a kept part that hold its rows, which its second form leaves to the file first:
/// `schedule_changes`' `packages`, one row per package with its part before and after.
const ROWS: [&str; 1] = ["packages"];

/// The line `exit` the rescue module gave for the row `row`, answered within `answer_budget` (this
/// server's measure in `era` and its export writer): a success line the measure would send neither
/// way (`budget::carried`) written whole through the writer and answered its [`header`]; every
/// other line, and every line when `answer_budget` has no writer, as it is.
pub(crate) fn answered(row: &str, exit: Exit, era: Era, answer_budget: &Budget<'_>) -> Exit {
    let Exit {
        line,
        developer,
        code,
    } = exit;
    let line = match line {
        Ok(line) if code == 0 && line.is_object() => line,
        line => {
            return Exit {
                line,
                developer,
                code,
            };
        }
    };
    let line = match answer_budget.export {
        Some(export) if budget::carried(&line, era, false).is_err() => {
            header(row, &line, export, answer_budget.fits)
        }
        _ => line,
    };
    Exit {
        line: Ok(line),
        developer,
        code,
    }
}

/// The header of `line`, the row `row`'s, written whole through `export` (see the module's
/// description): each part [`HEADERS`] names for the row that the line holds, in the first of its
/// [`forms`] with which the header still `fits`, else left to the file; `truncated` naming what was
/// left; `export`, the file's reference, unless the file could not be written; `warnings`,
/// `answer_exported` (or the writer's `material_export_failed`) first and the line's own after it;
/// the line's `outcome`.
fn header(row: &str, line: &Value, export: Writer<'_>, fits: &dyn Fn(&Value) -> bool) -> Value {
    let (reference, first) = match lab_budget::write_json(export, line) {
        Ok(reference) => {
            let note = cli_line::warning(&after_child::answer_exported(&reference));
            (Some(reference), note)
        }
        Err(failure) => (None, cli_line::warning(&failure)),
    };
    let mut warnings = vec![first];
    warnings.extend(after_child::listed(line.get(WARNINGS)));
    let answer = |kept: &Map<String, Value>, left: &[String]| {
        let mut header = kept.clone();
        if !left.is_empty() {
            header.insert(TRUNCATED.to_owned(), json!(left));
        }
        if let Some(reference) = &reference {
            header.insert(EXPORT.to_owned(), reference.clone());
        }
        header.insert(WARNINGS.to_owned(), Value::Array(warnings.clone()));
        if let Some(outcome) = line.get(OUTCOME) {
            header.insert(OUTCOME.to_owned(), outcome.clone());
        }
        Value::Object(header)
    };
    let mut kept = Map::new();
    let mut left = Vec::new();
    for pointer in parts(row) {
        let Some(value) = line.pointer(pointer) else {
            continue;
        };
        let fitting = forms(pointer, value).into_iter().find(|form| {
            let mut tried = kept.clone();
            put(&mut tried, &form.at, form.value.clone());
            fits(&answer(
                &tried,
                &[left.as_slice(), form.left.as_slice()].concat(),
            ))
        });
        match fitting {
            Some(form) => {
                put(&mut kept, &form.at, form.value);
                left.extend(form.left);
            }
            None => left.push((*pointer).to_owned()),
        }
    }
    answer(&kept, &left)
}

/// The parts [`HEADERS`] names for the row `row`; none for a row it does not name.
fn parts(row: &str) -> &'static [&'static str] {
    match HEADERS.iter().find(|header| header.row == row) {
        Some(header) => header.parts,
        None => &[],
    }
}

/// One form of a kept part: the value placed at `at` (a JSON pointer into the header, the part's
/// own place or, for a list's count, `<name>_total` beside it), and the pointers into the line of
/// what it leaves to the file.
#[derive(Debug)]
struct Form {
    at: String,
    value: Value,
    left: Vec<String>,
}

/// The forms of the part `value` at `pointer`, in the order they are tried: whole; for an object,
/// its rows ([`ROWS`]) counted, then every list it holds counted ([`counted`]), each form only when
/// it differs from the one before; for a list, its count beside its place.
fn forms(pointer: &str, value: &Value) -> Vec<Form> {
    let mut forms = vec![Form {
        at: pointer.to_owned(),
        value: value.clone(),
        left: Vec::new(),
    }];
    match value {
        Value::Object(members) => {
            let rows = counted(pointer, members, |name| ROWS.contains(&name));
            let lists = counted(pointer, members, |_| true);
            for form in [rows, lists].into_iter().flatten() {
                if forms.iter().all(|earlier| earlier.left != form.left) {
                    forms.push(form);
                }
            }
        }
        Value::Array(items) => {
            let (parent, name) = pointer.rsplit_once('/').unwrap_or(("", pointer));
            forms.push(Form {
                at: format!("{parent}/{name}{TOTAL}"),
                value: json!(items.len()),
                left: vec![pointer.to_owned()],
            });
        }
        _ => {}
    }
    forms
}

/// The object `members` at `pointer` with each list whose name `counts` takes counted in its place
/// as `<name>_total`, its other members as they are; `None` when it holds no such list.
fn counted(
    pointer: &str,
    members: &Map<String, Value>,
    counts: impl Fn(&str) -> bool,
) -> Option<Form> {
    let mut value = Map::new();
    let mut left = Vec::new();
    for (name, member) in members {
        match member.as_array() {
            Some(items) if counts(name) => {
                value.insert(format!("{name}{TOTAL}"), json!(items.len()));
                left.push(format!("{pointer}/{name}"));
            }
            _ => {
                value.insert(name.clone(), member.clone());
            }
        }
    }
    (!left.is_empty()).then(|| Form {
        at: pointer.to_owned(),
        value: Value::Object(value),
        left,
    })
}

/// `value` placed in `header` at `pointer`, whose segments [`HEADERS`] writes without escapes,
/// each object on the way made where there is none yet. The table's parts never lie inside one
/// another, so nothing on the way is ever another value.
fn put(header: &mut Map<String, Value>, pointer: &str, value: Value) {
    let mut segments = pointer.split('/').skip(1).collect::<Vec<_>>();
    let Some(last) = segments.pop() else {
        return;
    };
    let mut place = header;
    for segment in segments {
        let Value::Object(inner) = place
            .entry(segment.to_owned())
            .or_insert_with(|| Value::Object(Map::new()))
        else {
            return;
        };
        place = inner;
    }
    place.insert(last.to_owned(), value);
}
