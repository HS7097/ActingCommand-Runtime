// SPDX-License-Identifier: AGPL-3.0-only

//! The Lab's operation layer (Workflow #434 LAB-1; `contracts/lab-two-entries.md` §4, §8 and
//! §9; register §1.287).
//!
//! - [`table`]: the operation table `contracts/lab-operations.json` read into typed rows, the
//!   one source of the Lab's operations (contract §9.1).
//! - [`initiator`]: the six words that say where a request or an operation came from (§4.1).
//! - [`locate`]: the default location of an installed Lab program, its root and its state root,
//!   with no `--root` and no `--state-root` (§9.3).
//! - actingctl's read and write modules (register §1.287, LAB-1 row) and the helpers they
//!   share, moved here unchanged: actingctl calls them from this crate and answers as before
//!   (same words, output, codes and exit codes) until LAB-7 deletes it. Their texts still say
//!   `actingctl`, the program that runs them. `codes` stays the registry of the catalog owner
//!   `actingctl` (`contracts/outcome-codes/actingctl.json`), whose crate this now is.
//! - Workflow #434 LAB-1b: the other 23 core operations, whose code was in actingctl's `main.rs`
//!   and in MCP's tools (register §1.287, LAB-1b row): a function per operation that sends its
//!   requests on a connection it is given (for `app`, through the actinglab child it runs) and
//!   shapes its answer, which actingctl's CLI and MCP server call and wrap as before; [`served`]
//!   names the function that serves each core operation of LAB-1 and LAB-1b.
//! - Workflow #434 LAB-5D: [`budget`], the answer budget. It is the caller's: the reads whose MCP
//!   answers were cut or exported in MCP's own code (`status`, `diagnose`, `events`, `schedule`,
//!   `runs`, `run evidence`, `codes`; `facts`, `packs`, `remedy`, `summary`) do it here, each in
//!   its module's `within_budget`, only when the caller passes a budget.
//! - Workflow #434 LAB-5G1: [`core_ops`], the code that serves each row actinglab's CLI module
//!   served itself, from the row and its parameters, moved from actinglab.

#![forbid(unsafe_code)]

// Workflow #378 A5: the one result line of every exit; Workflow #434 LAB-2: built and returned
// here, printed by the program.
pub mod cli_line;
// Workflow #389 MCP-14: `codes` (MCP `ac_outcome_code`), entries of the embedded outcome catalog.
pub mod code_lookup;
// actingctl's outcome codes (Workflow #378 A2a).
pub mod codes;
pub mod events;
// Workflow #389 MCP-4: `facts --program | --instance` and `status --config` (MCP `ac_facts`).
pub mod facts;
pub mod packs;

// Workflow #389 MCP-5a: `observe --export` (MCP `ac_observe`), with the instance selector and the
// material export writer it shares with MCP.
pub mod instance_selector;
pub mod material_export;
pub mod observe_export;

// Workflow #389 MCP-3: `runs` (MCP `ac_runs`); it uses MCP-5a's instance selector above.
pub mod run_list;

// Workflow #389 MCP-7: `diagnose` (MCP `ac_diagnose`), with the child-process runner it shares
// with MCP for `actingd suspended`.
pub mod child;
pub mod diagnose;

// Workflow #389 MCP-8: `overview` (MCP `ac_overview`), with the read both share.
pub mod overview;

// Workflow #389 MCP-6: `run evidence` (MCP `ac_run_evidence`).
pub mod run_evidence;

// Workflow #398 RD-1c: `devices list|bind|unbind` (MCP `ac_devices`, `ac_device_bind`,
// `ac_device_unbind`).
pub mod devices;

// Workflow #410 item 6: `capability-gap` (MCP `ac_capability_gap`), the local capability-gap log.
pub mod capability_gap;

// Workflow #411 B15 R6: `remedy set|get|lift` (MCP `ac_remedy_set`, `ac_remedy_get`), an
// instance's remedy choices.
pub mod remedy;

// #422 LN-7c C: `evidence confirm-read` (MCP `ac_evidence_confirm_read`), the confirmation that an
// evidence frame or a run was read.
pub mod evidence_read;

// Workflow #386 SR-6b, MCP-19b: `schedule` (MCP `ac_schedule`), the latest policy evaluation's
// explanation, and what `overview` and `diagnose` show of it.
pub mod schedule;

// Workflow #429 R-2: `perf-preset set|get` (MCP `ac_perf_preset`), the performance presets.
pub mod perf_preset;

// Workflow #429 R-12: `routine-summary` (MCP `ac_routine_summary`), the routine summary's health
// statistics.
pub mod routine_summary;

// Workflow #434 LAB-1: the operation table, the initiator and the default location.
pub mod initiator;
pub mod locate;
pub mod table;

// Workflow #434 LAB-1b: the other core operations (register §1.287, LAB-1b row), by group of the
// table: runs and materials (`run show`, `material`, `targets`, `targets set`, `frames`),
// scheduling (`pause`, `resume`, `offset`), configuration and monitors (`config set`, `config
// clear`, `monitor set`, `monitor clear`), facts (`facts publish`), emulators (`discover`, `emu
// start|stop|restart|selfcheck`), games (`app launch|restart|stop`) and the rest (`shutdown`,
// `wake-test`); with the bounded wait of `emu start|stop|restart`, and the shutdown wait and the
// process probe it reads, moved from actingctl unchanged; and `lab_child`, actinglab run as a
// child and its envelope read, which `app` runs and MCP's other Lab tools share.
pub mod apps;
pub mod configure;
pub mod emulators;
pub mod fact_publish;
pub mod frames;
pub mod lab_child;
pub mod material;
pub mod process_probe;
pub mod run_show;
pub mod scheduling;
pub mod shutdown;
pub mod shutdown_wait;
pub mod targets;
pub mod wait;
pub mod wake_test;

// Workflow #434 LAB-1b: the registry of the core operations this crate serves.
pub mod served;

// Workflow #434 LAB-2: the program whose command lines the moved commands serve (actingctl's CLI
// channel, or actinglab's Lab channel), and `drill` (actingctl's `ladder-drill`, MCP
// `ac_ladder_drill`), moved from actingctl so that actinglab's CLI module serves it too.
pub mod front;
pub mod ladder_drill;

// Workflow #434 LAB-5D: the answer budget, the caller's: what it passes to say that an answer
// must fit and where one that does not is written whole; the reads that cut or export take it.
pub mod budget;

// Workflow #434 LAB-5G1: the code that serves the rows actinglab's CLI module served itself (the
// core rows but `app launch|restart|stop`, and `run start`, `run stop`, `reset`, `drill`), moved
// from actinglab so that another program serves them with it; the program gives what stays its
// own (`core_ops::Caller`).
pub mod core_ops;

// Workflow #434 LAB-5B1: the parameters the code of each row `core_ops` serves reads, one constant
// per row, which that code reads them through and builds a row's command line from.
pub mod core_reads;

// Workflow #467 RPT-1: `report export`, one problem report archive of the errors view (the newest
// failed and unfinished runs, the ledger's errors view, the versions), which `core_ops` serves.
pub mod report;

// Test-only: Workflow #434 LAB-1, the slice's named tests (`lab_ops_table_complete`,
// `lab_ops_locate_defaults`).
#[cfg(test)]
mod gate_434_lab1;
// Test-only: Workflow #434 LAB-1b, the slice's named test (`lab_ops_core_rows_served_by_lab_ops`).
#[cfg(test)]
mod gate_434_lab1b;
// Test-only: Workflow #434 LAB-5D, the slice's named tests (`lab_ops_no_budget_whole_answer`,
// `lab_ops_budget_cuts_keep_their_markers`, `lab_ops_budget_exports_the_header`).
#[cfg(test)]
mod gate_434_lab5d;
// Test-only: Workflow #434 LAB-5G1b, the slice's named test of the table
// (`lab_ops_undeclared_members_follow_the_table`).
#[cfg(test)]
mod gate_434_lab5g1b;
// Test-only: Workflow #434 LAB-5B1, the slice's named tests of the core rows' constants
// (`core_reads`): each held equal to its row, the command lines held to their parsers.
#[cfg(test)]
mod gate_434_lab5b1;
// Test-only: Workflow #438 HELD-1, the diagnose half of `held_t9` (the instance's held rows and
// the mark on its `not_run` rows).
#[cfg(test)]
mod gate_438_held;
// Test-only: Workflow #434 LAB-6S1a, the six initiator words this crate writes are the contract
// crate's (`lab_ops_initiator_words_are_the_contracts`).
#[cfg(test)]
mod gate_434_lab6s1a;
// Test-only: Workflow #434 LAB-6S3, the contract crate's UI list is the table's `ui_kinds`
// (`lab_ops_ui_allows_is_the_tables_ui_kinds`).
#[cfg(test)]
mod gate_434_lab6s3;
// Test-only: Workflow #467 RPT-1, the slice's named tests of `report export` (the summary from
// made-up evidence, the stream filter, the frames, the output's refusals, the secrets and masks,
// the reads, no frames, the size bound, the errors view and the table's row).
#[cfg(test)]
mod gate_467_rpt1;
// Test-only: ROUT-L RL-1c (#411), the standard-pack game root fixtures of `packs::tests`.
#[cfg(test)]
#[path = "../../../tests/support/standard_pack_schedule.rs"]
mod standard_pack_schedule_fixture;

use actingcommand_contract::SanitizationError;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Category, IoOp, Outcome, RawSource};
use std::fmt;
use std::io::{self, Read};
use std::path::Path;

/// Workflow #382 A1-FLAG: the flags a subcommand of this crate takes (`devices`, `remedy`,
/// `evidence`, `perf-preset` and `run` by their action), as its usage lists them; `None` for a
/// word that is no such subcommand. Any other flag, a flag given twice and a word that is no flag
/// are refused by name (Fail Loud: nothing is ignored silently). Workflow #434 LAB-1: these are
/// the arms of actingctl's `flags_of` for the commands whose modules moved here, unchanged;
/// actingctl's own table asks this one for every other word.
pub fn flags_of(command: &str, action: Option<&str>) -> Option<&'static [&'static str]> {
    let flags: &'static [&'static str] = match command {
        // Workflow #389 MCP-14: the list flags of `codes`; its code is a positional word.
        "codes" => &["--prefix", "--owner", "--limit", "--cursor"],
        // Workflow #389 MCP-1: `events`, one flag per parameter of `ac_events`
        // (`events::PARAMETERS`); MCP-2 adds `--event-types` and `--fact-key`.
        "events" => &[
            "--state-root",
            "--instance",
            "--view",
            "--since-unix-ms",
            "--until-unix-ms",
            "--after-sequence",
            "--through-sequence",
            "--event-type",
            "--event-types",
            "--exclude-event-types",
            "--fact-key",
            "--min-severity",
            "--max-severity",
            "--origin-module",
            "--task-id",
            "--run-id",
            "--request-id",
            "--correlation-id",
            "--detail",
            "--limit",
            "--cursor",
            "--wait-s",
        ],
        // Workflow #389 MCP-6: `run evidence`, by request_id or run_id, a page of steps.
        "run" if action == Some(run_evidence::ACTION) => &[
            "--state-root",
            "--request-id",
            "--run-id",
            "--from-step",
            "--limit",
            // #411 A42: whole steps or step summaries.
            "--detail",
        ],
        // Workflow #398 RD-1c: `devices`; the alias and the handle are positional words.
        "devices" if matches!(action, Some(devices::LIST | devices::BIND)) => {
            &["--state-root", "--root"]
        }
        "devices" if action == Some(devices::UNBIND) => &["--state-root"],
        // Workflow #410 item 6: both texts are required; no root is read.
        capability_gap::COMMAND => &["--capability", "--why"],
        // Workflow #411 B15 R6: `remedy set` writes one scope's rows, an instance's or a game's
        // (no `--server`: a server id is not game-qualified); `get` and `lift` take one instance.
        remedy::COMMAND if action == Some(remedy::SET) => {
            &["--state-root", "--instance", "--game", "--rows"]
        }
        remedy::COMMAND if matches!(action, Some(remedy::GET | remedy::LIFT)) => {
            &["--state-root", "--instance"]
        }
        // #422 LN-7c C: a confirmation names one frame (`--artifact`) or one run (`--run`).
        evidence_read::COMMAND if action == Some(evidence_read::CONFIRM_READ) => {
            &["--state-root", "--artifact", "--run"]
        }
        // Workflow #429 R-2: `set` names its preset as a word; both read the Runtime's facts.
        perf_preset::COMMAND if matches!(action, Some(perf_preset::SET | perf_preset::GET)) => {
            &["--state-root"]
        }
        // Workflow #429 R-12: one width flag (no value), the instance and the cursor.
        routine_summary::COMMAND => &[
            "--state-root",
            routine_summary::HOUR,
            routine_summary::DAY,
            "--instance",
            routine_summary::SINCE,
        ],
        _ => return None,
    };
    Some(flags)
}

/// Reads at most `limit` bytes of an input file; a longer file is `TooLarge`.
pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, InputProblem> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| InputProblem::Io(error, IoOp::Open))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| InputProblem::Io(error, IoOp::Read))?;
    if bytes.len() > limit {
        return Err(InputProblem::TooLarge(limit));
    }
    Ok(bytes)
}

/// Why a subcommand's bounded input file (`--record-file`, `--policy-file`, `--rows`) cannot be
/// used.
#[derive(Debug)]
pub enum InputProblem {
    /// Opening (`IoOp::Open`) or reading (`IoOp::Read`) it failed.
    Io(io::Error, IoOp),
    /// It holds no byte.
    Empty,
    /// It is larger than its bound, in bytes.
    TooLarge(usize),
    /// Its text is not UTF-8.
    NotUtf8(std::string::FromUtf8Error),
    /// Its JSON does not decode.
    Json(serde_json::Error),
    /// The contract refuses its content.
    Invalid(SanitizationError),
}

impl InputProblem {
    /// `top` with what this problem adds: the I/O or decode failure as its `caused_by` link
    /// (`foreign_os_error`, `foreign_data_error`), the size, or the contract's refusal code.
    pub fn attach(&self, top: Outcome) -> Outcome {
        match self {
            Self::Io(error, op) => top.caused_by(Outcome::from_io_error(error, *op)),
            Self::Empty => top.actual_bytes(0_u64),
            Self::TooLarge(limit) => top.limit_bytes(*limit),
            Self::NotUtf8(error) => top.caused_by(foreign_data_error(error)),
            Self::Json(error) => top.caused_by(foreign_data_error(error)),
            Self::Invalid(error) => top.caused_by(Outcome::new(error.code())),
        }
    }
}

/// `foreign_data_error` for JSON text a decoder refused: the decoder's message verbatim as
/// `raw_text` (spec §7).
pub fn foreign_data_error(error: &impl fmt::Display) -> Outcome {
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(error.to_string())
}

/// `top` caused by `cause`. A fatal cause is the top itself: a fatal link needs a fatal top
/// (spec §5), and the operation ended in that condition. Workflow #434 LAB-1b: moved from
/// actingctl's `main.rs`, where `install-transition` reads it too.
pub fn with_cause(top: Outcome, cause: Outcome) -> Outcome {
    if cause.category() == Category::Fatal {
        cause
    } else {
        top.caused_by(cause)
    }
}
