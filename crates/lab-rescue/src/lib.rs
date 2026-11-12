// SPDX-License-Identifier: AGPL-3.0-only

//! The Lab's rescue and installation module (Workflow #434 LAB-3; `contracts/lab-two-entries.md`
//! §5 to §7; register §1.287, "Slice LAB-3").
//!
//! - **What moved in** (§7), unchanged: `actingctl install` ([`install`], with `mcp-entry plan|list`
//!   that MCP's entry asks), `actingctl start` and `restart` ([`runtime_start`]) and `actingctl
//!   agent-channel` ([`agent_channel`]). actingctl keeps answering them from here until LAB-7
//!   deletes it, with its `install-cli` revision-1 face: the same words, lines, codes and exits.
//! - **The rescue module** ([`rescue`]): the table's `rescue` rows on those commands, which
//!   actinglab's CLI module and `serve --stdio` serve: no `--root` and no `--state-root` (the root
//!   is the one of the program that runs it), the journal's `initiator`, no confirmation
//!   parameters (§5), guards that report and never refuse (§6), and the composites `deploy`,
//!   `config replace`, `place` and `alive`.
//!
//! `crates/watchdog` reads actingctl's codes and result line from `crates/lab-ops`, so `lab-ops`
//! cannot link it, and the installer and the starts need it: this module is a crate of its own
//! above both. Like the watchdog it runs on Windows only.

#![cfg(windows)]
#![forbid(unsafe_code)]

// Workflow #432 AGT-T1: `agent-channel` (`contracts/agent-forward.md`), moved from actingctl's
// `main.rs` with its answers; the rescue module's `agent …` runs it too.
pub mod agent_channel;
// Workflow #421 INST-1: `actingctl install`, the command-line installer of a single-location root.
pub mod install;
// Workflow #434 LAB-3: the rescue module's operations.
pub mod rescue;
// Workflow #434 LAB-5B1b: the parameters the code of each rescue row reads, and the command line
// each row that hands one on builds, one constant per row.
pub mod rescue_reads;
// Workflow #389 MCP-10: `actingctl start` and `restart`.
pub mod runtime_start;

// Test-only: Workflow #390 WD-1, the gate of `start` without `--config`, moved with `start`.
#[cfg(test)]
mod gate_wd1_start;

// actingctl's modules the moved code reads, from `crates/lab-ops` (LAB-1, LAB-1b) and
// `crates/watchdog` (LAB-4), bound at the root so that every `crate::<module>` path of the moved
// files names them unchanged.
use actingcommand_lab_ops::{child, cli_line, codes, process_probe};
use actingcommand_watchdog as watchdog;
