// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #503 LIST-A3 (0.12.91; register §1.534, ruling 3): the offline list check's MCP face.
//! Its row `package list-check` of the authoring module is one tool of this server's, named by the
//! table's rule, served as every row of actinglab's own commands is.
//!
//! - `list_a3_the_list_check_is_offered_as_ac_package_list_check`: in the tools this build
//!   generates from the operation table it embeds, `ac_package_list_check` is offered, its row
//!   served by a `serve` child, listed with the authoring module and not with the core or the
//!   debugging module, read-only and not destructive; its input schema takes exactly `package`,
//!   `frames` and `step`, the first two required, and nothing else. No Runtime, no clock.

use crate::tools::{self, Catalog, Listing, Serving};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::locate::AuthoringModule;
use actingcommand_lab_ops::table::{self, OperationModule};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// A place outside any installation root (the listing names its modules, so it is not read).
fn unrooted() -> Result<AuthoringModule, Outcome> {
    Ok(AuthoringModule::Unrooted)
}

#[test]
fn list_a3_the_list_check_is_offered_as_ac_package_list_check() {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    let row = table
        .operation("package list-check")
        .expect("the table has the row package list-check");
    assert_eq!(row.module, OperationModule::Authoring);
    assert!(!row.writes, "it writes nothing");
    assert_eq!(tools::tool_name(row), "ac_package_list_check");
    assert_eq!(tools::serving(row), Serving::Child);

    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    assert!(
        catalog.offers("ac_package_list_check"),
        "the tool is offered"
    );
    let listed = catalog.listed(&Listing::Named(vec![OperationModule::Authoring]), &unrooted);
    let definition = listed
        .iter()
        .find(|tool| tool["name"] == "ac_package_list_check")
        .expect("ac_package_list_check is listed with the authoring module");
    assert_eq!(
        definition["annotations"],
        json!({"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false}),
        "{definition}"
    );
    let schema = &definition["inputSchema"];
    let names = schema["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    assert_eq!(
        names,
        ["frames", "package", "step"]
            .map(str::to_owned)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "{schema}"
    );
    assert_eq!(schema["required"], json!(["package", "frames"]), "{schema}");
    assert_eq!(
        schema["additionalProperties"],
        Value::Bool(false),
        "{schema}"
    );
    for other in [OperationModule::Core, OperationModule::Debugging] {
        let others = catalog.listed(&Listing::Named(vec![other]), &unrooted);
        assert!(
            others
                .iter()
                .all(|tool| tool["name"] != "ac_package_list_check"),
            "not listed with the module {other:?}"
        );
    }
}
