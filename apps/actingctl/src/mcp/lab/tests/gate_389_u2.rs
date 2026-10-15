// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 U-2 (unified interface model v2.2.3 §3.2 rule 7, §5.2; §8.2 TU-6): an MCP
// option tool hands its work to actinglab, which binds the Lab channel; MCP is the counter that
// forwarded it, so the child is started with `--via mcp` before the tool's own arguments.

use super::super::Program;
use std::path::PathBuf;

#[test]
fn u2_an_option_tool_starts_actinglab_naming_mcp_as_its_forwarding_counter() {
    let program = Program {
        actinglab: PathBuf::from("actinglab.exe"),
        root: PathBuf::from("root"),
        config: PathBuf::from("root").join("actingd.config.json"),
        installation: None,
        state_root: Ok(PathBuf::from("state")),
    };
    let command = program.command(&["lab".to_owned(), "status".to_owned()]);
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(arguments, ["--json", "--via", "mcp", "lab", "status"]);
}
