// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1c fix round 3 (#411; work order rd-1027/brief-rl1c-fix3.md, re-review
// rd-1027/v01220-review/rl1c-fix2.md M1): the suspended report's extra reads are index reads with
// no sort, on an empty database built as a new root's is (`plan_check`):
// - the ledger's latest time: `MAX(timestamp_unix_ms)`, one entry of the time index, then the
//   newest row at that time through the same index; both pass the plan rule;
// - the newest matching row of a type: the type index from a sequence down, newest first.

use super::plan_check::{PlanCheck, check_plan};
use super::{LATEST_TIME, latest_time_row_statement, newest_statement};

// The step SQLite adds when it sorts the rows for an ORDER BY.
const SORT: &str = "USE TEMP B-TREE";

// Whether the plan has `step`.
fn uses(check: &PlanCheck, step: &str) -> bool {
    check.plan.iter().any(|line| line.trim_start() == step)
}

// Whether the plan sorts.
fn sorts(check: &PlanCheck) -> bool {
    check.plan.iter().any(|line| line.contains(SORT))
}

#[test]
fn rl1c_fix3_the_latest_time_reads_one_entry_and_one_row_through_the_time_index() {
    let maximum = check_plan(LATEST_TIME);
    assert!(
        maximum.violations.is_empty()
            && uses(
                &maximum,
                "SEARCH ledger_events USING COVERING INDEX ledger_view_index_time_v1"
            )
            && !sorts(&maximum),
        "{maximum:?}"
    );
    let row = check_plan(&latest_time_row_statement());
    assert!(
        row.violations.is_empty()
            && uses(
                &row,
                "SEARCH ledger_events USING INDEX ledger_view_index_time_v1 (timestamp_unix_ms=?)"
            )
            && !sorts(&row),
        "{row:?}"
    );
}

#[test]
fn rl1c_fix3_the_newest_first_read_walks_the_type_index_without_a_sort() {
    let newest = check_plan(&newest_statement());
    assert!(
        uses(
            &newest,
            "SEARCH ledger_events USING INDEX ledger_view_index_type_v1 (event_type=? AND sequence<?)"
        ) && !sorts(&newest),
        "{newest:?}"
    );
    // No scan, automatic index or history-wide index. The plan rule's type index term asks for a
    // lower sequence bound; this walk is bounded from above, each statement by its row limit.
    assert!(
        newest.violations.iter().all(|violation| {
            violation.starts_with("the event type index without a sequence bound")
        }),
        "{newest:?}"
    );
}
