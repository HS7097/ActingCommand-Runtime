// SPDX-License-Identifier: AGPL-3.0-only

//! ActingCommand Lab authoring and debug adapter, a required workspace member.
//!
//! Production Runtime, scheduler, device, and ledger ownership live outside this
//! crate. Lab consumes stable contracts and injected ports so production remains
//! buildable and runnable when this crate is excluded.
//!
//! Package inspection uses `Lab::lab_validate`, `validate_lab_package_bytes` or
//! `validate_lab_package_path`.
//! Execution clients submit contained tasks through the Runtime client; the
//! `actinglab lab run` command uses that same Runtime-owned execution path.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

mod codes;
mod context;
// Workflow #392 DA-5cd: an offline scene that is a pure scale of its package, and a recording made
// on a display that is not the base size.
mod display_base;
mod drive;
mod drive_api;
mod env_api;
mod env_detection;
// Workflow #396 REC-4 (exec key 65): a Lab recording's click follows a template mark.
#[cfg(test)]
mod gate_396_rec4;
// Workflow #336 PKG-S5 (exec key 66): record stop writes `<package_id>.<ext>`, replaced in place.
#[cfg(test)]
mod gate_336_pkgs5;
// Workflow #373 NAV-3a follow-up (exec key 71): record stop prints its package's registry entry.
#[cfg(test)]
mod gate_373_nav3afu;
// Workflow #390 LAB-2 (exec key 72): forbidden and anchored pages, gestures, marks after stop.
#[cfg(test)]
mod gate_390_lab2;
// Test plan v2.2 E-5 G6 (exec key 35): record stop over a synthetic recording.
#[cfg(test)]
mod gate_e5_record_stop;
// #411 A10 (exec key 85): an undecodable Lab recording makes only the Lab part unavailable.
#[cfg(test)]
mod gate_rc2_a10;
// #411 Lab B-9 (exec key 108): a Lab recording marks a value test.
#[cfg(test)]
mod gate_lab_b9;
// #411 Lab BD (exec key 109): a Lab recording declares a brake drag and generates it.
#[cfg(test)]
mod gate_lab_bd;
// Workflow #406 T4 (exec key 112): offline runs draw as the kernel; a recording declares a path
// drag and names how the Lab pressed.
#[cfg(test)]
mod gate_406_t4;
// Workflow #449 REC-ARCH: a recording ended as material only; a recorded swipe is a step.
#[cfg(test)]
mod gate_449_rec_arch;
// #463 L1: `do --capture --record` records a whole step in one call.
#[cfg(test)]
mod gate_463_l1;
// #463 L2: a recorded step presses what it finds by its name (a located-text mark it follows).
#[cfg(test)]
mod gate_463_l2;
// #463 R1: a recorded step chooses in a list of fixed slots by the name read in each slot.
#[cfg(test)]
mod gate_463_r1;
// #463 R2: a stopped recording takes an ending from a frame, and record stop generates it.
#[cfg(test)]
mod gate_463_r2;
// #463 R4 and #464 P4: a stopped recording takes an optional step after one of its steps from a
// frame; record stop keeps its earlier output, refuses an installed root and answers the adoption.
#[cfg(test)]
mod gate_463_r4;
// #463 S1-1: a press is recorded by the command that makes it (`tap`, `swipe`, `long-tap
// --record`), with the screen before it and the screen after it.
#[cfg(test)]
mod gate_463_s11;
// #463 S1-2: a press is recorded with a fresh screen before it and a still screen after it.
#[cfg(test)]
mod gate_463_s12;
// #463 S1-7: a recording holds its instance; the session header keeps the hold's facts and
// `record status` shows them.
#[cfg(test)]
mod gate_463_s17;
// #463 S2-1: one read lists everything a recording still lacks before `record stop`.
#[cfg(test)]
mod gate_463_s21;
// #463 S2-1c: `record ready` lists every failure of each self-check, and whether its list is all.
#[cfg(test)]
mod gate_463_s21c;
// #463 stage two, train (S2-2, S2-7, S2-6, S2-1c): each step's `record review` lists exactly its
// items of the one readiness judgement `record ready` answers with.
#[cfg(test)]
mod gate_463_s2_train;
// #463 S2-13: `record frames` lists the stored frames just before and just after one step's press.
#[cfg(test)]
mod gate_463_s213;
// #463 S2-2: `record review` answers each step as the screen before, the press and the screen
// after, and writes its previews.
#[cfg(test)]
mod gate_463_s22;
// #463 S2-5: boxes drawn on a recording step, a region to recognize by and a region to press.
#[cfg(test)]
mod gate_463_s25_boxes;
// #463 S2-5: boxes on a draft stage one recorded, its captures before a press as base images.
#[cfg(test)]
mod gate_463_s25_draft;
// #463 S2-7: a page transition named by frames the recording stored; a press before the last that
// changed nothing dropped, and given back.
#[cfg(test)]
mod gate_463_s27;
// Workflow #392 DA-5ab fix round 1: a recording takes from a Lab operation only what it can take;
// the train of DA-5ab and DA-5cd (R4): every recording path judges the recording and the frame it
// took before it sends or writes anything.
#[cfg(test)]
mod gate_392_da5ab;
// #463 S2-6: a step marked first as holding a choice and refined later by its select; `record
// ready` lists one not refined (`select_unrefined`).
#[cfg(test)]
mod gate_463_s26;
// #463 S2-10: `record stop` writes the package's regression cases, each run first by the one
// runner of the case contract.
#[cfg(test)]
mod gate_463_s210;
// #463 S2-8': the swipes recorded before a press folded into one scrolling search, and unfolded.
#[cfg(test)]
mod gate_463_s28;
// #463 S2-9a: a scrolling choice that reads the list to its end, over the swipes folded into it.
#[cfg(test)]
mod gate_463_s29a;
// #463 S2-9b: a scrolling choice straight to the wanted item, its end mark optional.
#[cfg(test)]
mod gate_463_s29b;
// #463 KR-1: a recording's step may press the back key, declared after the fact and never sent
// while recording.
#[cfg(test)]
mod gate_463_kr1;
// The 0.12.70 train: where the key step of a recording meets the fold of swipes into a scrolling
// search and the scrolling choice.
#[cfg(test)]
mod gate_463_t70;
// #463 RM-1: a recording's self-checks warn of a template mark whose score lies nearer its
// threshold than a bound, on the side a self-check needs it on.
#[cfg(test)]
mod gate_463_rm1;
// #463 SC-1: a select step's page is checked for uniqueness on every frame of the recording.
#[cfg(test)]
mod gate_463_sc1;
// #463 SEL-2L-a: a select of fixed slots may read its names by detecting their blocks, and the
// refusal of a mark counts marks.
#[cfg(test)]
mod gate_463_sel2l_a;
mod lab_run;
mod lab_run_api;
mod ledger_port;
mod maa_task_graph;
mod ocr;
mod package_api;
mod package_build;
mod package_validate;
mod ports;
mod projection;
mod readonly;
mod readonly_api;
mod recording;
mod recording_api;
// #463 RC-1: a resource repository's regression case, its reader and its runner.
mod regression_case;
mod scheduling;
// #463 R2 fix 1: the scripted clock of a gate test that runs the kernel's interpreter on a device
// of its own; no gate file reads a clock.
#[cfg(test)]
mod scripted_clock;
mod state;
// Workflow #406 T4: the touch keys of the Lab's root configuration.
mod touch;

// #463 RC-1: and the words of a regression case's verdict.
pub use codes::{
    LabCode, RegressionNotRun, RegressionProblem, RegressionResult, RegressionVerdict,
};
pub use context::*;
pub use display_base::{
    RECORDING_BASE_SIZE, frames_in_base, is_base_size, package_base, record_display_not_base,
    scene_in_base, scene_png,
};
pub use drive::*;
pub use drive_api::*;
pub use env_api::*;
pub use env_detection::*;
pub use lab_run::{
    target_evaluations_stable_for_wait, validate_lab_package_bytes, validate_lab_package_path,
};
pub use lab_run_api::*;
pub use ledger_port::*;
pub use maa_task_graph::{
    MaaFact, MaaFactOrigin, MaaFactSource, MaaFactValue, MaaTaskFacts, MaaTaskGraph,
    MaaTaskGraphStats, compile_maa_task_graph,
};
// Workflow #428 LAB-OCR-DL: the provider of an offline evaluation of a saved scene whose switch
// raises the OCR inference deadline, and the deadline it raises it to.
pub use ocr::{LabOcr, OCR_DEADLINE_MAX_MS, with_ocr_deadline_max};
pub use package_api::*;
pub use ports::*;
pub use projection::*;
pub use readonly_api::*;
pub use recording::*;
pub use recording_api::*;
// #463 RC-1: the regression case's reader and runner.
pub use regression_case::*;
pub use scheduling::*;
pub use state::*;
pub use touch::LabTouch;

pub use actingcommand_artifact_store::{FrameStoreControl, MemorySample, MemorySampleSource};
pub use actingcommand_contract::{LabError, LabResult};
pub use actingcommand_execution_kernel::{
    DetectKind, OfflineDecision, OfflinePostAdmissionOcrStatus, OfflineSimulationError,
    OfflineSimulationResult, PreparedContainedTask, RecoveryAction, RecoveryExecError,
    RecoveryExecutionReport, RecoveryGraph, RecoveryNode, RecoveryResult, RecoveryRuntime,
    RecoverySignal, RecoveryStatus, execute_recovery_graph, simulate_contained_task,
    simulate_contained_task_at,
};
// #412 Lab B-3 S1: a declared operation as admission states it, and how its one step ended, for
// `actinglab do --operation` (S2).
pub use actingcommand_execution_kernel::{DeclaredOperation, DeclaredOperationOutcome};
// Workflow #407 LOAD-2: the loading set `actinglab package check` answers.
pub use actingcommand_execution_kernel::PackageLoadingSet;
// Workflow #308 SEL-L6a: the whole-package replay of `actinglab replay --frames`.
pub use actingcommand_execution_kernel::{
    OfflineReplayEnd, OfflineReplayError, OfflineReplayFacts, OfflineReplayResult,
    replay_contained_task,
};
// Workflow #308 SEL-L6b: the scan replay of `actinglab replay --frames --select --scan`.
pub use actingcommand_execution_kernel::{ScanDeclaration, ScanDryRun, replay_scan};
// #411 Lab RP: the run parameters a Lab evaluation reads, given with `--parameter`.
pub use actingcommand_execution_kernel::GivenParameterProblem;
// Workflow #463 V2: the variable values the scan replay reads, given with `--variable`; fix round
// 1: with the parameters they join, which the scan replay hands its scan.
pub use actingcommand_execution_kernel::{GivenVariableProblem, RunParameters};

pub use actingcommand_execution_kernel::{
    DriveNavigationGraph, parse_environment_catalog_value, validate_control_declaration,
};
// Workflow #390 LAB-2 item 7: the box an `offset` navigation click presses, for actinglab.
pub use actingcommand_execution_kernel::drive_offset_rect;
// #411 Lab L6: the online observation's value entries, for offline `observe`, and the online
// observation itself, which actinglab's parity test compares offline `observe` with.
pub use actingcommand_execution_kernel::{PreparedPageObservation, observation_fields};
// #411 Lab L7: the kernel's page check of an operation's guard, for actinglab's offline `do`.
pub use actingcommand_execution_kernel::page_anchor_matches;
// Workflow #406 T4: the offline runs' and the Lab's draws, the seed derivation, the touch keys,
// and the path drag's bounds for the Lab's readers.
pub use actingcommand_execution_kernel::{
    DriveDraw, OfflineDraw, TouchSampling, offline_action_seed, path_drag_bounds_hold,
    replay_contained_task_drawn, sampling_algorithm_name, simulate_contained_task_drawn,
};
// Workflow #357 DB-2: the dry runs' shared tables, bound from the root they are given.
pub use actingcommand_execution_kernel::{
    RunTableFailure, RunTables, replay_contained_task_with_tables, replay_scan_with_tables,
    simulate_contained_task_with_tables,
};
// Workflow #392 DA-5ab: a Runtime frame the Lab's client observes, on its package's base by the
// frame gate's own map and resample, and the map its input leaves under (`DisplayMap::send_input`,
// with the adapter's box and refusal), for actinglab's `--capture` forms and its frame files.
pub use actingcommand_display_adapter::{DisplayMap, InputSendError, Rect as DisplayRect};
pub use actingcommand_execution_kernel::{LabBaseScene, lab_display_map, lab_scene_on_base};

pub struct Lab<P: LabPorts> {
    ports: P,
    state: LabState,
}

impl<P: LabPorts> Lab<P> {
    pub fn new(ports: P, state: LabState) -> LabResult<Self> {
        Ok(Self { ports, state })
    }

    pub fn ports(&self) -> &P {
        &self.ports
    }

    pub fn state(&self) -> &LabState {
        &self.state
    }
}
