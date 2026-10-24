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

use crate::outcome::Outcome;
use crate::{StandardPackProblem, standard_pack_page_id};
use serde::{Deserialize, Serialize};
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
    pub fn expired_columns(&self, _now_unix_ms: u64) -> Vec<String> {
        Vec::new()
    }
}

/// What is wrong with a refused table, and where (a JSON pointer, empty for the whole file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackTableIssue {
    pub problem: StandardPackProblem,
    pub pointer: String,
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

/// A table file's bytes, decoded and checked on their own (§2.2): `file_name` is the file's name
/// in `tables/`, `game` the game directory it sits in (any valid game identifier when `None`,
/// for a resource repository's root). Canonical JSON of at most 2 MiB; `table_id` equal to the
/// file name without `.json`; 2..=32 typed columns; a row key of 1..=8 columns led by the one
/// `key` column; 1..=8192 rows, each as long as `columns`, every cell of its column's type and
/// range, unique on the row key.
pub fn parse_standard_pack_table(
    _bytes: &[u8],
    _game: Option<&str>,
    _file_name: &str,
) -> Result<StandardPackTable, StandardPackTableIssue> {
    // DB-1 red stub: no check yet.
    Err(StandardPackTableIssue {
        problem: StandardPackProblem::Malformed,
        pointer: String::new(),
    })
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
    /// The columns expired at `now_unix_ms` ([`StandardPackTable::expired_columns`]).
    pub fn expired_columns(&self, _now_unix_ms: u64) -> Vec<String> {
        Vec::new()
    }
}

/// Reads every table of the game root `game_root` (the directory of `game`) as the Runtime's
/// startup, check-config and `actingctl packs` read them (§2.3, §2.4): every entry of `tables/`
/// in file name order, the first 64 parsed with [`parse_standard_pack_table`], each later one
/// `table_too_large`. Nothing is kept beyond the row count and the column expiries. No
/// `tables/` directory is no table; a `tables/` that cannot be listed is
/// `standard_pack_table_unreadable` for the directory.
pub fn read_standard_pack_tables(
    _game_root: &Path,
    _game: &str,
) -> Result<Vec<StandardPackTableFile>, Outcome> {
    // DB-1 red stub: no table is read yet.
    Ok(Vec::new())
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
