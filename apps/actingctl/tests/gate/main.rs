// SPDX-License-Identifier: AGPL-3.0-only

//! The gate tests of actingctl that run its binary (test plan §2.1: one gate test binary per
//! crate, one module per work item).

#[allow(dead_code)]
#[path = "../../../../tests/support/emulator_controller.rs"]
mod emulator_controller;
#[allow(dead_code)]
#[path = "../../../../tests/support/held_runtime.rs"]
mod held_runtime;

// Workflow #382 A-1: usage errors before any connection, and refused requests' exits.
mod cli_grammar;
mod emulator_restart;
// Workflow #389 MCP-21: `actingctl emulator start|stop|restart --wait`.
mod emulator_wait;
// Workflow #382 A1-FLAG: flags a subcommand does not take, flags given twice, stray words.
mod flag_refusals;
mod install_transition;
// Workflow #389 MCP-13a: `ac_app` is an operator tool.
mod mcp_app;
// Workflow #389 MCP-14: `actingctl codes` ↔ `ac_outcome_code`.
mod mcp_codes_parity;
// Workflow #398 RD-1c: `actingctl devices` ↔ `ac_devices`, `ac_device_bind`, `ac_device_unbind`.
mod mcp_devices_parity;
// Workflow #389 MCP-7: `actingctl diagnose` ↔ `ac_diagnose`.
mod mcp_diagnose_parity;
mod mcp_events_parity;
// Workflow #389 MCP-4: `actingctl facts`, `status --config` ↔ `ac_facts`.
mod mcp_facts_parity;
mod mcp_harness;
mod mcp_instance_alias;
// Workflow #390 DRILL-1: `actingctl ladder-drill` ↔ `ac_ladder_drill`.
mod mcp_ladder_drill_parity;
mod mcp_observe_parity;
// Workflow #389 MCP-8: `actingctl overview` ↔ `ac_overview`.
mod mcp_overview_parity;
mod mcp_packs;
// Workflow #389 MCP-6: `actingctl run evidence` ↔ `ac_run_evidence`.
mod mcp_run_evidence_parity;
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
