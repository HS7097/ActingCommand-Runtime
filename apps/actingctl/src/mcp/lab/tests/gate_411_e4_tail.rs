// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 E4 tail (B34 follow-up; apps key 97): actinglab `package check` answers
// `warnings` (register §1.156 E4-3), and `ac_pack_check` answers that data verbatim, so the
// declared `ac_pack_check` result schema names the `warnings` member the result carries, and an
// agent reading the tool list knows to look for it.

use super::super::super::protocol::Era;
use super::super::super::tools::{definition, find};

#[test]
fn e4_tail_t1_ac_pack_check_result_schema_names_its_warnings() {
    let tool = find("ac_pack_check").expect("ac_pack_check is in the tool table");
    let described = definition(tool, Era::Modern);
    let result = described
        .pointer("/outputSchema/properties/result")
        .unwrap_or_else(|| panic!("ac_pack_check declares no result schema: {described}"));
    let warnings = result
        .pointer("/properties/warnings")
        .unwrap_or_else(|| panic!("the result schema names no warnings member: {result}"));
    assert_eq!(warnings["type"], "array", "{result}");
    let description = result["description"]
        .as_str()
        .unwrap_or_else(|| panic!("the result schema has no description: {result}"));
    assert!(description.contains("warnings"), "{description}");
}
