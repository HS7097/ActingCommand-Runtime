// SPDX-License-Identifier: AGPL-3.0-only

//! #480 PG-2d (0.12.74; register §1.481; coordinator work order
//! `rd-1118/brief-480-pg2d-pause-records-resume-cause.md`, test 4): the Lab's `resume` row answers
//! the receipt's cause, and its MCP tool takes the parameters it took.
//!
//! - `pg2d_ac_resume_takes_the_parameters_it_took`: in the tools this build generates from the
//!   operation table it embeds, `ac_resume` lists exactly `instance` (a string) and `expect`,
//!   `expect` alone required, no other property. No Runtime, no clock.

use crate::tools::{Catalog, Listing};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::locate::AuthoringModule;
use actingcommand_lab_ops::table::{self, OperationModule};
use serde_json::{Value, json};

/// A place outside any installation root (the listing names its modules, so it is not read).
fn unrooted() -> Result<AuthoringModule, Outcome> {
    Ok(AuthoringModule::Unrooted)
}

#[test]
fn pg2d_ac_resume_takes_the_parameters_it_took() {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    let listed = catalog.listed(&Listing::Named(vec![OperationModule::Core]), &unrooted);
    let tool = listed
        .iter()
        .find(|definition| definition["name"] == "ac_resume")
        .unwrap_or_else(|| panic!("ac_resume is listed"));
    let schema = &tool["inputSchema"];
    let mut names = schema["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    names.sort();
    assert_eq!(names, ["expect", "instance"], "{tool}");
    assert_eq!(schema["properties"]["instance"]["type"], "string", "{tool}");
    assert_eq!(schema["required"], json!(["expect"]), "{tool}");
    assert_eq!(schema["additionalProperties"], Value::Bool(false), "{tool}");
}
