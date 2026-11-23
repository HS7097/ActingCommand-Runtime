// SPDX-License-Identifier: AGPL-3.0-only

//! #463 SEL-F, Addendum 1 (0.12.83; register §1.523; coordinator work order
//! `rd-1123/brief-463-self-fix1.md`, test 11): the package route's MCP face. Its row `package
//! fix-select` of the authoring module is one tool of this server's, named by the table's rule.
//!
//! - `self_t11_package_fix_select_is_offered_where_package_edit_is`: in the tools this build
//!   generates from the operation table it embeds, `ac_package_fix_select` is offered, its row
//!   served by a `serve` child as `package edit`'s is, listed with the authoring module beside
//!   `ac_package_edit` and with its hints (it writes, it is not destructive); its input schema
//!   names `package`, `variable` and `fixed`, the first two required. No Runtime, no clock.

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
fn self_t11_package_fix_select_is_offered_where_package_edit_is() {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    let row = table
        .operation("package fix-select")
        .expect("the table has the row package fix-select");
    let edit = table
        .operation("package edit")
        .expect("the table has the row package edit");
    assert_eq!(row.module, OperationModule::Authoring);
    assert_eq!(row.module, edit.module);
    assert_eq!(tools::tool_name(row), "ac_package_fix_select");
    assert_eq!(tools::serving(row), Serving::Child);
    assert_eq!(tools::serving(row), tools::serving(edit));

    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    assert!(
        catalog.offers("ac_package_fix_select"),
        "the tool is offered"
    );
    assert!(
        catalog.offers("ac_package_edit"),
        "and so is package edit's"
    );
    let listed = catalog.listed(&Listing::Named(vec![OperationModule::Authoring]), &unrooted);
    let definition = |name: &str| -> Value {
        listed
            .iter()
            .find(|tool| tool["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} is listed with the authoring module"))
    };
    let fix = definition("ac_package_fix_select");
    assert_eq!(
        fix["annotations"],
        definition("ac_package_edit")["annotations"],
        "{fix}"
    );
    assert_eq!(fix["annotations"]["readOnlyHint"], false, "{fix}");
    assert_eq!(fix["annotations"]["destructiveHint"], false, "{fix}");
    let schema = &fix["inputSchema"];
    for name in ["package", "variable", "fixed"] {
        assert!(
            schema["properties"].get(name).is_some(),
            "{name} in {schema}"
        );
    }
    assert_eq!(
        schema["required"],
        json!(["package", "variable"]),
        "{schema}"
    );
    for core in [OperationModule::Core, OperationModule::Debugging] {
        let others = catalog.listed(&Listing::Named(vec![core]), &unrooted);
        assert!(
            others
                .iter()
                .all(|tool| tool["name"] != "ac_package_fix_select"),
            "not listed with the module {core:?}"
        );
    }
}
