// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5B1b (register §1.287, "Slice LAB-5B1b"; coordinator work order
// rd-1113/brief-lab5b1b.md, points 1 and 2): the parameters the code of each row the rescue
// module serves reads are one constant per row (`crate::rescue_reads`), which that code reads them
// through and builds a command line from, held equal to the row.
// - `rescue_read_here_rows_read_what_they_declare`: each row of `READ_HERE` is a planned rescue row
//   the module serves, and its constant holds exactly the row's declared parameters: every name its
//   code reads is declared, every declared parameter is read.
// - `rescue_handed_rows_hand_what_they_declare`: the same for the rows whose answer is an installer
//   command (`INSTALLER`) or the agent channel's words (`AGENT`); a parameter written as LAB-5A's
//   further flags were is written under the flag of its name; each part is one its builder writes
//   in its place, those written in the order given (`Written::Each`, `Part::Further`) last.
// - `rescue_exceptions_are_outside_the_table`: no row an exception names declares its name; no row
//   the module serves declares a parameter it refuses by name (`rescue::REFUSED_PARAMETERS`).
// - `rescue_a_read_outside_its_constant_fails`: a read of a name a row's constant does not hold
//   fails the debug assertion test builds have; a declared one and an exception do not.
// A failure names the row, the parameter and the direction. The embedded table and the constants
// only: no Runtime, no file, no device. Left for one more slice (the work order's "Size"): the
// flags each command line writes held to what its receiver takes, and each receiver fed every
// declared parameter; done by Workflow #434 LAB-5B1c (`gate_434_lab5b1c`).

use crate::rescue;
use crate::rescue_reads::{self, AGENT, EXCEPTIONS, INSTALLER, READ_HERE};
use actingcommand_lab_ops::core_reads::{Part, Written};
use actingcommand_lab_ops::table::{Operation, OperationModule, OperationStatus, OperationTable};
use serde_json::Map;
use std::collections::BTreeSet;

fn table() -> OperationTable {
    actingcommand_lab_ops::table::operation_table().expect("the operation table")
}

/// A parameter's flag as the command line writes it.
fn flag(name: &str) -> String {
    format!("--{}", name.replace('_', "-"))
}

/// The planned rescue row `name` the module serves; a failure otherwise.
fn served_row<'t>(
    table: &'t OperationTable,
    name: &str,
    failures: &mut Vec<String>,
) -> Option<&'t Operation> {
    match table.operation(name) {
        Some(operation)
            if operation.module == OperationModule::Rescue
                && operation.status == OperationStatus::Planned
                && rescue::serves(name) =>
        {
            Some(operation)
        }
        Some(_) => {
            failures.push(format!("{name}: no planned rescue row the module serves"));
            None
        }
        None => {
            failures.push(format!("{name}: no row of the table"));
            None
        }
    }
}

/// The names a row's constant holds against the row's declared parameters, both directions.
fn compare(operation: &Operation, names: &[&str], failures: &mut Vec<String>) {
    let row = operation.name.as_str();
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            failures.push(format!("{row}: {name}: its constant holds it twice"));
        }
        if !operation.params.iter().any(|param| param.name == *name) {
            failures.push(format!(
                "{row}: {name}: code to row: its code reads it, the row does not declare it"
            ));
        }
    }
    for param in &operation.params {
        if !names.contains(&param.name.as_str()) {
            failures.push(format!(
                "{row}: {}: row to code: the row declares it, its code does not read it",
                param.name
            ));
        }
    }
}

#[test]
fn rescue_read_here_rows_read_what_they_declare() {
    let table = table();
    let mut failures = Vec::new();
    for reads in &READ_HERE {
        if let Some(operation) = served_row(&table, reads.row, &mut failures) {
            compare(operation, reads.names, &mut failures);
        }
    }
    assert!(
        failures.is_empty(),
        "rows read in the rescue module that differ from their constant:\n{}",
        failures.join("\n")
    );
}

/// Whether `part` is one the builder of an installer row (`installer`) or of an agent row writes
/// in its place.
fn in_place(installer: bool, part: Part) -> bool {
    if installer {
        matches!(part, Part::Root | Part::Param(_, Written::Flag(_)))
    } else {
        matches!(part, Part::StateRoot | Part::Param(_, Written::Word))
    }
}

#[test]
fn rescue_handed_rows_hand_what_they_declare() {
    let table = table();
    let mut failures = Vec::new();
    let rows = INSTALLER
        .iter()
        .map(|handed| (true, handed))
        .chain(AGENT.iter().map(|handed| (false, handed)));
    for (installer, handed) in rows {
        let row = handed.row;
        let Some(operation) = served_row(&table, row, &mut failures) else {
            continue;
        };
        let names = handed.params().map(|(name, _)| name).collect::<Vec<_>>();
        compare(operation, &names, &mut failures);
        if handed.only_with.is_some() {
            failures.push(format!("{row}: a rescue row is served one way only"));
        }
        // LAB-5A's further flags went on under the flag of their name (`further_flags`).
        for (name, written) in handed.params() {
            if let Written::Each(written) = written
                && written != flag(name)
            {
                failures.push(format!(
                    "{row}: {name}: written as {written}, not as {}",
                    flag(name)
                ));
            }
        }
        // The parts written in their place first, then those written in the order given, with
        // `Part::Further` last: the builder writes the latter all at once, at `Part::Further`.
        let mut ordered = false;
        for (index, part) in handed.parts.iter().enumerate() {
            let fits = match part {
                Part::Param(_, Written::Each(_)) => {
                    ordered = true;
                    true
                }
                Part::Further => index + 1 == handed.parts.len(),
                other => !ordered && in_place(installer, *other),
            };
            if !fits {
                failures.push(format!(
                    "{row}: {part:?}: not a part its builder writes there"
                ));
            }
        }
        if ordered && handed.parts.last() != Some(&Part::Further) {
            failures.push(format!(
                "{row}: parameters written in the order given, without Part::Further"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "handed rescue rows that differ from their constant:\n{}",
        failures.join("\n")
    );
}

#[test]
fn rescue_exceptions_are_outside_the_table() {
    let table = table();
    let mut failures = Vec::new();
    for exception in &EXCEPTIONS {
        if exception.rows.is_empty() {
            failures.push(format!("{}: an exception of no row", exception.name));
        }
        for row in exception.rows {
            let Some(operation) = served_row(&table, row, &mut failures) else {
                continue;
            };
            if operation
                .params
                .iter()
                .any(|param| param.name == exception.name)
            {
                failures.push(format!(
                    "{row}: {}: an exception, and the row declares it",
                    exception.name
                ));
            }
        }
        if exception.why.trim().is_empty() {
            failures.push(format!(
                "{}: an exception without its reason",
                exception.name
            ));
        }
    }
    // The parameters the module refuses by name on every row: no row it serves declares one.
    for operation in &table.operations {
        if operation.module != OperationModule::Rescue || !rescue::serves(&operation.name) {
            continue;
        }
        for param in &operation.params {
            if rescue::REFUSED_PARAMETERS.contains(&param.name.as_str()) {
                failures.push(format!(
                    "{}: {}: the row declares a parameter the module refuses",
                    operation.name, param.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[should_panic(expected = "the code of the row alive reads neutral, which its constant")]
fn rescue_a_read_outside_its_constant_fails() {
    if !cfg!(debug_assertions) {
        panic!(
            "Workflow #434 LAB-5B1b: a read outside a row's constant fails a debug assertion, \
             which this build does not assert"
        );
    }
    let params = Map::new();
    let alive = rescue_reads::read_here("alive", &params).expect("alive is read here");
    // A declared parameter, a handed row's, and `config restore`'s `sha` (an exception until
    // Workflow #434 LAB-5B1c, which the row declares since).
    assert_eq!(alive.get("wait"), None);
    let status = rescue_reads::installer("op status").expect("op status hands a line");
    assert_eq!(
        rescue_reads::handing(status, &params).get("check_rollback"),
        None
    );
    let restore = rescue_reads::read_here("config restore", &params).expect("read here");
    assert_eq!(restore.get("sha"), None);
    // A name the row does not declare.
    assert_eq!(alive.get("neutral"), None);
}
