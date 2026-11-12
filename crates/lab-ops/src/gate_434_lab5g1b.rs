// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G1b (register §1.287, "Slice LAB-5G1b"; coordinator work order
// rd-1112/brief-lab5g1b.md, point 4): the names a row does not declare on a front end, from the
// operation table this build embeds alone (`OperationTable::undeclared`).
// - `lab_ops_undeclared_members_follow_the_table`: on the MCP front end a declared member of
//   `events` is not returned and an undeclared one is, in the order given; every parameter of
//   every row is declared on every front end the table names and a name no row declares is
//   undeclared on each; `resume`'s `expect`, required on the MCP front end only (`required_in`),
//   is declared on every front end; on a name that is no front end of the table nothing is
//   declared.

use crate::table::{ModuleRole, OperationTable, operation_table};

/// The ids of the table's front ends, in its order.
fn front_ends(table: &OperationTable) -> Vec<&str> {
    table
        .modules
        .iter()
        .filter(|module| module.role == ModuleRole::FrontEnd)
        .map(|module| module.id.as_str())
        .collect()
}

#[test]
fn lab_ops_undeclared_members_follow_the_table() {
    let table = operation_table().expect("the operation table");
    let fronts = front_ends(&table);
    assert_eq!(fronts, ["cli", "mcp", "serve", "watchdog"]);

    // A declared member and an undeclared one, on the MCP front end.
    let events = table.operation("events").expect("the row events");
    assert_eq!(
        table.undeclared(events, "mcp", ["instance", "neutral_member", "limit"]),
        ["neutral_member"]
    );

    // Every row's parameters on every front end, and a name no row declares.
    for operation in &table.operations {
        let names = operation
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>();
        for front in &fronts {
            assert!(
                table
                    .undeclared(operation, front, names.iter().copied())
                    .is_empty(),
                "{} on {front}",
                operation.name
            );
            assert_eq!(
                table.undeclared(operation, front, ["neutral_member"]),
                ["neutral_member"],
                "{} on {front}",
                operation.name
            );
        }
    }

    // A parameter the table requires on one front end only is declared on every front end.
    let resume = table.operation("resume").expect("the row resume");
    let expect = resume
        .params
        .iter()
        .find(|param| param.name == "expect")
        .expect("resume's expect");
    assert_eq!(expect.required_in.as_deref(), Some(&["mcp".to_owned()][..]));
    for front in &fronts {
        assert!(
            table.undeclared(resume, front, ["expect"]).is_empty(),
            "resume's expect on {front}"
        );
    }

    // A name that is no front end of the table declares nothing.
    assert_eq!(
        table.undeclared(events, "neutral_front", ["instance", "neutral_member"]),
        ["instance", "neutral_member"]
    );
}
