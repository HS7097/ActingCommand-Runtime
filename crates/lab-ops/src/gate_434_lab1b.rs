// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-1b (register §1.287, "Slice LAB-1b"): the slice's named test.
//!
//! - `lab_ops_core_rows_served_by_lab_ops`: every `core` row of the operation table file whose
//!   `slice` is `LAB-1` or `LAB-1b` names a function of this crate that serves it, in the
//!   registry the crate exposes ([`crate::served`]); and every entry of the registry names such a
//!   row, once. That an entry names something callable (a function, or a `const` or `static` of a
//!   function-pointer type, or a tuple-struct constructor), not a value constant or another item,
//!   the registry checks as it compiles.

use crate::served::{SERVED, server};
use crate::table::{Operation, OperationModule, TABLE_PATH, read_operation_table};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The table file of this repository.
fn table_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(TABLE_PATH)
}

/// Whether `operation` is a `core` row LAB-1 or LAB-1b builds.
fn served_here(operation: &Operation) -> bool {
    operation.module == OperationModule::Core
        && matches!(operation.slice.as_str(), "LAB-1" | "LAB-1b")
}

// The table file read itself and the registry: no core row of LAB-1 or LAB-1b is without the
// function that serves it, and the registry names only such rows, each once.
#[test]
fn lab_ops_core_rows_served_by_lab_ops() {
    let table = read_operation_table(&table_file())
        .unwrap_or_else(|outcome| panic!("the operation table reads: {outcome}"));
    let mut violations = Vec::new();

    let mut rows = 0;
    for operation in &table.operations {
        if !served_here(operation) {
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
        if !registered.insert(served.name) {
            violations.push(format!("the registry names {} more than once", served.name));
        }
        match table
            .operations
            .iter()
            .find(|operation| operation.name == served.name)
        {
            None => violations.push(format!(
                "the registry names {} ({}), which is no operation of the table",
                served.name, served.function
            )),
            Some(operation) if !served_here(operation) => violations.push(format!(
                "the registry names {} ({}), whose row is {:?} of slice {}, not core of LAB-1 or \
                 LAB-1b",
                served.name, served.function, operation.module, operation.slice
            )),
            Some(_) => {}
        }
    }

    assert!(rows > 0, "the table has no core row of LAB-1 or LAB-1b");
    assert!(
        violations.is_empty(),
        "core rows of LAB-1 and LAB-1b and the lab-ops registry disagree:\n{}",
        violations.join("\n")
    );
}
