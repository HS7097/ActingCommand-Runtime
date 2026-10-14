// SPDX-License-Identifier: AGPL-3.0-only

//! The gate tests of actingctl that run its binary (test plan §2.1: one gate test binary per
//! crate, one module per work item).

#[allow(dead_code)]
#[path = "../../../../tests/support/emulator_controller.rs"]
mod emulator_controller;
#[allow(dead_code)]
#[path = "../../../../tests/support/held_runtime.rs"]
mod held_runtime;

mod emulator_restart;
// Workflow #382 A1-FLAG: flags a subcommand does not take, flags given twice, stray words.
mod flag_refusals;
mod install_transition;
// Workflow #389 MCP-14: `actingctl codes` ↔ `ac_outcome_code`.
mod mcp_codes_parity;
// Workflow #389 MCP-7: `actingctl diagnose` ↔ `ac_diagnose`.
mod mcp_diagnose_parity;
mod mcp_events_parity;
// Workflow #389 MCP-4: `actingctl facts`, `status --config` ↔ `ac_facts`.
mod mcp_facts_parity;
mod mcp_harness;
mod mcp_instance_alias;
mod mcp_observe_parity;
mod mcp_packs;
mod mcp_runs_parity;
mod mcp_shell;

#[allow(dead_code)]
#[path = "../../../../tests/support/c4_runtime.rs"]
mod c4_runtime;

/// The child entry of the c4 fake Runtime: `c4_runtime::RuntimeChild` runs this binary with
/// `--exact c4_runtime_child_process`; without its environment it returns at once.
#[test]
fn c4_runtime_child_process() {
    c4_runtime::run_child_if_requested();
}
