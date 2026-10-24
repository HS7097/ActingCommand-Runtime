// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-1 (model M-357a v1.1 §2; register §1.65 TB1): the shared tables of a
//! standard-pack game root, `<standard_packs.root>/<game>/tables/<table_id>.json`
//! (`actingcommand.table.v1`). A table holds final values only: typed rows under a row key,
//! with four column types (`key`, `integer`, `boolean`, `enum_string`), nullable columns and a
//! per-column expiry. Several packages and the scheduler share one table, so it sits in the game
//! root, never in a package.
//!
//! One structure check, three readers (§2.3): the Runtime at startup, `actingd check-config`
//! (and `actingctl packs` / `ac_packs`), and `actinglab resource validate` all call
//! [`parse_standard_pack_table`]. A refused table is `standard_pack_table_invalid` with its
//! `standard_pack_problem` and JSON pointer; one that cannot be read is
//! `standard_pack_table_unreadable` with the OS error as the cause. A refused table never breaks
//! its game root, and the tables of a broken root are not read.
//!
//! The Runtime names no game, table, column or value here: they are opaque data of the resource
//! side.

use crate::canonical::parse_canonical_json;
use crate::codes::SharedCode;
use crate::outcome::{IoOp, Outcome};
use crate::{StandardPackProblem, standard_pack_page_id};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The reserved directory of a game root that holds its tables (§2.1).
pub const STANDARD_PACK_TABLES_DIRECTORY: &str = "tables";
/// The file name suffix of a table file: `tables/<table_id>.json`.
pub const STANDARD_PACK_TABLE_SUFFIX: &str = ".json";
/// The largest table file (§2.2).
pub const MAX_STANDARD_PACK_TABLE_BYTES: usize = 2 * 1024 * 1024;
/// The most rows one table holds (§2.2).
pub const MAX_STANDARD_PACK_TABLE_ROWS: usize = 8192;
/// The fewest columns one table declares (§2.2).
pub const MIN_STANDARD_PACK_TABLE_COLUMNS: usize = 2;
/// The most columns one table declares (§2.2).
pub const MAX_STANDARD_PACK_TABLE_COLUMNS: usize = 32;
/// The most columns of one row key (§2.2).
pub const MAX_STANDARD_PACK_TABLE_ROW_KEY: usize = 8;
/// The most values one `enum_string` column allows (§2.2).
pub const MAX_STANDARD_PACK_TABLE_ENUM_VALUES: usize = 1024;
/// The longest allowed value of an `enum_string` column (§2.2).
pub const MAX_STANDARD_PACK_TABLE_ENUM_VALUE_BYTES: usize = 128;
/// The most tables of one game root that are read; by file name, each later one is
/// `table_too_large` (§2.2).
pub const MAX_STANDARD_PACK_TABLES: usize = 64;

/// The one table file version this build reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StandardPackTableVersion {
    #[serde(rename = "actingcommand.table.v1")]
    V1,
}

/// The type of one table column (§2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandardPackTableColumnType {
    /// The one key column, `row_key[0]`: non-empty strings of the business identity grammar.
    Key,
    /// Integers within `[min, max]`.
    Integer { min: i64, max: i64 },
    /// `true` or `false`.
    Boolean,
    /// One of `allowed`.
    EnumString { allowed: Vec<String> },
}

/// One declared column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackTableColumn {
    pub name: String,
    pub column_type: StandardPackTableColumnType,
    /// A nullable column's cell may be `null`: required data is missing, nothing may be chosen
    /// by it.
    pub nullable: bool,
    /// After this instant (Unix seconds) every cell of the (nullable) column reads `null`.
    pub valid_until_unix_s: Option<i64>,
}

/// One cell of a row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StandardPackTableCell {
    Null,
    Integer(i64),
    Boolean(bool),
    /// A `key` or `enum_string` cell.
    Text(String),
}

/// One table as read and checked (§2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackTable {
    pub game: String,
    pub table_id: String,
    pub row_key: Vec<String>,
    pub columns: Vec<StandardPackTableColumn>,
    /// Each row's cells, in the order of `columns`.
    pub rows: Vec<Vec<StandardPackTableCell>>,
}

impl StandardPackTable {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The columns whose `valid_until_unix_s` has passed at `now_unix_ms`: from the second after
    /// it on, the column reads `null` (§2.2, D25).
    pub fn expired_columns(&self, now_unix_ms: u64) -> Vec<String> {
        self.columns
            .iter()
            .filter(|column| {
                column
                    .valid_until_unix_s
                    .is_some_and(|until| expired(until, now_unix_ms))
            })
            .map(|column| column.name.clone())
            .collect()
    }
}

fn expired(valid_until_unix_s: i64, now_unix_ms: u64) -> bool {
    i128::from(now_unix_ms / 1000) > i128::from(valid_until_unix_s)
}

/// What is wrong with a refused table, and where (a JSON pointer, empty for the whole file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackTableIssue {
    pub problem: StandardPackProblem,
    pub pointer: String,
}

impl StandardPackTableIssue {
    fn new(problem: StandardPackProblem, pointer: impl Into<String>) -> Self {
        Self {
            problem,
            pointer: pointer.into(),
        }
    }
}

fn table_issue(problem: StandardPackProblem, pointer: impl Into<String>) -> StandardPackTableIssue {
    StandardPackTableIssue::new(problem, pointer)
}

impl std::fmt::Display for StandardPackTableIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "standard_pack_problem={} json_pointer={}",
            self.problem, self.pointer
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTable {
    schema_version: StandardPackTableVersion,
    game: String,
    table_id: String,
    row_key: Vec<String>,
    columns: Vec<RawColumn>,
    rows: Vec<Vec<Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawColumn {
    name: String,
    #[serde(rename = "type")]
    column_type: String,
    #[serde(default)]
    min: Option<i64>,
    #[serde(default)]
    max: Option<i64>,
    #[serde(default)]
    allowed: Option<Vec<String>>,
    #[serde(default)]
    nullable: Option<bool>,
    #[serde(default)]
    valid_until_unix_s: Option<i64>,
}

/// The table id a table file's name gives: the name without `.json`, when that is a table id
/// (`[a-z][a-z0-9_]{0,63}`).
pub fn standard_pack_table_id_of_file(file_name: &str) -> Option<&str> {
    file_name
        .strip_suffix(STANDARD_PACK_TABLE_SUFFIX)
        .filter(|table_id| standard_pack_table_id(table_id))
}

/// A table id or a column name: 1-64 bytes, `^[a-z][a-z0-9_]*$` (§2.1, §2.2).
pub fn standard_pack_table_id(id: &str) -> bool {
    standard_pack_page_id(id)
}

/// An allowed value of an `enum_string` column: 1-128 bytes of `[a-z0-9_.:-]` (§2.2).
fn enum_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_STANDARD_PACK_TABLE_ENUM_VALUE_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.:-".contains(&byte)
        })
}

/// A table file's bytes, decoded and checked on their own (§2.2): `file_name` is the file's name
/// in `tables/`, `game` the game directory it sits in (any valid game identifier when `None`,
/// for a resource repository's root). Canonical JSON of at most 2 MiB; `table_id` equal to the
/// file name without `.json`; 2..=32 typed columns; a row key of 1..=8 columns led by the one
/// `key` column; 1..=8192 rows, each as long as `columns`, every cell of its column's type and
/// range, unique on the row key.
pub fn parse_standard_pack_table(
    bytes: &[u8],
    game: Option<&str>,
    file_name: &str,
) -> Result<StandardPackTable, StandardPackTableIssue> {
    use StandardPackProblem as Problem;
    if bytes.len() > MAX_STANDARD_PACK_TABLE_BYTES {
        return Err(table_issue(Problem::TableTooLarge, ""));
    }
    // Floating point numbers, unsafe integers and duplicate keys are not canonical JSON.
    parse_canonical_json(bytes).map_err(|_| table_issue(Problem::Malformed, ""))?;
    let raw: RawTable =
        serde_json::from_slice(bytes).map_err(|_| table_issue(Problem::Malformed, ""))?;
    let game_valid = match game {
        Some(directory) => raw.game == directory,
        None => crate::package::bundle_identifier(&raw.game),
    };
    if !game_valid {
        return Err(table_issue(Problem::GameMismatch, "/game"));
    }
    if standard_pack_table_id_of_file(file_name) != Some(raw.table_id.as_str()) {
        return Err(table_issue(Problem::TableIdMismatch, "/table_id"));
    }
    let columns = check_columns(&raw.columns)?;
    let row_key = check_row_key(&raw.row_key, &columns)?;
    if raw.rows.is_empty() {
        return Err(table_issue(Problem::TableRowInvalid, "/rows"));
    }
    if raw.rows.len() > MAX_STANDARD_PACK_TABLE_ROWS {
        return Err(table_issue(Problem::TableTooLarge, "/rows"));
    }
    let mut rows = Vec::with_capacity(raw.rows.len());
    let mut keys = BTreeSet::new();
    for (index, row) in raw.rows.iter().enumerate() {
        if row.len() != columns.len() {
            return Err(table_issue(
                Problem::TableRowInvalid,
                format!("/rows/{index}"),
            ));
        }
        let cells = row
            .iter()
            .zip(&columns)
            .enumerate()
            .map(|(position, (cell, column))| {
                read_cell(cell, column)
                    .map_err(|problem| table_issue(problem, format!("/rows/{index}/{position}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let projection = row_key
            .iter()
            .map(|position| cells[*position].clone())
            .collect::<Vec<_>>();
        if !keys.insert(projection) {
            return Err(table_issue(
                Problem::TableRowDuplicate,
                format!("/rows/{index}"),
            ));
        }
        rows.push(cells);
    }
    Ok(StandardPackTable {
        game: raw.game,
        table_id: raw.table_id,
        row_key: raw.row_key,
        columns,
        rows,
    })
}

/// The declared columns (§2.2): 2..=32, each name a column name used once, each type declared
/// with exactly its members, at most one `key` column, and `valid_until_unix_s` only on a
/// nullable column.
fn check_columns(
    raw: &[RawColumn],
) -> Result<Vec<StandardPackTableColumn>, StandardPackTableIssue> {
    let invalid = |pointer: String| {
        StandardPackTableIssue::new(StandardPackProblem::TableColumnInvalid, pointer)
    };
    if !(MIN_STANDARD_PACK_TABLE_COLUMNS..=MAX_STANDARD_PACK_TABLE_COLUMNS).contains(&raw.len()) {
        return Err(invalid("/columns".to_owned()));
    }
    let mut names = BTreeSet::new();
    let mut key_seen = false;
    let mut columns = Vec::with_capacity(raw.len());
    for (index, column) in raw.iter().enumerate() {
        let at = |member: &str| format!("/columns/{index}/{member}");
        if !standard_pack_table_id(&column.name) || !names.insert(column.name.as_str()) {
            return Err(invalid(at("name")));
        }
        let column_type = match column.column_type.as_str() {
            "key" => {
                if key_seen {
                    return Err(invalid(at("type")));
                }
                key_seen = true;
                StandardPackTableColumnType::Key
            }
            "integer" => match (column.min, column.max) {
                (Some(min), Some(max)) if min <= max => {
                    StandardPackTableColumnType::Integer { min, max }
                }
                (None, _) => return Err(invalid(at("min"))),
                _ => return Err(invalid(at("max"))),
            },
            "boolean" => StandardPackTableColumnType::Boolean,
            "enum_string" => {
                let Some(allowed) = &column.allowed else {
                    return Err(invalid(at("allowed")));
                };
                if allowed.is_empty() || allowed.len() > MAX_STANDARD_PACK_TABLE_ENUM_VALUES {
                    return Err(invalid(at("allowed")));
                }
                let mut seen = BTreeSet::new();
                for (position, value) in allowed.iter().enumerate() {
                    if !enum_value(value) || !seen.insert(value.as_str()) {
                        return Err(invalid(format!("/columns/{index}/allowed/{position}")));
                    }
                }
                StandardPackTableColumnType::EnumString {
                    allowed: allowed.clone(),
                }
            }
            _ => return Err(invalid(at("type"))),
        };
        let integer = matches!(column_type, StandardPackTableColumnType::Integer { .. });
        let enumerated = matches!(column_type, StandardPackTableColumnType::EnumString { .. });
        if !integer && column.min.is_some() {
            return Err(invalid(at("min")));
        }
        if !integer && column.max.is_some() {
            return Err(invalid(at("max")));
        }
        if !enumerated && column.allowed.is_some() {
            return Err(invalid(at("allowed")));
        }
        let nullable = column.nullable.unwrap_or(false);
        if nullable && column_type == StandardPackTableColumnType::Key {
            return Err(invalid(at("nullable")));
        }
        if column.valid_until_unix_s.is_some() && !nullable {
            return Err(invalid(at("valid_until_unix_s")));
        }
        columns.push(StandardPackTableColumn {
            name: column.name.clone(),
            column_type,
            nullable,
            valid_until_unix_s: column.valid_until_unix_s,
        });
    }
    Ok(columns)
}

/// The row key (§2.2): 1..=8 declared columns, used once each, led by the `key` column; the
/// others neither nullable nor `key`. Answers each row key column's position in `columns`.
fn check_row_key(
    row_key: &[String],
    columns: &[StandardPackTableColumn],
) -> Result<Vec<usize>, StandardPackTableIssue> {
    let invalid = |pointer: String| {
        StandardPackTableIssue::new(StandardPackProblem::TableRowKeyInvalid, pointer)
    };
    if row_key.is_empty() || row_key.len() > MAX_STANDARD_PACK_TABLE_ROW_KEY {
        return Err(invalid("/row_key".to_owned()));
    }
    let mut positions = Vec::with_capacity(row_key.len());
    for (index, name) in row_key.iter().enumerate() {
        let pointer = format!("/row_key/{index}");
        let Some(position) = columns.iter().position(|column| &column.name == name) else {
            return Err(invalid(pointer));
        };
        if positions.contains(&position) {
            return Err(invalid(pointer));
        }
        let column = &columns[position];
        let is_key = column.column_type == StandardPackTableColumnType::Key;
        if (index == 0) != is_key || column.nullable {
            return Err(invalid(pointer));
        }
        positions.push(position);
    }
    Ok(positions)
}

/// One cell of `column`: `table_row_invalid` when it is not of the column's type,
/// `table_value_out_of_range` when it is outside the column's range or allowed values, or
/// `null` in a column that is not nullable.
fn read_cell(
    cell: &Value,
    column: &StandardPackTableColumn,
) -> Result<StandardPackTableCell, StandardPackProblem> {
    use StandardPackProblem as Problem;
    if cell.is_null() {
        return if column.nullable {
            Ok(StandardPackTableCell::Null)
        } else {
            Err(Problem::TableValueOutOfRange)
        };
    }
    match &column.column_type {
        StandardPackTableColumnType::Key => {
            let text = cell.as_str().ok_or(Problem::TableRowInvalid)?;
            if crate::candidate_projection::is_business_identity(text) {
                Ok(StandardPackTableCell::Text(text.to_owned()))
            } else {
                Err(Problem::TableValueOutOfRange)
            }
        }
        StandardPackTableColumnType::Integer { min, max } => {
            let value = cell.as_i64().ok_or(Problem::TableRowInvalid)?;
            if (*min..=*max).contains(&value) {
                Ok(StandardPackTableCell::Integer(value))
            } else {
                Err(Problem::TableValueOutOfRange)
            }
        }
        StandardPackTableColumnType::Boolean => cell
            .as_bool()
            .map(StandardPackTableCell::Boolean)
            .ok_or(Problem::TableRowInvalid),
        StandardPackTableColumnType::EnumString { allowed } => {
            let text = cell.as_str().ok_or(Problem::TableRowInvalid)?;
            if allowed.iter().any(|value| value == text) {
                Ok(StandardPackTableCell::Text(text.to_owned()))
            } else {
                Err(Problem::TableValueOutOfRange)
            }
        }
    }
}

/// One file of a game root's `tables/` directory as read: its table id (the file name without
/// `.json`), where it is, and its row count and column expiries, or why it is refused
/// (`standard_pack_table_invalid` or `standard_pack_table_unreadable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackTableFile {
    pub table_id: String,
    pub path: PathBuf,
    pub read: Result<StandardPackTableSummary, Outcome>,
}

/// What a read table reports: its row count and the expiry of each column that declares one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackTableSummary {
    pub row_count: usize,
    /// (column name, `valid_until_unix_s`), in column order.
    pub valid_until: Vec<(String, i64)>,
}

impl StandardPackTableSummary {
    fn of(table: &StandardPackTable) -> Self {
        Self {
            row_count: table.row_count(),
            valid_until: table
                .columns
                .iter()
                .filter_map(|column| {
                    column
                        .valid_until_unix_s
                        .map(|until| (column.name.clone(), until))
                })
                .collect(),
        }
    }

    /// The columns expired at `now_unix_ms` ([`StandardPackTable::expired_columns`]).
    pub fn expired_columns(&self, now_unix_ms: u64) -> Vec<String> {
        self.valid_until
            .iter()
            .filter(|(_, until)| expired(*until, now_unix_ms))
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// Reads every table of the game root `game_root` (the directory of `game`) as the Runtime's
/// startup, check-config and `actingctl packs` read them (§2.3, §2.4): every entry of `tables/`
/// in file name order, the first 64 parsed with [`parse_standard_pack_table`], each later one
/// `table_too_large`. Nothing is kept beyond the row count and the column expiries. No
/// `tables/` directory is no table; a `tables/` that cannot be listed is
/// `standard_pack_table_unreadable` for the directory.
pub fn read_standard_pack_tables(
    game_root: &Path,
    game: &str,
) -> Result<Vec<StandardPackTableFile>, Outcome> {
    let directory = game_root.join(STANDARD_PACK_TABLES_DIRECTORY);
    let unreadable = |path: &Path, error: &io::Error, op: IoOp| {
        Outcome::new(SharedCode::StandardPackTableUnreadable)
            .game(game)
            .path(path)
            .caused_by(Outcome::from_io_error(error, op))
    };
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unreadable(&directory, &error, IoOp::ReadDir)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| unreadable(&directory, &error, IoOp::ReadDir))?;
        names.push(entry.file_name());
    }
    names.sort();
    Ok(names
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let path = directory.join(&name);
            let name = name.to_string_lossy().into_owned();
            let table_id = name
                .strip_suffix(STANDARD_PACK_TABLE_SUFFIX)
                .unwrap_or(&name)
                .to_owned();
            let invalid = |issue: StandardPackTableIssue| {
                Outcome::new(SharedCode::StandardPackTableInvalid)
                    .game(game)
                    .path(&path)
                    .standard_pack_problem(issue.problem)
                    .json_pointer(issue.pointer)
            };
            let read = if index >= MAX_STANDARD_PACK_TABLES {
                Err(invalid(StandardPackTableIssue::new(
                    StandardPackProblem::TableTooLarge,
                    "",
                )))
            } else {
                read_table_file(&path)
                    .map_err(|(error, op)| unreadable(&path, &error, op))
                    .and_then(|bytes| {
                        parse_standard_pack_table(&bytes, Some(game), &name)
                            .map(|table| StandardPackTableSummary::of(&table))
                            .map_err(invalid)
                    })
            };
            StandardPackTableFile {
                table_id,
                path,
                read,
            }
        })
        .collect())
}

/// A table file's bytes, at most one byte past the size limit (the parser refuses a larger
/// file without the rest being read).
fn read_table_file(path: &Path) -> Result<Vec<u8>, (io::Error, IoOp)> {
    let file = fs::File::open(path).map_err(|error| (error, IoOp::Open))?;
    let mut bytes = Vec::new();
    file.take(MAX_STANDARD_PACK_TABLE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| (error, IoOp::Read))?;
    Ok(bytes)
}

/// One table in a report of the game root (register §1.65 TB3, TB4: check-config's
/// `standard_packs.games[].tables` and `actingctl packs` / `ac_packs`):
/// `{table_id, path, row_count?, problem?, expired_columns?}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StandardPackTableReport {
    pub table_id: String,
    pub path: PathBuf,
    /// The table's rows; absent when it is refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_count: Option<usize>,
    /// `standard_pack_table_invalid` or `standard_pack_table_unreadable` when it is refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Outcome>,
    /// The columns whose `valid_until_unix_s` has passed; absent when none has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expired_columns: Option<Vec<String>>,
}

impl StandardPackTableReport {
    /// The report of `file` at `now_unix_ms`.
    pub fn of(file: &StandardPackTableFile, now_unix_ms: u64) -> Self {
        let (row_count, problem, expired_columns) = match &file.read {
            Ok(summary) => {
                let expired = summary.expired_columns(now_unix_ms);
                (
                    Some(summary.row_count),
                    None,
                    (!expired.is_empty()).then_some(expired),
                )
            }
            Err(problem) => (None, Some(problem.clone()), None),
        };
        Self {
            table_id: file.table_id.clone(),
            path: file.path.clone(),
            row_count,
            problem,
            expired_columns,
        }
    }
}

// Workflow #357 DB-1 (M-357a v1.1 §8, the DB-1 test list).
#[cfg(test)]
#[path = "standard_pack_table_tests.rs"]
mod tests;
