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
mod install_transition;
mod mcp_harness;
mod mcp_instance_alias;
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
