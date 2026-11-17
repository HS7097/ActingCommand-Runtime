// SPDX-License-Identifier: AGPL-3.0-only

//! #398 DB-1 (register §1.460; `contracts/device-binding.md`, "Choosing a device"; coordinator work
//! order `rd-1117/brief-398-db1-bind-candidate-config.md`, its test 5): the MCP part of the slice,
//! which only this server's code reaches; the rest of the slice's tests are actingd's
//! `gate_398_db1`, which reaches check-config.
//!
//! - `db1_t5_the_mcp_tool_takes_config_and_leaves_bound_via_out`: the tool of `devices bind`,
//!   `ac_devices_bind`, has exactly `instance`, `handle` and `config` (a string whose description
//!   asks for an absolute path), requires the first two and takes no other member (#398 DB-2,
//!   adapted: and `device_key`, the handle no longer required, register §1.467); it has no
//!   `bound_via` (the front leaves it out, `tools::SUPPLIED`) and no `via`. The front hands an
//!   absolute `config` on as given and adds no `bound_via`; a `bound_via` or a `via` sent is refused
//!   `arguments_invalid`, `unknown`, as any argument the tool does not know.
//!
//! The embedded table and this server's own code only: no Runtime, no device, no clock.

use crate::calls;
use crate::tools::{self, Catalog, Listing};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::locate::AuthoringModule;
use actingcommand_lab_ops::table::{self, OperationModule};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// The tool of the row `devices bind`, by the table's rule.
const TOOL: &str = "ac_devices_bind";

/// A place outside any installation root, where every module is listed.
fn unrooted() -> Result<AuthoringModule, Outcome> {
    Ok(AuthoringModule::Unrooted)
}

/// The tool's required members and `extra`.
fn arguments(extra: &[(&str, Value)]) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("instance".to_owned(), json!("neutral.phone"));
    arguments.insert("handle".to_owned(), json!("usb:7"));
    for (member, value) in extra {
        arguments.insert((*member).to_owned(), value.clone());
    }
    arguments
}

#[test]
fn db1_t5_the_mcp_tool_takes_config_and_leaves_bound_via_out() {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    let catalog = Catalog::of(&table).expect("the table's tools are generated");
    let listed = catalog.listed(&Listing::Named(vec![OperationModule::Core]), &unrooted);
    let tool = listed
        .iter()
        .find(|definition| definition["name"] == TOOL)
        .unwrap_or_else(|| panic!("{TOOL} is listed"));
    let schema = &tool["inputSchema"];
    let properties = schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("{TOOL} has properties: {tool}"));
    // #398 DB-2 (adapted: ruling 3 adds the device key in place of the handle, ruling 6 and the
    // coordinator's addition leave both forms of its salt out).
    assert_eq!(
        properties
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["config", "device_key", "handle", "instance"]),
        "{TOOL}: its members: {tool}"
    );
    assert_eq!(properties["config"]["type"], "string", "{tool}");
    assert!(
        properties["config"]["description"]
            .as_str()
            .is_some_and(|text| text.ends_with(tools::ABSOLUTE_PATH)),
        "config takes an absolute path: {tool}"
    );
    assert!(
        properties.get("bound_via").is_none() && properties.get("via").is_none(),
        "no channel through MCP: {tool}"
    );
    // #398 DB-2 (adapted: the handle is no longer required, the device key may take its place).
    assert_eq!(schema["required"], json!(["instance"]), "{tool}");
    assert_eq!(schema["additionalProperties"], false, "{tool}");

    // What the front hands the serving code, and what it refuses.
    let operation = catalog
        .operation(TOOL)
        .unwrap_or_else(|| panic!("{TOOL} is an offered row's tool"));
    let exports = tempfile::tempdir().expect("tempdir");
    let candidate = exports
        .path()
        .join("candidate.json")
        .to_str()
        .expect("the temporary folder is UTF-8")
        .to_owned();
    let taken = calls::prepare(
        operation,
        &arguments(&[("config", json!(candidate))]),
        exports.path(),
    )
    .unwrap_or_else(|refused| panic!("config is taken: {}", refused.outcome));
    assert_eq!(
        taken,
        arguments(&[("config", json!(candidate))]),
        "config is handed on as given, and nothing is added"
    );
    for member in ["bound_via", "via"] {
        let refused = calls::prepare(
            operation,
            &arguments(&[("config", json!(candidate)), (member, json!("ui"))]),
            exports.path(),
        )
        .expect_err("a channel sent through MCP is refused");
        let outcome = cli_line::outcome_value(&refused.outcome, Detail::Full);
        assert_eq!(outcome["code"], "arguments_invalid", "{member}: {outcome}");
        assert_eq!(
            (
                &outcome["values"]["argument"],
                &outcome["values"]["argument_problem"]
            ),
            (&json!(member), &json!("unknown")),
            "{member}: {outcome}"
        );
    }
}
