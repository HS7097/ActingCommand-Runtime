// SPDX-License-Identifier: AGPL-3.0-only

//! #480 PG-2e (0.12.75; register §1.485; coordinator work order
//! `rd-1119/brief-480-pg2e-status-answers-the-pause.md`, test 5): the status answers an instance
//! pause's mode and what is in its way, and `ac_status` passes them through; the MCP tools of the
//! rows a pause is read and asked with take the parameters they took.
//!
//! - `pg2e_ac_status_and_ac_pause_take_the_parameters_they_took`: in the tools this build
//!   generates from the operation table it embeds, `ac_status` lists exactly `instance` (a string)
//!   and `sections` (an array), neither required, and `ac_pause` exactly `instance`, `reason` and
//!   `drain`, `reason` alone required; no other property. No Runtime, no clock.

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
fn pg2e_ac_status_and_ac_pause_take_the_parameters_they_took() {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    let listed = catalog.listed(&Listing::Named(vec![OperationModule::Core]), &unrooted);
    let schema_of = |name: &str| {
        listed
            .iter()
            .find(|definition| definition["name"] == name)
            .unwrap_or_else(|| panic!("{name} is listed"))["inputSchema"]
            .clone()
    };

    let status = schema_of("ac_status");
    let mut names = status["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    names.sort();
    assert_eq!(names, ["instance", "sections"], "{status}");
    assert_eq!(
        status["properties"]["instance"]["type"], "string",
        "{status}"
    );
    assert_eq!(
        status["properties"]["sections"]["type"], "array",
        "{status}"
    );
    assert_eq!(status.get("required"), None, "{status}");
    assert_eq!(
        status["additionalProperties"],
        Value::Bool(false),
        "{status}"
    );

    let pause = schema_of("ac_pause");
    let mut names = pause["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    names.sort();
    assert_eq!(names, ["drain", "instance", "reason"], "{pause}");
    assert_eq!(pause["required"], json!(["reason"]), "{pause}");
    assert_eq!(pause["additionalProperties"], Value::Bool(false), "{pause}");
}
