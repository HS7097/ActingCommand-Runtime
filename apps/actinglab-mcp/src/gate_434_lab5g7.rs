// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G7 (register §1.287, "Slice LAB-5G7"; `apps/actinglab-mcp/MANUAL.md`): the
//! manual and the instructions name no tool the operation table does not give. The manual pastes
//! no list of tools; where it, or the instructions, name one, the name is held here to the table
//! this build embeds. No Runtime, no clock; the manual is read as the build embeds it.
//!
//! - `mcp_manual_names_only_offered_tools`: every `ac_…` name the manual and the instructions
//!   write is the tool of a planned row this server offers (off Windows, where the rescue module
//!   does not run, a rescue row's too), but `ac_observe`, which both name as no tool and which is
//!   no row's tool; both name each tool of `tools::NAMED`; the instructions name the manual by its
//!   path in the repository.
//! - `mcp_manual_exception_is_the_refused_rows`: the tools the manual's section on the one
//!   exception names are exactly those of the rows the front forces detached (`tools::SUPPLIED`,
//!   `tools::runs_detached`), and on Windows exactly the offered rows `tools::serving` refuses;
//!   elsewhere this server offers no row it refuses.

use crate::tools::{self, Catalog, NAMED, Serving};
use actingcommand_lab_ops::table::{self, OperationModule, OperationStatus, OperationTable};
use std::collections::BTreeSet;

/// The manual, as the repository holds it beside this program.
const MANUAL: &str = include_str!("../MANUAL.md");
/// The manual's path in the repository, which the instructions name.
const MANUAL_PATH: &str = "apps/actinglab-mcp/MANUAL.md";
/// The heading of the manual's section on the one exception.
const EXCEPTION_HEADING: &str = "## 9 The one exception";
/// The name no tool takes (board #434, the coordinator's ruling 6784514298, point 1).
const RETIRED: &str = "ac_observe";
/// What every tool's name begins with.
const TOOL_PREFIX: &str = "ac_";

fn embedded() -> OperationTable {
    table::operation_table().expect("the operation table this build embeds reads")
}

/// Every tool name `text` writes: `ac_` and the lowercase letters, digits and `_` after it, where
/// no letter, digit or `_` stands before it; the rule's own `ac_`, followed by none, is no name.
fn tool_names_in(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut names = BTreeSet::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(TOOL_PREFIX) {
        let start = from + found;
        from = start + TOOL_PREFIX.len();
        let joined = start
            .checked_sub(1)
            .map(|before| bytes[before])
            .is_some_and(|before| before.is_ascii_alphanumeric() || before == b'_');
        if joined {
            continue;
        }
        let rest = &text[from..];
        let length = rest
            .bytes()
            .take_while(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
            .count();
        let word = rest[..length].trim_end_matches('_');
        if !word.is_empty() {
            names.insert(format!("{TOOL_PREFIX}{word}"));
        }
    }
    names
}

/// The text of the manual's section that begins with `heading`, up to the next section.
fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("the manual has no section {heading:?}"));
    let body = &text[start + heading.len()..];
    let end = body.find("\n## ").unwrap_or(body.len());
    &body[..end]
}

#[test]
fn mcp_manual_names_only_offered_tools() {
    let table = embedded();
    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    let manual = tool_names_in(MANUAL);
    let instructions = tool_names_in(tools::INSTRUCTIONS);
    assert!(
        manual.len() > NAMED.len() && manual.contains(RETIRED) && instructions.contains(RETIRED),
        "setup: the manual names tools, and both name the name no tool takes: {manual:?} \
         {instructions:?}"
    );
    for name in manual.iter().chain(&instructions) {
        if name.as_str() == RETIRED {
            assert!(!catalog.offers(name), "{name} is no tool");
            assert!(
                table
                    .operations
                    .iter()
                    .all(|operation| tools::tool_name(operation) != name.as_str()),
                "{name} is no row's tool"
            );
            continue;
        }
        let operation = table
            .operations
            .iter()
            .find(|operation| tools::tool_name(operation) == name.as_str())
            .unwrap_or_else(|| panic!("{name} is no row's tool"));
        assert_eq!(
            operation.status,
            OperationStatus::Planned,
            "{name}: the tool of a planned row"
        );
        assert!(
            catalog.offers(name) || (!cfg!(windows) && operation.module == OperationModule::Rescue),
            "{name}: the tool of a row this server offers"
        );
    }
    for (row, tool) in NAMED {
        assert_eq!(
            tools::tool_name(
                table
                    .operation(row)
                    .unwrap_or_else(|| panic!("the table has no row {row}"))
            ),
            tool,
            "{row}"
        );
        assert!(
            manual.contains(tool) && instructions.contains(tool),
            "the manual and the instructions name {tool}, the tool of {row}"
        );
    }
    assert!(
        tools::INSTRUCTIONS.contains(&format!("Manual: {MANUAL_PATH}")),
        "the instructions name the manual: {}",
        tools::INSTRUCTIONS
    );
}

#[test]
fn mcp_manual_exception_is_the_refused_rows() {
    let table = embedded();
    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    let named = tool_names_in(section(MANUAL, EXCEPTION_HEADING));
    let detached = table
        .operations
        .iter()
        .filter(|operation| tools::runs_detached(&operation.name))
        .map(tools::tool_name)
        .collect::<BTreeSet<_>>();
    assert!(
        !detached.is_empty(),
        "setup: the front forces rows detached"
    );
    assert_eq!(
        named, detached,
        "the manual's one exception is the rows the front forces detached"
    );
    let refused = table
        .operations
        .iter()
        .filter(|operation| {
            catalog.offers(&tools::tool_name(operation))
                && tools::serving(operation) == Serving::Refused
        })
        .map(tools::tool_name)
        .collect::<BTreeSet<_>>();
    if cfg!(windows) {
        assert_eq!(
            refused, detached,
            "on Windows the offered rows this build refuses are those"
        );
    } else {
        assert!(
            refused.is_empty(),
            "where the rescue module does not run, no offered row is refused: {refused:?}"
        );
    }
}
