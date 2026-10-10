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
