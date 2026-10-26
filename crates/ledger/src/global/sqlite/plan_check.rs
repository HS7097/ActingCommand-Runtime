// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c F6 (rd-1026/ln7c/final-query.md §1.3): the plan check of a ledger statement, for
//! ledger tests. The statement's plan (`EXPLAIN QUERY PLAN`, its parameters unbound) is taken on
//! an empty database built as a new root's is (`schema.sql`, then the views and their indexes),
//! with no statistics, and held to the rule:
//! - no `SCAN` of a ledger table, of an alias the statement gives one, or of an index of one, and
//!   no automatic index on one (SQLite builds it by reading the whole table);
//! - the event type index only with a sequence bound (`event_type=? AND sequence>?`), or as the
//!   one-entry `MAX` seek on its covering form (`event_type=?`);
//! - never the severity, source, module or sensitivity index.
//!
//! A `SCAN` of a `json_each` parameter, a materialized CTE, a subquery or a constant row passes.
//! SQLite names a table by its alias in a plan, so the aliases are read from the statement's
//! text; an alias given to a ledger table must not also name a CTE of the statement.

use super::*;
use std::collections::BTreeMap;

/// The indexes whose every entry range is history-wide.
const HISTORY_WIDE_INDEXES: [&str; 4] = [
    "ledger_view_index_severity_v1",
    "ledger_view_index_source_v1",
    "ledger_view_index_module_v1",
    "ledger_events_sensitivity",
];
/// The words that may follow a table's name in a statement and are not an alias.
const KEYWORDS: &str = "and as cross default except from full group having indexed inner intersect \
    join left limit natural not on or order outer returning right select set union using values \
    where window";

/// A statement's plan on the empty database, one line per step indented two spaces per level,
/// and the rule's violations, one per offending step.
#[derive(Debug)]
pub(crate) struct PlanCheck {
    pub(crate) plan: Vec<String>,
    pub(crate) violations: Vec<String>,
}

/// An empty ledger database as a new root has it, in memory: `schema.sql`, then the views and
/// their five indexes.
pub(crate) fn empty_database() -> Connection {
    let connection = Connection::open_in_memory().expect("an in-memory database");
    connection
        .execute_batch(include_str!("schema.sql"))
        .expect("the ledger schema");
    views::deploy(&connection).expect("the ledger views and their indexes");
    connection
}

/// The plan of `sql` on an empty ledger database, held to the rule.
pub(crate) fn check_plan(sql: &str) -> PlanCheck {
    let connection = empty_database();
    let tables = schema_names(
        &connection,
        "SELECT lower(name) FROM sqlite_schema WHERE type = 'table' AND name GLOB 'ledger_*'",
    );
    let indexes = schema_names(
        &connection,
        "SELECT lower(name) FROM sqlite_schema WHERE type = 'index' AND tbl_name GLOB 'ledger_*'",
    );
    let mut ledger = aliases(sql, &tables);
    ledger.extend(tables);
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare the statement's plan");
    let mut rows = statement.raw_query();
    let mut depths = BTreeMap::new();
    let mut check = PlanCheck {
        plan: Vec::new(),
        violations: Vec::new(),
    };
    while let Some(row) = rows.next().expect("read the statement's plan") {
        let id: i64 = row.get(0).expect("a plan step's id");
        let parent: i64 = row.get(1).expect("a plan step's parent");
        let detail: String = row.get(3).expect("a plan step's detail");
        let depth = depths.get(&parent).map_or(0, |depth| depth + 1);
        depths.insert(id, depth);
        check.plan.push(format!("{}{detail}", "  ".repeat(depth)));
        if let Some(rule) = violation(&detail, &ledger, &indexes) {
            check.violations.push(format!("{rule}: {detail}"));
        }
    }
    check
}

fn schema_names(connection: &Connection, sql: &str) -> BTreeSet<String> {
    let mut statement = connection.prepare(sql).expect("the schema's statement");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("the schema's names")
        .collect::<rusqlite::Result<_>>()
        .expect("read the schema's names")
}

/// The rule a plan step breaks, if any. `ledger` holds the ledger tables and the aliases the
/// statement gives them, `indexes` the indexes of the ledger tables, all in lower case.
fn violation(
    detail: &str,
    ledger: &BTreeSet<String>,
    indexes: &BTreeSet<String>,
) -> Option<&'static str> {
    if HISTORY_WIDE_INDEXES
        .iter()
        .any(|index| detail.contains(index))
    {
        return Some("a history-wide index");
    }
    let mut words = detail.split(' ');
    let access = words.next()?;
    // `SCAN a`, `SEARCH a USING ...`, or `SCAN ledger_artifacts AS a`.
    let mut named: Vec<String> = words
        .next()
        .map(str::to_ascii_lowercase)
        .into_iter()
        .collect();
    if words.next() == Some("AS") {
        named.extend(words.next().map(str::to_ascii_lowercase));
    }
    let of_ledger = named.iter().any(|name| ledger.contains(name));
    let index = ["USING INDEX ", "USING COVERING INDEX "]
        .iter()
        .find_map(|marker| detail.split_once(marker))
        .and_then(|(_, rest)| rest.split(' ').next())
        .map(str::to_ascii_lowercase);
    let type_index = detail.contains("ledger_view_index_type_v1")
        && !detail.contains("(event_type=? AND sequence>")
        && !detail.contains("USING COVERING INDEX ledger_view_index_type_v1 (event_type=?)");
    match access {
        "SCAN" if detail == "SCAN CONSTANT ROW" || detail.contains(" VIRTUAL TABLE ") => None,
        "SCAN" if of_ledger || index.as_ref().is_some_and(|index| indexes.contains(index)) => {
            Some("a full scan of a ledger table or index")
        }
        "SEARCH" if of_ledger && detail.contains(" USING AUTOMATIC ") => {
            Some("an automatic index on a ledger table")
        }
        "SEARCH" if type_index => Some("the event type index without a sequence bound"),
        _ => None,
    }
}

/// The aliases `sql` gives the tables in `tables`: the word after a table's name, or after its
/// `AS`, unless it is a keyword.
fn aliases(sql: &str, tables: &BTreeSet<String>) -> BTreeSet<String> {
    let words = words(sql);
    let mut aliases = BTreeSet::new();
    for (at, word) in words.iter().enumerate() {
        if !tables.contains(&word.to_ascii_lowercase()) {
            continue;
        }
        let mut next = at + 1;
        if words
            .get(next)
            .is_some_and(|word| word.eq_ignore_ascii_case("as"))
        {
            next += 1;
        }
        if let Some(alias) = words.get(next).map(|word| word.to_ascii_lowercase())
            && alias.starts_with(|first: char| first.is_ascii_alphabetic() || first == '_')
            && !KEYWORDS.split_whitespace().any(|keyword| keyword == alias)
        {
            aliases.insert(alias);
        }
    }
    aliases
}

/// The words and ASCII punctuation of `sql`, without its string literals and `--` comments.
fn words(sql: &str) -> Vec<&str> {
    let bytes = sql.as_bytes();
    let mut words = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'\'' {
            // A doubled quote inside a literal reads as two literals, both skipped.
            at = sql[at + 1..]
                .find('\'')
                .map_or(bytes.len(), |end| at + end + 2);
        } else if bytes[at..].starts_with(b"--") {
            at = sql[at..].find('\n').map_or(bytes.len(), |end| at + end + 1);
        } else if byte.is_ascii_alphanumeric() || byte == b'_' {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            words.push(&sql[start..at]);
        } else {
            if byte.is_ascii_punctuation() {
                words.push(&sql[at..=at]);
            }
            at += 1;
        }
    }
    words
}
