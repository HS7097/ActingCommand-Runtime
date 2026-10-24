// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-1 (M-357a v1.1 §8, the DB-1 test list; §2.2, §6.2): a synthetic table
//! with nullable cells is read; each `standard_pack_problem` a table can have is refused at
//! its pointer; the row and byte limits pass at the limit and refuse one more.

use super::*;
use serde_json::{Value, json};

const GAME: &str = "neutral";
const FILE: &str = "unit_scores.json";

/// The model's neutral example table (§2.2), with nullable cells.
fn table() -> Value {
    json!({
        "schema_version": "actingcommand.table.v1",
        "game": GAME,
        "table_id": "unit_scores",
        "row_key": ["unit", "group", "variant"],
        "columns": [
            {"name": "unit", "type": "key"},
            {"name": "group", "type": "enum_string", "allowed": ["alpha", "beta"]},
            {"name": "variant", "type": "enum_string", "allowed": ["v1", "v2", "none"]},
            {"name": "listed", "type": "boolean"},
            {"name": "primary_milli", "type": "integer", "min": 0, "max": 100000, "nullable": true},
            {"name": "secondary_milli", "type": "integer", "min": 0, "max": 100000, "nullable": true,
             "valid_until_unix_s": 1_800_000_000_i64},
            {"name": "primary_status", "type": "enum_string", "allowed": ["confirmed", "partial"],
             "nullable": true}
        ],
        "rows": [
            ["unit.a", "alpha", "v1", true, 87500, 21300, "confirmed"],
            ["unit.a", "alpha", "v2", true, 92000, null, "partial"],
            ["unit.b", "beta", "none", true, null, null, null]
        ]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("fixture JSON")
}

fn read(value: &Value) -> Result<StandardPackTable, StandardPackTableIssue> {
    parse_standard_pack_table(&bytes(value), Some(GAME), FILE)
}

fn issue(problem: StandardPackProblem, pointer: &str) -> StandardPackTableIssue {
    StandardPackTableIssue {
        problem,
        pointer: pointer.to_owned(),
    }
}

/// A two-column table (`unit` key, `score_milli` integer) of `rows` rows.
fn rows_table(rows: usize) -> Value {
    json!({
        "schema_version": "actingcommand.table.v1",
        "game": GAME,
        "table_id": "unit_scores",
        "row_key": ["unit"],
        "columns": [
            {"name": "unit", "type": "key"},
            {"name": "score_milli", "type": "integer", "min": 0, "max": 100000}
        ],
        "rows": (0..rows).map(|index| json!([format!("unit.{index:05}"), 1000])).collect::<Vec<_>>()
    })
}

#[test]
fn db1_a_synthetic_table_with_nullable_cells_is_read() {
    let read = read(&table()).expect("a valid table");
    assert_eq!(read.game, GAME);
    assert_eq!(read.table_id, "unit_scores");
    assert_eq!(read.row_key, ["unit", "group", "variant"]);
    assert_eq!(read.columns.len(), 7);
    assert_eq!(
        read.columns[4].column_type,
        StandardPackTableColumnType::Integer {
            min: 0,
            max: 100_000
        }
    );
    assert!(read.columns[4].nullable);
    assert_eq!(read.columns[5].valid_until_unix_s, Some(1_800_000_000));
    assert_eq!(read.row_count(), 3);
    assert_eq!(
        read.rows[1],
        [
            StandardPackTableCell::Text("unit.a".to_owned()),
            StandardPackTableCell::Text("alpha".to_owned()),
            StandardPackTableCell::Text("v2".to_owned()),
            StandardPackTableCell::Boolean(true),
            StandardPackTableCell::Integer(92_000),
            StandardPackTableCell::Null,
            StandardPackTableCell::Text("partial".to_owned()),
        ]
    );
    assert_eq!(read.rows[2][4], StandardPackTableCell::Null);
    // The expiring column reads as expired only after its second.
    assert!(read.expired_columns(1_800_000_000_999).is_empty());
    assert_eq!(read.expired_columns(1_800_000_001_000), ["secondary_milli"]);
    // A resource repository's table names its game itself.
    parse_standard_pack_table(&bytes(&table()), None, FILE)
        .expect("a table read without its directory");
}

#[test]
fn db1_each_table_problem_is_refused_at_its_pointer() {
    use StandardPackProblem as Problem;
    // The example table with one change.
    let with = |change: &dyn Fn(&mut Value)| {
        let mut table = table();
        change(&mut table);
        bytes(&table)
    };
    let cases: Vec<(&str, Vec<u8>, StandardPackTableIssue)> = vec![
        (
            "a floating point number",
            bytes(&table()).pipe_replace(b"87500", b"875.5"),
            issue(Problem::Malformed, ""),
        ),
        (
            "an unsafe integer",
            bytes(&table()).pipe_replace(b"87500", b"9007199254740993"),
            issue(Problem::Malformed, ""),
        ),
        (
            "a duplicate key",
            bytes(&table()).pipe_replace(
                b"\"game\":\"neutral\"",
                b"\"game\":\"neutral\",\"game\":\"neutral\"",
            ),
            issue(Problem::Malformed, ""),
        ),
        (
            "an unknown member",
            with(&|table: &mut Value| table["notes"] = json!("prose")),
            issue(Problem::Malformed, ""),
        ),
        (
            "the game of another directory",
            with(&|table: &mut Value| table["game"] = json!("other")),
            issue(Problem::GameMismatch, "/game"),
        ),
        (
            "a table id that is not the file name",
            with(&|table: &mut Value| table["table_id"] = json!("unit_ranks")),
            issue(Problem::TableIdMismatch, "/table_id"),
        ),
        (
            "valid_until_unix_s on a column that is not nullable",
            with(&|table: &mut Value| {
                table["columns"][3]["valid_until_unix_s"] = json!(1_800_000_000_i64);
            }),
            issue(Problem::TableColumnInvalid, "/columns/3/valid_until_unix_s"),
        ),
        (
            "the row key's first column is not the key column",
            with(&|table: &mut Value| table["row_key"] = json!(["group", "unit", "variant"])),
            issue(Problem::TableRowKeyInvalid, "/row_key/0"),
        ),
        (
            "a row shorter than the columns",
            with(&|table: &mut Value| {
                table["rows"][1] = json!(["unit.a", "alpha", "v2", true, 92000, null]);
            }),
            issue(Problem::TableRowInvalid, "/rows/1"),
        ),
        (
            "an integer column holding a string",
            with(&|table: &mut Value| table["rows"][0][4] = json!("87500")),
            issue(Problem::TableRowInvalid, "/rows/0/4"),
        ),
        (
            "null in a column that is not nullable",
            with(&|table: &mut Value| table["rows"][2][3] = Value::Null),
            issue(Problem::TableValueOutOfRange, "/rows/2/3"),
        ),
        (
            "an enum value outside allowed",
            with(&|table: &mut Value| table["rows"][0][1] = json!("gamma")),
            issue(Problem::TableValueOutOfRange, "/rows/0/1"),
        ),
        (
            "a duplicate row key",
            with(&|table: &mut Value| table["rows"][1][2] = json!("v1")),
            issue(Problem::TableRowDuplicate, "/rows/1"),
        ),
    ];
    for (case, input, expected) in cases {
        assert_eq!(
            parse_standard_pack_table(&input, Some(GAME), FILE).map(|_| ()),
            Err(expected),
            "{case}"
        );
    }
    assert_eq!(
        parse_standard_pack_table(&bytes(&table()), Some(GAME), "unit-scores.json").map(|_| ()),
        Err(issue(Problem::TableIdMismatch, "/table_id")),
        "a file name that is no table id"
    );
}

#[test]
fn db1_rows_and_bytes_pass_at_the_limit_and_one_more_is_refused() {
    let at_limit = parse_standard_pack_table(
        &bytes(&rows_table(MAX_STANDARD_PACK_TABLE_ROWS)),
        Some(GAME),
        FILE,
    )
    .expect("8192 rows");
    assert_eq!(at_limit.row_count(), MAX_STANDARD_PACK_TABLE_ROWS);
    assert_eq!(
        parse_standard_pack_table(
            &bytes(&rows_table(MAX_STANDARD_PACK_TABLE_ROWS + 1)),
            Some(GAME),
            FILE
        )
        .map(|_| ()),
        Err(issue(StandardPackProblem::TableTooLarge, "/rows"))
    );
    // JSON whitespace after the document fills the file to the byte limit.
    let mut padded = bytes(&table());
    padded.resize(MAX_STANDARD_PACK_TABLE_BYTES, b' ');
    assert_eq!(padded.len(), 2 * 1024 * 1024);
    parse_standard_pack_table(&padded, Some(GAME), FILE).expect("a table of 2 MiB");
    padded.push(b' ');
    assert_eq!(
        parse_standard_pack_table(&padded, Some(GAME), FILE).map(|_| ()),
        Err(issue(StandardPackProblem::TableTooLarge, ""))
    );
}

/// `bytes` with the first `from` replaced by `to` (a fixture edit JSON values cannot spell).
trait PipeReplace {
    fn pipe_replace(self, from: &[u8], to: &[u8]) -> Vec<u8>;
}

impl PipeReplace for Vec<u8> {
    fn pipe_replace(self, from: &[u8], to: &[u8]) -> Vec<u8> {
        let at = self
            .windows(from.len())
            .position(|window| window == from)
            .expect("fixture bytes hold the text to replace");
        let mut replaced = self[..at].to_vec();
        replaced.extend_from_slice(to);
        replaced.extend_from_slice(&self[at + from.len()..]);
        replaced
    }
}
