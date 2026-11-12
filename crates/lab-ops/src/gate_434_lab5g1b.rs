// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G1b (register §1.287, "Slice LAB-5G1b"; coordinator work order
// rd-1112/brief-lab5g1b.md, point 4, and the coordinator's ruling on it): the names a row does not
// declare, from the operation table this build embeds alone (`Operation::undeclared`).
// - `lab_ops_undeclared_members_follow_the_table`: of `events`' members a declared one is not
//   returned and an undeclared one is, in the order given; every parameter of every row is
//   declared and a name no row declares is undeclared on each; `resume`'s `expect`, which the
//   table requires on the MCP front end only (`required_in`), is declared like any other.

use crate::table::operation_table;

#[test]
fn lab_ops_undeclared_members_follow_the_table() {
    let table = operation_table().expect("the operation table");

    // A declared member and an undeclared one.
    let events = table.operation("events").expect("the row events");
    assert_eq!(
        events.undeclared(["instance", "neutral_member", "limit"]),
        ["neutral_member"]
    );

    // Every row's parameters, and a name no row declares.
    for operation in &table.operations {
        let names = operation
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>();
        assert!(
            operation.undeclared(names.iter().copied()).is_empty(),
            "{}",
            operation.name
        );
        assert_eq!(
            operation.undeclared(["neutral_member"]),
            ["neutral_member"],
            "{}",
            operation.name
        );
    }

    // A parameter the table requires on one front end only is declared all the same.
    let resume = table.operation("resume").expect("the row resume");
    let expect = resume
        .params
        .iter()
        .find(|param| param.name == "expect")
        .expect("resume's expect");
    assert_eq!(expect.required_in.as_deref(), Some(&["mcp".to_owned()][..]));
    assert!(resume.undeclared(["expect"]).is_empty());
}
