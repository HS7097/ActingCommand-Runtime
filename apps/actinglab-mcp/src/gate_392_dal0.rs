// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-L0 (0.12.83; register §1.521; coordinator work order
//! `rd-1123/brief-392-dal0.md`, test 8): the layout probe's MCP face. Its row `layout probe` of the
//! authoring module is one tool of this server's, named by the table's rule.
//!
//! - `dal0_t8_the_layout_probe_is_offered_as_ac_layout_probe`: in the tools this build generates
//!   from the operation table it embeds, `ac_layout_probe` is offered, its row served by a `serve`
//!   child, listed with the authoring module, read-only and not destructive; its input schema lists
//!   exactly `scene`, `package`, `zip`, `page` and `tolerance_px`, in that order, `scene` alone
//!   required, `tolerance_px` an integer from 0 with the default 4. No Runtime, no clock.

use crate::tools::{self, Catalog, Listing, Serving};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::locate::AuthoringModule;
use actingcommand_lab_ops::table::{self, OperationModule};
use serde_json::{Value, json};

/// A place outside any installation root (the listing names its modules, so it is not read).
fn unrooted() -> Result<AuthoringModule, Outcome> {
    Ok(AuthoringModule::Unrooted)
}

#[test]
fn dal0_t8_the_layout_probe_is_offered_as_ac_layout_probe() {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    let row = table
        .operation("layout probe")
        .expect("the table has the row layout probe");
    assert_eq!(row.module, OperationModule::Authoring);
    assert_eq!(tools::tool_name(row), "ac_layout_probe");
    assert_eq!(tools::serving(row), Serving::Child);

    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    assert!(catalog.offers("ac_layout_probe"), "the tool is offered");
    let listed = catalog.listed(&Listing::Named(vec![OperationModule::Authoring]), &unrooted);
    let definition = listed
        .iter()
        .find(|definition| definition["name"] == "ac_layout_probe")
        .expect("ac_layout_probe is listed with the authoring module");
    assert_eq!(
        definition["annotations"],
        json!({"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false}),
        "{definition}"
    );
    let schema = &definition["inputSchema"];
    let names = schema["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    assert_eq!(
        names,
        ["scene", "package", "zip", "page", "tolerance_px"],
        "{schema}"
    );
    assert_eq!(schema["required"], json!(["scene"]), "{schema}");
    assert_eq!(
        schema["additionalProperties"],
        Value::Bool(false),
        "{schema}"
    );
    let tolerance = &schema["properties"]["tolerance_px"];
    assert_eq!(tolerance["type"], "integer", "{tolerance}");
    assert_eq!(tolerance["minimum"], 0, "{tolerance}");
    assert_eq!(tolerance["default"], 4, "{tolerance}");
}
