// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-A (0.12.90; register §1.533, ruling 3; contracts/resource-readings.md, "List
// readings", "A list of the wanted keys only"; contracts/scheduling/README.md, "A pool read from
// rows of a list"): a list reading that passes over writes the wanted keys only, one row per
// wanted key a card showed, and none for a wanted item held at zero, which has no card. A pool
// that reads three of the list's rows (`list_rows`) sums the two the list holds and counts the one
// it does not hold 0, by the rule that exists: #455 P2's `list_rows_reading`, the one computation
// the host gives the evaluator a pool's reading by. The keys, columns and rows are made up; times
// are fixed Unix milliseconds.

use super::*;
use crate::pool_list_rows::list_rows_reading;
use actingcommand_contract::FactScalar;
use actingcommand_policy::{GivenReadingState, PoolListRow};

const LIST_KEY: &str = "inventory.list.example_kind_a";
/// When the list is read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long the list read stays fresh.
const FRESH_MS: u64 = 60_000;

/// A row `{id, count}` of the list, as a reading of the key column `id` and the integer column
/// `count` writes it.
fn row(key: &str, count: i64) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("id".to_owned(), FactScalar::String(key.to_owned())),
        ("count".to_owned(), FactScalar::Integer(count)),
    ])
}

// The list holds `member-a` (5) and `member-c` (9) and no row of `member-b`; the pool reads the
// rows of all three, each at the factor 1, with the minimum confidence 500. Its reading is
// 5 + 0 + 9, observed and expiring as the list.
#[test]
fn list_a_a_pool_of_three_rows_sums_the_two_the_list_holds_and_counts_the_absent_one_0() {
    let mut list = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        LIST_KEY,
        ContractFactValue::RecordList(vec![row("member-a", 5), row("member-c", 9)]),
        "snapshot:list-a-pool",
        Vec::new(),
    );
    list.observed_at_unix_ms = READ_AT;
    list.expires_at_unix_ms = Some(READ_AT + FRESH_MS);
    let rows = ["member-a", "member-b", "member-c"].map(|key| PoolListRow {
        key: key.to_owned(),
        factor: 1,
    });
    assert_eq!(
        list_rows_reading("id", "count", &rows, 500, Some(&list)),
        GivenReadingState::Read {
            value: 5 + 9,
            observed_at_unix_ms: READ_AT,
            expires_at_unix_ms: Some(READ_AT + FRESH_MS),
        },
        "member-b, which the list does not hold, counts 0"
    );
}
