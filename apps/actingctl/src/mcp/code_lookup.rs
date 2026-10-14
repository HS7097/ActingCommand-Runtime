// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_outcome_code` (Workflow #389 MCP-14, MCP-0 command table v1.1 §4.15): `actingctl codes`
//! over MCP. Its arguments are the CLI's flags (R2; `code` is the CLI's positional word) and its
//! result is the CLI's stdout JSON (`crate::code_lookup`), read offline from the outcome catalog
//! this build embeds. A list is cut from its last row to the output budget: `truncated`, with
//! next_cursor after the last row kept (§2.2); an entry over the budget answers
//! `output_budget_exceeded` as every tool does. Before U-3 an observer tool, after it a general
//! read (command table §3).

use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::code_lookup::{self as lookup, Answer, ListQuery, Page, Query, Refusal};
use crate::codes::ArgumentProblem;
use serde_json::{Map, Value, json};

pub(super) const DESCRIPTION: &str = "The catalog entry of one outcome code, or one page of the code list, read from the outcome catalog this build embeds, as actingctl codes answers them: offline, no Runtime request and no ledger record. Look up here what a code in an outcome means. With code: the entry as the catalog holds it, with code first: category, ladder_class, owner, layer, uncertain, values (each key required or optional), review, status, since, description, common_causes, handling, and replaced_by and retired_in when the entry has them; the text is English, written for maintainers, and TBD where a pending entry has none yet. A code the catalog does not list answers outcome_code_unknown. Without code: the codes in name order as {codes: [{code, category, ladder_class, status}], next_cursor}; prefix keeps the codes whose name starts with it, owner the codes of one owner as the entries name it (actingctl, runtime-host, ...). limit defaults to 20 (at most 100). next_cursor is present while codes past the page remain: send it back with the same prefix and owner to read on (another prefix or owner answers cursor_invalid); it does not expire. truncated true means the page was cut to the output budget, and next_cursor continues after its last code. code excludes prefix, owner, limit and cursor (arguments_invalid). A catalog that cannot be read answers outcome_catalog_unreadable.";

/// The arguments of a list; `code` excludes them.
const LIST_ARGUMENTS: [&str; 4] = ["prefix", "owner", "limit", "cursor"];

pub(super) fn run(_context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["code", "prefix", "owner", "limit", "cursor"])?;
    let query = match arguments.string("code")? {
        Some(spelling) => {
            if let Some(list_argument) = LIST_ARGUMENTS.iter().find(|name| arguments.has(name)) {
                return Err(invalid_argument(
                    list_argument,
                    ArgumentProblem::Conflicting,
                ));
            }
            Query::One(spelling.to_owned())
        }
        None => Query::List(ListQuery {
            prefix: arguments.string("prefix")?.map(str::to_owned),
            owner: arguments.string("owner")?.map(str::to_owned),
            limit: arguments
                .integer("limit", 1, lookup::MAX_LIMIT)?
                .unwrap_or(lookup::DEFAULT_LIMIT),
            cursor: arguments.string("cursor")?.map(str::to_owned),
        }),
    };
    match lookup::read_embedded(&query).map_err(refused)? {
        Answer::Entry(document) => Ok(ToolSuccess::new(document)),
        Answer::List(page) => within_budget(&page),
    }
}

/// The page with as many of its rows, from the first, as the output budget lets through.
fn within_budget(page: &Page) -> ToolOutcome {
    let mut kept = page.row_count();
    loop {
        let result = page.render(kept).map_err(refused)?;
        // A result that does not fit even without rows answers output_budget_exceeded.
        if kept == 0 || tools::fits_success(&result) {
            return Ok(ToolSuccess::new(result));
        }
        kept -= 1;
    }
}

/// A refusal of the lookup with the CLI's outcome: the caller's to change is `usage`, a build
/// defect `runtime`.
fn refused(refusal: Refusal) -> ToolError {
    match refusal {
        Refusal::Usage(outcome) => ToolError::outcome("usage", outcome),
        Refusal::Defect(outcome) => ToolError::outcome("runtime", outcome),
    }
}

pub(super) fn input_schema() -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "prefix": {
                "type": "string",
                "minLength": 1,
                "description": "Only codes whose name starts with it.",
            },
            "owner": {
                "type": "string",
                "minLength": 1,
                "description": "Only codes of this owner, as the entries name it.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": lookup::MAX_LIMIT,
                "default": lookup::DEFAULT_LIMIT,
            },
            "cursor": {
                "type": "string",
                "minLength": 1,
                "description": "next_cursor of the previous page, sent with the same prefix and owner.",
            },
        },
        "additionalProperties": false,
    });
    schema["properties"]["code"] = json!({
        "type": "string",
        "minLength": 1,
        "description": "One code: its catalog entry. Without it, the code list.",
    });
    schema
}

pub(super) fn result_schema() -> Value {
    let text = json!({"type": "string"});
    let mut row = json!({
        "type": "object",
        "properties": {"ladder_class": text, "status": text},
    });
    row["properties"]["code"] = text.clone();
    row["properties"]["category"] = text.clone();
    row["required"] = json!(["code", "category", "ladder_class", "status"]);
    let mut result = json!({
        "type": "object",
        "properties": {
            "ladder_class": text,
            "owner": text,
            "layer": text,
            "uncertain": {"type": "boolean"},
            "values": {"type": "object"},
            "review": text,
            "status": text,
            "since": text,
            "description": text,
            "common_causes": text,
            "handling": text,
            "replaced_by": text,
            "retired_in": text,
            "codes": {"type": "array", "items": row},
            "next_cursor": text,
            "truncated": {"type": "boolean"},
        },
    });
    result["properties"]["code"] = text.clone();
    result["properties"]["category"] = text;
    result
}

// Test-only: Workflow #389 MCP-14, the output budget of `ac_outcome_code` (command table §2.2).
#[cfg(test)]
mod tests {
    use super::*;

    // As many codes as one page holds at most, with names as long as the catalog's longer ones.
    const CODES: usize = 100;

    fn catalog() -> Value {
        let mut codes = Map::new();
        for index in 0..CODES {
            let mut entry = Map::new();
            for (member, value) in [
                ("category", "error"),
                ("ladder_class", "none"),
                ("owner", "neutral-owner"),
                ("status", "active"),
            ] {
                entry.insert(member.to_owned(), Value::String(value.to_owned()));
            }
            codes.insert(
                format!("neutral_launcher_writer_lock_probe_{index:03}_failed"),
                Value::Object(entry),
            );
        }
        let mut catalog = Map::new();
        catalog.insert("codes".to_owned(), Value::Object(codes));
        Value::Object(catalog)
    }

    fn page(catalog: &Value, cursor: Option<&str>) -> Page {
        let query = Query::List(ListQuery {
            prefix: None,
            owner: None,
            limit: lookup::MAX_LIMIT,
            cursor: cursor.map(str::to_owned),
        });
        match lookup::read(catalog, &query) {
            Ok(Answer::List(page)) => page,
            Ok(Answer::Entry(entry)) => panic!("a list query answered an entry: {entry}"),
            Err(refusal) => panic!("the list was refused: {refusal:?}"),
        }
    }

    fn names(result: &Value) -> Vec<String> {
        result["codes"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["code"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_list_over_the_output_budget_is_cut_with_truncated_and_read_on_from_its_cursor() {
        let catalog = catalog();
        let whole = names(
            &page(&catalog, None)
                .render(CODES)
                .expect("render the whole page"),
        );
        assert_eq!(whole.len(), CODES);

        // Command table §2.2: a list over the output budget keeps the rows that fit, from the
        // first, and is marked truncated with next_cursor after the last row kept.
        let first = within_budget(&page(&catalog, None))
            .unwrap_or_else(|error| panic!("ac_outcome_code refused: {}", error.to_value()));
        assert!(tools::fits_success(&first.result), "{}", first.result);
        assert_eq!(first.result["truncated"], json!(true), "{}", first.result);
        let kept = names(&first.result);
        assert!(
            !kept.is_empty() && kept.len() < CODES,
            "kept {} of {CODES}",
            kept.len()
        );
        assert_eq!(kept, whole[..kept.len()]);

        // The cursor reads on from the first row not kept: no row is lost or repeated.
        let cursor = first.result["next_cursor"]
            .as_str()
            .unwrap_or_else(|| panic!("a truncated page without next_cursor: {}", first.result));
        let rest = page(&catalog, Some(cursor));
        let rest = rest.render(rest.row_count()).expect("render the rest");
        assert_eq!(names(&rest), whole[kept.len()..]);
        assert_eq!(rest.get("next_cursor"), None, "{rest}");
    }
}
