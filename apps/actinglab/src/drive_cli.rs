// SPDX-License-Identifier: AGPL-3.0-only

use super::lab2_cli::refuse_repeated_options;
use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, finish_semantic_result_with_ledger,
    parse_optional_duration_ms, read_user_config, reject_legacy_session_routing,
    resolve_instance_id, semantic_ledger_context, target_argument,
};
use crate::flag_args::reads::{FlagReads, ReadPart};
use actingcommand_lab::{NavigateRequest, TapTargetRequest};
use serde::Serialize;
use serde_json::Value;

/// #411 Lab B-2 R7 (coordinator decision Q9 of 2026-10-19): `tap-target` and `navigate` read one
/// frame and take every option once, `--scene` included; an option given twice is refused as
/// `repeated` before anything is read, never last-wins.
const ONE_FRAME_REPEATS: &[&str] = &[];

/// Workflow #434 LAB-5B3: the flags `tap-target` reads: the legacy routing it refuses, its target
/// (also its first positional, `target_argument`), the seed, the dry run, and the readers' frame
/// input and package; `--capture` it reads and then refuses with the readers'. Workflow #434
/// LAB-5B4: its command line is parsed by it (`FlagArgs::parse_reading`).
pub(super) const TAP_TARGET_READS: FlagReads = FlagReads {
    row: "tap-target",
    parts: &[
        &super::commands::device_commands::LEGACY_ROUTING_READS,
        &ReadPart {
            options: &["--target"],
            switches: &["--dry-run", "--allow-destructive"],
            refused: &[],
        },
        &super::lab_draw::SEED_READS,
        &super::readonly_cli::READONLY_INPUT_READS,
        &super::contained_resources::PACKAGE_LOCATOR_READS,
    ],
    positionals: &["<target>"],
};

/// Workflow #434 LAB-5B3: the flags `navigate` reads; the step timing only when it presses (no dry
/// run). Workflow #434 LAB-5B4: its command line is parsed by it; no positional.
pub(super) const NAVIGATE_READS: FlagReads = FlagReads {
    row: "navigate",
    parts: &[
        &super::commands::device_commands::LEGACY_ROUTING_READS,
        &ReadPart {
            options: &["--to", "--step-timeout-ms", "--poll-ms"],
            switches: &["--dry-run", "--allow-destructive"],
            refused: &[],
        },
        &super::lab_draw::SEED_READS,
        &super::readonly_cli::READONLY_INPUT_READS,
        &super::contained_resources::PACKAGE_LOCATOR_READS,
    ],
    positionals: &[],
};

pub(super) fn run_tap_target(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_reading(args, &TAP_TARGET_READS)?;
    reject_legacy_session_routing(&flags)?;
    refuse_repeated_options(&flags, "tap-target", ONE_FRAME_REPEATS)?;
    let target = target_argument(&flags, "tap-target")?;
    // Workflow #406 T4: `--seed` draws the point pressed (or planned) in the target's click box.
    let seed = super::lab_draw::seed_flag(&flags)?;
    let mut ledger = semantic_ledger_context("tap-target", global, args);
    let result = (|| -> CliOutcome<Value> {
        let dry_run = global.dry_run || flags.bool("--dry-run");
        let config = read_user_config()?;
        let capture_requested = flags.bool("--capture");
        let instance_alias = capture_requested
            .then(|| resolve_instance_id(global, &config))
            .transpose()?;
        let input =
            super::readonly_cli::recognition_input_with_config(global, &flags, false, &config)?;
        let mut lab =
            super::env_detection::build_drive_lab(config, instance_alias.as_deref(), !dry_run)?;
        let request = TapTargetRequest {
            input,
            target,
            allow_destructive: flags.bool("--allow-destructive"),
            dry_run,
            capture_requested,
            draw: super::lab_draw::press_draw(seed)?,
        };
        serialize_response(lab.tap_target(request, &mut ledger)?)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

pub(super) fn run_navigate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_reading(args, &NAVIGATE_READS)?;
    reject_legacy_session_routing(&flags)?;
    refuse_repeated_options(&flags, "navigate", ONE_FRAME_REPEATS)?;
    let to = flags.required("--to")?;
    // Workflow #406 T4: `--seed` draws the point of each box the route presses; a planned route
    // presses nothing, so it is refused there, never ignored.
    let seed = super::lab_draw::seed_flag(&flags)?;
    if seed.is_some() && (global.dry_run || flags.bool("--dry-run")) {
        return Err(super::lab_draw::seed_refusal(
            "conflicting",
            "navigate reads --seed only for the route it presses (--capture without --dry-run)",
        ));
    }
    let mut ledger = semantic_ledger_context("navigate", global, args);
    let result = (|| -> CliOutcome<Value> {
        let dry_run = global.dry_run || flags.bool("--dry-run");
        let config = read_user_config()?;
        let capture_requested = flags.bool("--capture");
        let instance_alias = capture_requested
            .then(|| resolve_instance_id(global, &config))
            .transpose()?;
        let input =
            super::readonly_cli::recognition_input_with_config(global, &flags, true, &config)?;
        let mut lab =
            super::env_detection::build_drive_lab(config, instance_alias.as_deref(), !dry_run)?;
        let request = NavigateRequest {
            input,
            to,
            allow_destructive: flags.bool("--allow-destructive"),
            dry_run,
            capture_requested,
            step_timeout: (!dry_run)
                .then(|| parse_optional_duration_ms(&flags, "--step-timeout-ms", 5_000)),
            poll: (!dry_run).then(|| parse_optional_duration_ms(&flags, "--poll-ms", 500)),
            draw: super::lab_draw::press_draw(seed)?,
        };
        serialize_response(lab.navigate(request, &mut ledger)?)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
