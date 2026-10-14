// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl codes` (Workflow #389 MCP-14, MCP-0 command table v1.1 §4.15), the CLI original of
//! MCP `ac_outcome_code`: the catalog entry of one outcome code, or one page of the code list,
//! read from the outcome catalog this build embeds (`outcome::catalog_json`). It is offline: no
//! Runtime request, no ledger record and no `--state-root` (command table D5). Both sides parse
//! their arguments into one [`Query`], read it with [`read`] and answer what it returns; stdout is
//! one line, the same JSON as the `ac_outcome_code` result, with the outcome
//! `outcome_codes_reported` (Workflow #378 A5). Only MCP cuts a list to its output budget
//! (`truncated`, command table §2.2).
//!
//! One entry is the catalog entry as the catalog holds it, with its `code` first. The list is the
//! codes in name order, each `{code, category, ladder_class, status}`, and `next_cursor` while
//! codes past the page remain. A cursor names the last code of its page and the filters it was
//! issued for, so it holds across processes, restarts and both sides.

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{CodeStr, Outcome, RawSource};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::process::ExitCode;

const COMMAND: &str = "codes";
const USAGE: &str = "usage: actingctl codes <code> | actingctl codes [--prefix <text>] [--owner <owner>] [--limit <1-100>] [--cursor <next_cursor>]";
pub(crate) const DEFAULT_LIMIT: u64 = 20;
pub(crate) const MAX_LIMIT: u64 = 100;
/// `c1.<last code of the page>.<filters>`.
const CURSOR_VERSION: &str = "c1";
/// The hex digits of the filters' SHA-256 a cursor carries.
const CURSOR_FINGERPRINT_DIGITS: usize = 16;
/// The longest code name (outcome-code writing spec §4).
const MAX_CODE_BYTES: usize = 64;

/// What one call asks for: one code's entry, or one page of the code list.
pub(crate) enum Query {
    /// `codes <code>` / `code`: the code as given.
    One(String),
    List(ListQuery),
}

/// The filters and the page of a code list (R2: each MCP argument is the CLI flag's name).
pub(crate) struct ListQuery {
    /// Only codes whose name starts with it.
    pub(crate) prefix: Option<String>,
    /// Only codes of this owner, as the entries' `owner` names it.
    pub(crate) owner: Option<String>,
    pub(crate) limit: u64,
    /// `next_cursor` of the previous page.
    pub(crate) cursor: Option<String>,
}

/// Why a lookup was refused, as its outcome.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The caller's to change: `outcome_code_unknown`, `cursor_invalid` or a usage error.
    Usage(Outcome),
    /// A build defect: `outcome_catalog_unreadable`, or `actingctl_output_failed` for an answer
    /// that does not encode.
    Defect(Outcome),
}

impl Refusal {
    pub(crate) fn outcome(&self) -> &Outcome {
        match self {
            Self::Usage(outcome) | Self::Defect(outcome) => outcome,
        }
    }

    /// `outcome_catalog_unreadable`, caused by the decoder's failure when there is one.
    fn unreadable(cause: Option<&serde_json::Error>) -> Self {
        let outcome = Outcome::new(ActingctlCode::OutcomeCatalogUnreadable);
        Self::Defect(match cause {
            Some(error) => outcome.caused_by(foreign_data_error(error)),
            None => outcome,
        })
    }

    /// `actingctl_output_failed`, caused by the encoder's failure.
    fn output_failed(error: &serde_json::Error) -> Self {
        Self::Defect(
            Outcome::new(ActingctlCode::ActingctlOutputFailed).caused_by(foreign_data_error(error)),
        )
    }

    fn cursor_invalid() -> Self {
        Self::Usage(Outcome::new(ActingctlCode::CursorInvalid))
    }
}

fn foreign_data_error(error: &serde_json::Error) -> Outcome {
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(error.to_string())
}

/// The answer of a lookup.
pub(crate) enum Answer {
    /// One entry: the catalog entry with its `code`.
    Entry(Value),
    List(Page),
}

/// The members of a catalog entry a list reads.
#[derive(Deserialize)]
struct Summary {
    category: Value,
    ladder_class: Value,
    status: Value,
    owner: Value,
}

/// One row of the list: the code and three members of its entry, as the entry has them.
#[derive(Serialize)]
struct Row {
    code: CodeStr,
    category: Value,
    ladder_class: Value,
    status: Value,
}

/// One entry as answered: its code, then the entry's members in the catalog's order.
#[derive(Serialize)]
struct EntryDocument<'c> {
    code: CodeStr,
    #[serde(flatten)]
    entry: &'c Map<String, Value>,
}

/// One page of the code list, before rendering.
pub(crate) struct Page {
    rows: Vec<Row>,
    /// The code this page started after (the cursor's), if any.
    from: Option<String>,
    /// Whether codes that match remain after the last row.
    more: bool,
    fingerprint: String,
}

impl Page {
    pub(crate) fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// `{codes, next_cursor?, truncated?}` with the first `kept` rows. With fewer rows than the
    /// page holds it is `truncated`, and next_cursor continues after the last row kept.
    pub(crate) fn render(&self, kept: usize) -> Result<Value, Refusal> {
        let kept = kept.min(self.rows.len());
        let truncated = kept < self.rows.len();
        let after = match kept.checked_sub(1) {
            Some(last) if truncated || self.more => Some(self.rows[last].code.as_str()),
            Some(_) => None,
            None if truncated => self.from.as_deref(),
            None => None,
        };
        let mut result = Map::new();
        result.insert(
            "codes".to_owned(),
            serde_json::to_value(&self.rows[..kept])
                .map_err(|error| Refusal::output_failed(&error))?,
        );
        if truncated || self.more {
            let cursor = match after {
                Some(after) => format!("{CURSOR_VERSION}.{after}.{}", self.fingerprint),
                // A page cut to nothing at the start of the list continues from its start.
                None => format!("{CURSOR_VERSION}..{}", self.fingerprint),
            };
            result.insert("next_cursor".to_owned(), Value::String(cursor));
        }
        if truncated {
            result.insert("truncated".to_owned(), Value::Bool(true));
        }
        Ok(Value::Object(result))
    }
}

/// Reads `query` from the catalog this build embeds.
pub(crate) fn read_embedded(query: &Query) -> Result<Answer, Refusal> {
    let catalog = actingcommand_contract::outcome::catalog_json()
        .map_err(|error| Refusal::unreadable(Some(error)))?;
    read(catalog, query)
}

/// Reads `query` from `catalog` (a parsed `actingcommand.outcome-codes.v1` document). A code it
/// does not list is `outcome_code_unknown` with the code as given (`code_name`, when it is a
/// word a name may hold); a cursor that is malformed or was issued for other filters is
/// `cursor_invalid`; a catalog without its codes table, or with an entry that is not an object
/// or lacks a member the answer reads, is `outcome_catalog_unreadable`.
pub(crate) fn read(catalog: &Value, query: &Query) -> Result<Answer, Refusal> {
    let codes = catalog
        .get("codes")
        .and_then(Value::as_object)
        .ok_or_else(|| Refusal::unreadable(None))?;
    match query {
        Query::One(spelling) => {
            let Some(entry) = codes.get(spelling) else {
                let unknown = Outcome::new(ActingctlCode::OutcomeCodeUnknown);
                return Err(Refusal::Usage(if names_a_word(spelling) {
                    unknown.code_name(spelling.as_str())
                } else {
                    unknown
                }));
            };
            let entry = entry.as_object().ok_or_else(|| Refusal::unreadable(None))?;
            let document = EntryDocument {
                code: received(spelling)?,
                entry,
            };
            serde_json::to_value(&document)
                .map(Answer::Entry)
                .map_err(|error| Refusal::output_failed(&error))
        }
        Query::List(list) => page(codes, list).map(Answer::List),
    }
}

/// A word a `name` value may hold: at most 256 bytes and no control character.
fn names_a_word(text: &str) -> bool {
    text.len() <= 256 && !text.chars().any(char::is_control)
}

/// A catalog code name as a received code (`CodeStr` is built only by deserialising).
fn received(name: &str) -> Result<CodeStr, Refusal> {
    serde_json::from_value(Value::String(name.to_owned()))
        .map_err(|error| Refusal::unreadable(Some(&error)))
}

fn page(codes: &Map<String, Value>, query: &ListQuery) -> Result<Page, Refusal> {
    let fingerprint = fingerprint(query);
    let from = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, &fingerprint))
        .transpose()?
        .flatten();
    let mut names = codes
        .keys()
        .filter(|name| {
            query
                .prefix
                .as_deref()
                .is_none_or(|prefix| name.starts_with(prefix))
                && from.as_deref().is_none_or(|from| name.as_str() > from)
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    let limit = usize::try_from(query.limit).unwrap_or(usize::MAX);
    let mut rows = Vec::new();
    let mut more = false;
    for name in names {
        let summary = Summary::deserialize(&codes[name.as_str()])
            .map_err(|error| Refusal::unreadable(Some(&error)))?;
        if let Some(owner) = &query.owner {
            let Some(entry_owner) = summary.owner.as_str() else {
                return Err(Refusal::unreadable(None));
            };
            if entry_owner != owner {
                continue;
            }
        }
        if rows.len() == limit {
            more = true;
            break;
        }
        rows.push(Row {
            code: received(name)?,
            category: summary.category,
            ladder_class: summary.ladder_class,
            status: summary.status,
        });
    }
    Ok(Page {
        rows,
        from,
        more,
        fingerprint,
    })
}

/// The filters a cursor is issued for.
fn fingerprint(query: &ListQuery) -> String {
    let canonical = json!([query.prefix, query.owner]).to_string();
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..CURSOR_FINGERPRINT_DIGITS]
        .to_owned()
}

/// The code a cursor's page ended with (`None`: the start of the list), when the cursor is well
/// formed and was issued for `fingerprint`; else `cursor_invalid`.
fn decode_cursor(cursor: &str, fingerprint: &str) -> Result<Option<String>, Refusal> {
    let parts = cursor.split('.').collect::<Vec<_>>();
    let [version, after, issued_for] = parts.as_slice() else {
        return Err(Refusal::cursor_invalid());
    };
    if *version != CURSOR_VERSION
        || *issued_for != fingerprint
        || after.len() > MAX_CODE_BYTES
        || !after
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(Refusal::cursor_invalid());
    }
    Ok((!after.is_empty()).then(|| (*after).to_owned()))
}

/// Runs `codes` when the first argument names it; `None` leaves every other command to the CLI.
/// It takes no `--state-root`: it never connects to the Runtime (command table D5). One result
/// line on stdout (Workflow #378 A5): the entry or the page with `outcome_codes_reported`, or
/// `{outcome}` with the refusal (a usage refusal's developer line adds the usage) and exit 1.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    Some(match run(&arguments[1..]) {
        Ok(document) => cli_line::success(
            cli_line::members(document, COMMAND),
            &Outcome::new(ActingctlCode::OutcomeCodesReported),
        ),
        Err(refusal) if refusal.outcome().is(ActingctlCode::ActingctlUsageInvalid) => {
            cli_line::failure(refusal.outcome(), Some(USAGE))
        }
        Err(refusal) => cli_line::failure(refusal.outcome(), None),
    })
}

fn run(arguments: &[OsString]) -> Result<Value, Refusal> {
    match read_embedded(&parse(arguments)?)? {
        Answer::Entry(document) => Ok(document),
        // The CLI has no output budget: the whole page.
        Answer::List(page) => page.render(page.row_count()),
    }
}

fn usage(argument: Option<&str>, problem: ArgumentProblem) -> Refusal {
    Refusal::Usage(cli_line::usage_invalid(argument, problem))
}

/// `codes <code>`, or the list flags `flags_of` names for `codes`, each at most once and with a
/// value. The code is the first word, and nothing else may follow it. Any other word, a repeated
/// flag, a malformed value or a code with a list flag is `actingctl_usage_invalid` with the word
/// (`argument`) and what is wrong with it (`argument_problem`).
fn parse(arguments: &[OsString]) -> Result<Query, Refusal> {
    let takes = crate::flags_of(COMMAND, None).unwrap_or_default();
    let (spelling, flags) = match arguments.first().map(|word| word.to_str()) {
        Some(None) => return Err(usage(None, ArgumentProblem::NotUtf8)),
        Some(Some(word)) if !word.starts_with("--") => (Some(word.to_owned()), &arguments[1..]),
        _ => (None, arguments),
    };
    let mut words = flags.iter();
    let mut given: Vec<(&str, String)> = Vec::new();
    while let Some(word) = words.next() {
        let Some(word) = word.to_str() else {
            return Err(usage(None, ArgumentProblem::NotUtf8));
        };
        let Some(flag) = takes.iter().copied().find(|flag| *flag == word) else {
            return Err(usage(Some(word), ArgumentProblem::Unknown));
        };
        if spelling.is_some() {
            return Err(usage(Some(flag), ArgumentProblem::Conflicting));
        }
        if given.iter().any(|(name, _)| *name == flag) {
            return Err(usage(Some(flag), ArgumentProblem::Repeated));
        }
        let value = words
            .next()
            .ok_or_else(|| usage(Some(flag), ArgumentProblem::ValueMissing))?
            .to_str()
            .ok_or_else(|| usage(Some(flag), ArgumentProblem::NotUtf8))?;
        if value.trim().is_empty() {
            return Err(usage(Some(flag), ArgumentProblem::ValueMissing));
        }
        given.push((flag, value.to_owned()));
    }
    if let Some(spelling) = spelling {
        return Ok(Query::One(spelling));
    }
    let mut value = |flag: &str| {
        given
            .iter()
            .position(|(name, _)| *name == flag)
            .map(|index| given.swap_remove(index).1)
    };
    let limit = match value("--limit") {
        None => DEFAULT_LIMIT,
        Some(text) => match text.parse::<u64>() {
            Ok(limit) if (1..=MAX_LIMIT).contains(&limit) => limit,
            Ok(_) => {
                return Err(Refusal::Usage(
                    cli_line::usage_invalid(Some("--limit"), ArgumentProblem::OutOfRange)
                        .range_min(1_u64)
                        .range_max(MAX_LIMIT),
                ));
            }
            Err(_) => return Err(usage(Some("--limit"), ArgumentProblem::WrongType)),
        },
    };
    Ok(Query::List(ListQuery {
        prefix: value("--prefix"),
        owner: value("--owner"),
        limit,
        cursor: value("--cursor"),
    }))
}

// Test-only: Workflow #389 MCP-14, the refusals of a lookup (command table §4.15, §5).
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::outcome::Category;

    // A catalog with two codes of one owner.
    fn catalog() -> Value {
        let entry = |category: &str| {
            let mut entry = Map::new();
            for (member, value) in [
                ("category", category),
                ("ladder_class", "none"),
                ("owner", "neutral-owner"),
                ("status", "active"),
            ] {
                entry.insert(member.to_owned(), Value::String(value.to_owned()));
            }
            Value::Object(entry)
        };
        let mut codes = Map::new();
        codes.insert("neutral_first_done".to_owned(), entry("success"));
        codes.insert("neutral_second_failed".to_owned(), entry("error"));
        let mut catalog = Map::new();
        catalog.insert("codes".to_owned(), Value::Object(codes));
        Value::Object(catalog)
    }

    fn list(prefix: Option<&str>, cursor: Option<&str>) -> Query {
        Query::List(ListQuery {
            prefix: prefix.map(str::to_owned),
            owner: None,
            limit: 1,
            cursor: cursor.map(str::to_owned),
        })
    }

    #[track_caller]
    fn refused(answer: Result<Answer, Refusal>) -> Refusal {
        match answer {
            Ok(_) => panic!("the lookup answered where a refusal is required"),
            Err(refusal) => refusal,
        }
    }

    #[test]
    fn each_refusal_of_a_code_lookup_carries_its_registered_code() {
        let catalog = catalog();

        // Command table §4.15: a code the catalog does not list is outcome_code_unknown, an
        // error, with the code as given.
        let unknown = refused(read(
            &catalog,
            &Query::One("neutral_third_failed".to_owned()),
        ));
        assert!(matches!(unknown, Refusal::Usage(_)), "{unknown:?}");
        let unknown = unknown.outcome();
        assert!(unknown.is(ActingctlCode::OutcomeCodeUnknown), "{unknown}");
        assert_eq!(unknown.category(), Category::Error);
        assert_eq!(
            unknown.values().get("code_name"),
            Some(&Value::String("neutral_third_failed".to_owned()))
        );

        // Command table §2.2: a cursor continues only the list it was issued for; another
        // prefix, or a cursor that is not one, is cursor_invalid.
        let first = match read(&catalog, &list(Some("neutral_"), None)) {
            Ok(Answer::List(page)) => page.render(page.row_count()).expect("render the page"),
            Ok(Answer::Entry(entry)) => panic!("a list query answered an entry: {entry}"),
            Err(refusal) => panic!("the first page was refused: {refusal:?}"),
        };
        let cursor = first["next_cursor"].as_str().unwrap_or_else(|| {
            panic!("a first page of one of two codes without next_cursor: {first}")
        });
        for query in [
            list(Some("neutral_s"), Some(cursor)),
            list(Some("neutral_"), Some("neutral_first_done")),
        ] {
            let invalid = refused(read(&catalog, &query));
            assert!(matches!(invalid, Refusal::Usage(_)), "{invalid:?}");
            assert!(
                invalid.outcome().is(ActingctlCode::CursorInvalid),
                "{}",
                invalid.outcome()
            );
            assert_eq!(invalid.outcome().category(), Category::Error);
        }

        // Command table §5: a catalog without its codes table, or one whose entry lacks a member
        // a list reads, is outcome_catalog_unreadable, an error; the decoder's failure is its
        // cause when there is one.
        let mut lacking = catalog.clone();
        if let Some(entry) = lacking["codes"]["neutral_first_done"].as_object_mut() {
            entry.remove("status");
        }
        for (document, caused) in [(Value::Object(Map::new()), false), (lacking, true)] {
            let unreadable = refused(read(&document, &list(None, None)));
            assert!(matches!(unreadable, Refusal::Defect(_)), "{unreadable:?}");
            let unreadable = unreadable.outcome();
            assert!(
                unreadable.is(ActingctlCode::OutcomeCatalogUnreadable),
                "{unreadable}"
            );
            assert_eq!(unreadable.category(), Category::Error);
            assert_eq!(
                unreadable
                    .causes()
                    .iter()
                    .any(|link| link.outcome().is(ContractCode::ForeignDataError)),
                caused,
                "{unreadable}"
            );
        }
    }
}
