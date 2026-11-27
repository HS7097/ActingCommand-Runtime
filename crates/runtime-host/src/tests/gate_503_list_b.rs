// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-B (0.12.93; register §1.536, ruling 5; contracts/resource-readings.md, "List
// readings", "A list of the wanted keys only"; contracts/scheduling/README.md, "A pool read from
// rows of a list"): a list read over cards identified by pictures writes one row per key a card
// showed, and none for a key held at zero, which has no card. The rows are those the execution
// kernel's gate `gate_503_list_b` reads from its grid of eighty cards: `member-a` to `member-f`
// with the counts 5, 7, 11, 13, 17 and 19, and no row of `member-g`, `member-h` or `member-i`. A
// pool that reads the nine rows (`list_rows`) sums the six and counts the three 0, by the rule
// that exists: #455 P2's `list_rows_reading`, the one computation the host gives the evaluator a
// pool's reading by. Times are fixed Unix milliseconds.

use super::*;
use crate::pool_list_rows::list_rows_reading;
use actingcommand_contract::FactScalar;
use actingcommand_policy::{GivenReadingState, PoolListRow};

const LIST_KEY: &str = "inventory.list.example_kind_a";
/// When the list is read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long the list read stays fresh.
const FRESH_MS: u64 = 60_000;
/// The nine keys, and the six rows the list holds.
const KEYS: [&str; 9] = [
    "member-a", "member-b", "member-c", "member-d", "member-e", "member-f", "member-g", "member-h",
    "member-i",
];
const HELD: [(&str, i64); 6] = [
    ("member-a", 5),
    ("member-b", 7),
    ("member-c", 11),
    ("member-d", 13),
    ("member-e", 17),
    ("member-f", 19),
];

/// A row `{id, count}` of the list.
fn row(key: &str, count: i64) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("id".to_owned(), FactScalar::String(key.to_owned())),
        ("count".to_owned(), FactScalar::Integer(count)),
    ])
}

// The list holds the six rows; the pool reads the rows of all nine keys, each at the factor 1,
// with the minimum confidence 500. Its reading is 5 + 7 + 11 + 13 + 17 + 19 (72), observed and
// expiring as the list.
#[test]
fn list_b_a_pool_of_nine_rows_sums_the_six_the_list_holds_and_counts_the_three_absent_0() {
    let mut list = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        LIST_KEY,
        ContractFactValue::RecordList(HELD.iter().map(|(key, count)| row(key, *count)).collect()),
        "snapshot:list-b-pool",
        Vec::new(),
    );
    list.observed_at_unix_ms = READ_AT;
    list.expires_at_unix_ms = Some(READ_AT + FRESH_MS);
    let rows = KEYS.map(|key| PoolListRow {
        key: key.to_owned(),
        factor: 1,
    });
    assert_eq!(
        list_rows_reading("id", "count", &rows, 500, Some(&list)),
        GivenReadingState::Read {
            value: 5 + 7 + 11 + 13 + 17 + 19,
            observed_at_unix_ms: READ_AT,
            expires_at_unix_ms: Some(READ_AT + FRESH_MS),
        },
        "member-g, member-h and member-i, which the list does not hold, count 0"
    );
}
