// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S3 (register §1.287, "Slice LAB-6S3"; contracts/lab-two-entries.md §3): the
// part of `gate_two_entries` that holds the UI list to the operation table. The contract crate's
// static table `ui_allows` takes exactly the kinds of the table's `ui_kinds` (the transitional
// `RecordClientAction` included), and the table names each of them once.

use crate::table::{operation_table, request_kind_name};
use actingcommand_contract::{RuntimeOperationKind, ui_allows};
use std::collections::BTreeSet;

#[test]
fn lab_ops_ui_allows_is_the_tables_ui_kinds() {
    let table =
        operation_table().unwrap_or_else(|outcome| panic!("the operation table reads: {outcome}"));
    let listed = table
        .ui_kinds
        .iter()
        .map(|row| row.kind)
        .collect::<BTreeSet<RuntimeOperationKind>>();
    assert_eq!(
        listed.len(),
        table.ui_kinds.len(),
        "the table lists a kind twice"
    );
    for kind in RuntimeOperationKind::ALL {
        assert_eq!(
            ui_allows(*kind),
            listed.contains(kind),
            "{}",
            request_kind_name(*kind)
        );
    }
}
