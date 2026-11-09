// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-1b (register §1.287, "Slice LAB-1b"): the slice's named test.
//!
//! - `lab_ops_core_rows_served_by_lab_ops`: every `core` row of the operation table file whose
//!   `slice` is `LAB-1` or `LAB-1b` names a function of this crate that serves it, in the
//!   registry the crate exposes ([`crate::served`]).

use crate::served::{SERVED, server};
use crate::table::{OperationModule, TABLE_PATH, read_operation_table};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The table file of this repository.
fn table_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(TABLE_PATH)
}

// The table file read itself and the registry: no core row of LAB-1 or LAB-1b is without the
// function that serves it, and the registry names each operation once.
#[test]
fn lab_ops_core_rows_served_by_lab_ops() {
    let table = read_operation_table(&table_file())
        .unwrap_or_else(|outcome| panic!("the operation table reads: {outcome}"));
    let mut violations = Vec::new();

    let mut rows = 0;
    for operation in &table.operations {
        if operation.module != OperationModule::Core
            || !matches!(operation.slice.as_str(), "LAB-1" | "LAB-1b")
        {
            continue;
        }
        rows += 1;
        if server(&operation.name).is_none() {
            violations.push(format!(
                "{} (slice {}) names no function of lab-ops that serves it",
                operation.name, operation.slice
            ));
        }
    }

    let mut registered = BTreeSet::new();
    for served in SERVED {
        if !registered.insert(served.operation) {
            violations.push(format!(
                "the registry names {} more than once",
                served.operation
            ));
        }
        if served.function.is_empty() {
            violations.push(format!(
                "the registry names no function for {}",
                served.operation
            ));
        }
    }

    assert!(rows > 0, "the table has no core row of LAB-1 or LAB-1b");
    assert!(
        violations.is_empty(),
        "core rows of LAB-1 and LAB-1b without their server in lab-ops:\n{}",
        violations.join("\n")
    );
}
