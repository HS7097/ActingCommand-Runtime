// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S0 (register §1.287, "Slice LAB-6S0"; contracts/lab-two-entries.md §4.2;
// coordinator work order rd-1113/brief-lab6s0.md): a Lab process takes `Lab-MCP` from
// `ACTINGCOMMAND_INITIATOR` only beside the marker `ACTINGCOMMAND_DETACHED_RUN=rescue`, which the
// rescue module's detached start alone sets.
// - `lab_mcp_from_the_variable_only_with_the_marker`: without the marker every value of the
//   variable gives what LAB-2's rule gives (`Lab-MCP` refused, as before); with it `Lab-MCP` is
//   taken; the marker alone, or beside `Lab-制包`, `Lab-救急`, `Lab-CLI`, `UI` or `Lab-看门狗`,
//   changes nothing, and a marker of another value is none; `serve --stdio --front mcp` still
//   refuses the variable set to anything, `Lab-MCP` included (its rule reads no marker).
// Pure functions of the values given: no environment is changed, no Runtime, no clock, no sleep.

use crate::cli_module::serve;
use actingcommand_contract::outcome::Detail;
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::initiator::{DETACHED_RUN_MARK, ENVIRONMENT_VARIABLE, Initiator};
use serde_json::{Value, json};
use std::ffi::OsStr;

/// The values of the variable a test gives, `None` for one not set.
const VALUES: [Option<&str>; 8] = [
    None,
    Some("Lab-制包"),
    Some("Lab-救急"),
    Some("Lab-MCP"),
    Some("Lab-CLI"),
    Some("UI"),
    Some("Lab-看门狗"),
    Some(""),
];

/// The word the initiator rule gives for the variable `variable` and the marker `marker`, or the
/// variable its refusal names (`actingctl_usage_invalid`).
fn ruled(variable: Option<&str>, marker: Option<&str>) -> Result<Initiator, Value> {
    Initiator::of_lab_process_marked(variable.map(OsStr::new), marker.map(OsStr::new)).map_err(
        |outcome| {
            let value = cli_line::outcome_value(&outcome, Detail::Full);
            assert_eq!(
                value["code"], "actingctl_usage_invalid",
                "{variable:?} {marker:?}"
            );
            value["values"]["variable"].clone()
        },
    )
}

/// What LAB-2's rule, which reads no marker, gives for `variable`, in the same form.
fn without_marker(variable: Option<&str>) -> Result<Initiator, Value> {
    Initiator::of_lab_process(variable.map(OsStr::new)).map_err(|outcome| {
        cli_line::outcome_value(&outcome, Detail::Full)["values"]["variable"].clone()
    })
}

#[test]
fn lab_mcp_from_the_variable_only_with_the_marker() {
    let refused = Err(json!(ENVIRONMENT_VARIABLE));
    let mark = Some(DETACHED_RUN_MARK);

    // Without the marker, as before: LAB-2's rule, `Lab-MCP` refused.
    for value in VALUES {
        assert_eq!(ruled(value, None), without_marker(value), "{value:?}");
    }
    assert_eq!(ruled(Some("Lab-MCP"), None), refused);

    // With the marker, `Lab-MCP` is taken.
    assert_eq!(ruled(Some("Lab-MCP"), mark), Ok(Initiator::LabMcp));

    // The marker alone, or beside another word, changes nothing.
    assert_eq!(ruled(None, mark), Ok(Initiator::LabCli));
    assert_eq!(ruled(Some("Lab-制包"), mark), Ok(Initiator::LabAuthoring));
    assert_eq!(ruled(Some("Lab-救急"), mark), Ok(Initiator::LabRescue));
    for value in ["Lab-CLI", "UI", "Lab-看门狗", "lab-mcp", ""] {
        assert_eq!(ruled(Some(value), mark), refused, "{value:?}");
    }
    for value in VALUES.into_iter().filter(|value| *value != Some("Lab-MCP")) {
        assert_eq!(ruled(value, mark), without_marker(value), "{value:?}");
    }

    // A marker of another value is none, compared byte for byte.
    for other in ["Rescue", "rescue ", "", "Lab-MCP"] {
        assert_eq!(ruled(Some("Lab-MCP"), Some(other)), refused, "{other:?}");
    }

    // `serve --stdio --front mcp` refuses the variable set to anything, as before; its rule
    // reads no marker, so the marker changes nothing there.
    let words = ["--stdio", "--front", "mcp"]
        .iter()
        .map(|word| (*word).to_owned())
        .collect::<Vec<_>>();
    for value in VALUES.into_iter().flatten() {
        let exit = match serve::front(&words, Some(OsStr::new(value))) {
            Ok(front) => panic!("{value:?} must be refused, not read as {front:?}"),
            Err(exit) => exit,
        };
        assert_eq!(exit.code, 1, "{value:?}");
        let line = exit.line.expect("the refusal's line");
        assert_eq!(
            (&line["outcome"]["code"], &line["outcome"]["values"]),
            (
                &json!("actingctl_usage_invalid"),
                &json!({"variable": ENVIRONMENT_VARIABLE})
            ),
            "{value:?}: {line}"
        );
    }
}
