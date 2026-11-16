// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"; register §1.402): the Lab's
//! MCP server's tools of the rows `tap`, `swipe` and `long-tap` take the rows' new parameters.
//!
//! - `s11_ac_tap_takes_record_and_after_ms`: in the tools this build generates from the operation
//!   table it embeds, `ac_tap`, `ac_swipe` and `ac_long_tap` each list `record` (a boolean) and
//!   `after_ms` (an integer), neither required. No Runtime, no clock.

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
fn s11_ac_tap_takes_record_and_after_ms() {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    let listed = catalog.listed(&Listing::Named(vec![OperationModule::Debugging]), &unrooted);
    for name in ["ac_tap", "ac_swipe", "ac_long_tap"] {
        let tool = listed
            .iter()
            .find(|definition| definition["name"] == name)
            .unwrap_or_else(|| panic!("{name} is listed"));
        let properties = &tool["inputSchema"]["properties"];
        assert_eq!(properties["record"]["type"], "boolean", "{name}: {tool}");
        assert_eq!(properties["after_ms"]["type"], "integer", "{name}: {tool}");
        let required = tool["inputSchema"]["required"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for member in ["record", "after_ms"] {
            assert!(
                !required.contains(&json!(member)),
                "{name}: {member} is required: {tool}"
            );
        }
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| text.contains("with record")),
            "{name}: {tool}"
        );
        assert_eq!(
            tool["inputSchema"]["additionalProperties"],
            Value::Bool(false)
        );
    }
}
