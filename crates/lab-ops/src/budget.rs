// SPDX-License-Identifier: AGPL-3.0-only

//! The answer budget (Workflow #434 LAB-5D; `contracts/lab-two-entries.md` §9.3; register §1.287,
//! "Slice LAB-5D"): what a caller passes to say that an operation's answer must fit, and where an
//! answer that does not fit is written whole instead. The budget is the caller's: a read cuts or
//! exports its answer only when its caller passes one, and a caller that passes none (the CLI,
//! `serve --stdio`) gets the whole answer.
//!
//! Each read that cuts or exports takes the budget in its own module (`within_budget`). `status`
//! (`overview`), `diagnose`, `events`, `schedule`, `runs` (`run_list`), `run evidence`
//! (`run_evidence`) and `codes` (`code_lookup`) cut their answer, each in its own order and with
//! its own markers; `facts`, `packs`, `remedy` and `summary` (`routine_summary`) write the whole
//! answer through the caller's export and answer its header beside the export reference, one way
//! for the four ([`exported`]), as the payload page of `events` does with its own header. MCP's
//! server (`actingctl mcp-serve`) passes its measure of an answer as it sends it and its export
//! directory; that measure, and the cursors bound to its connection, stay its own.

use crate::codes::ActingctlCode;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Outcome, RawSource};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// The caller's export writer: `bytes`, whose SHA-256 is `sha256` (`sha256:<hex>`), written whole
/// as `<hex>.<extension>` where the caller keeps its exports. It answers the export reference
/// `{path, sha256, size}`, or the outcome of a write that failed.
pub type Writer<'a> = &'a dyn Fn(&str, &str, &[u8]) -> Result<Value, Outcome>;

/// What a caller passes to say "the answer must fit".
#[derive(Clone, Copy)]
pub struct Budget<'a> {
    /// Whether an answer fits: the caller's measure of an answer as it sends it.
    pub fits: &'a dyn Fn(&Value) -> bool,
    /// Where a read that exports writes an answer that does not fit, whole. `None`: the caller has
    /// no place for it, and such an answer is given whole, for the caller to refuse.
    pub export: Option<Writer<'a>>,
}

impl<'a> Budget<'a> {
    /// The budget `fits` measures, without an export.
    pub fn new(fits: &'a dyn Fn(&Value) -> bool) -> Self {
        Self { fits, export: None }
    }

    /// This budget, a read that exports writing an answer that does not fit through `export`.
    pub fn with_export(self, export: Writer<'a>) -> Self {
        Self {
            fits: self.fits,
            export: Some(export),
        }
    }
}

/// Whether `answer` is within `budget`: every answer is, without one.
pub fn fits(budget: Option<&Budget<'_>>, answer: &Value) -> bool {
    budget.is_none_or(|budget| (budget.fits)(answer))
}

/// An answer given within a caller's budget.
#[derive(Debug)]
pub struct Fitted {
    /// The answer, or the header of one written whole to the caller's export, with `export`.
    pub result: Value,
    /// `answer_exported` naming the file (`path`) when the answer was written whole: the caller
    /// gives it beside the header as its warning, and the file holds the rest.
    pub exported: Option<Outcome>,
}

impl Fitted {
    /// `result`, given whole.
    pub fn whole(result: Value) -> Self {
        Self {
            result,
            exported: None,
        }
    }
}

/// The writer `answer` is written through: none without a budget, for an answer the budget takes,
/// and for a caller with no export.
pub fn export_for<'a>(budget: Option<&Budget<'a>>, answer: &Value) -> Option<Writer<'a>> {
    let budget = budget?;
    if (budget.fits)(answer) {
        return None;
    }
    budget.export
}

/// `answer` as JSON, written whole through `export` as `<its sha256>.json`. An answer that does
/// not encode is `material_export_failed`, caused by the encoder's failure; a write that fails is
/// the writer's outcome.
pub fn write_json(export: Writer<'_>, answer: &Value) -> Result<Value, Outcome> {
    let bytes = serde_json::to_vec(answer).map_err(|error| {
        Outcome::new(ActingctlCode::MaterialExportFailed).caused_by(
            Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Json)
                .raw_text(error.to_string()),
        )
    })?;
    write_bytes(export, &bytes)
}

/// `bytes`, a whole answer's JSON, written through `export` as `<their sha256>.json`.
pub fn write_bytes(export: Writer<'_>, bytes: &[u8]) -> Result<Value, Outcome> {
    export(
        &format!("sha256:{:x}", Sha256::digest(bytes)),
        "json",
        bytes,
    )
}

/// The header of `answer`, written whole through the caller's export: `header`'s members of
/// `answer`, in that order and each when `answer` has it, then `export`, the writer's
/// `reference`; and `note` (`answer_exported`, with what the read adds to it) naming the file
/// (`path`).
pub fn exported_header(answer: &Value, header: &[&str], reference: Value, note: Outcome) -> Fitted {
    let mut result = header
        .iter()
        .filter_map(|member| {
            answer
                .get(*member)
                .map(|value| ((*member).to_owned(), value.clone()))
        })
        .collect::<Map<_, _>>();
    let note = match reference["path"].as_str() {
        Some(path) => note.path(path),
        None => note,
    };
    result.insert("export".to_owned(), reference);
    Fitted {
        result: Value::Object(result),
        exported: Some(note),
    }
}

/// The one way of the reads that export (`facts`, `remedy`, `summary`; `packs` writes its whole
/// document the same way): `answer` whole without a budget, when the budget takes it, or for a
/// caller with no export; otherwise written whole as JSON through the caller's export
/// ([`write_json`]) and answered as its header ([`exported_header`]) with the warning
/// `answer_exported`.
pub fn exported(
    answer: Value,
    header: &[&str],
    budget: Option<&Budget<'_>>,
) -> Result<Fitted, Outcome> {
    let Some(export) = export_for(budget, &answer) else {
        return Ok(Fitted::whole(answer));
    };
    let reference = write_json(export, &answer)?;
    Ok(exported_header(
        &answer,
        header,
        reference,
        Outcome::new(ActingctlCode::AnswerExported),
    ))
}
