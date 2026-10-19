// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #406 T4 (plan Q20, Q14; `contracts/resource-declarations.md`, "The Lab's draw"): the
//! seed the Lab is given (`--seed`) and the touch keys of the root configuration it reads, from
//! which it draws the points it presses and an offline run draws its steps' points.
//!
//! - An offline run (`package dry-run`, `replay`) always draws, from the seed or 0 without one.
//! - A Lab press (`do`, the routes of `ensure`, `navigate` and `session recover`, a recorded
//!   click) draws only when it is given a seed; without one it presses each box's centre, as
//!   before.
//! - The touch keys are those of the root configuration (`LabTouch`); a process outside every
//!   root has none and draws each box uniformly. A configuration that cannot be read refuses the
//!   draw loudly.

use super::{CliError, CliOutcome, FlagArgs};
use actingcommand_lab::{DriveDraw, LabTouch, OfflineDraw};
use serde_json::json;

/// The flag that gives the Lab its seed.
pub(super) const SEED_FLAG: &str = "--seed";

/// Tests only: a Runtime configuration file whose touch keys the Lab reads instead of its root's.
#[cfg(test)]
pub(super) const TEST_TOUCH_CONFIG_ENV: &str = "ACTINGCOMMAND_TEST_TOUCH_CONFIG";

/// The seed `--seed` gives, `None` without the flag. A flag without a value is `value_missing`,
/// one that is not an unsigned 64-bit integer `wrong_type`, each `validation_failed` naming
/// `--seed`, before anything is read.
pub(super) fn seed_flag(flags: &FlagArgs) -> CliOutcome<Option<u64>> {
    let Some(value) = flags.optional(SEED_FLAG) else {
        return Ok(None);
    };
    if value == "true" || value.trim().is_empty() {
        return Err(seed_refusal(
            "value_missing",
            "--seed takes an unsigned 64-bit integer",
        ));
    }
    value.parse::<u64>().map(Some).map_err(|_| {
        seed_refusal(
            "wrong_type",
            format!("--seed {value} is not an unsigned 64-bit integer"),
        )
    })
}

/// A refused `--seed`: `validation_failed` (exit 2) with the details of the Lab's other option
/// refusals (`argument`, `argument_problem`, `arguments`).
pub(super) fn seed_refusal(problem: &str, message: impl Into<String>) -> CliError {
    CliError::usage(message).with_details(json!({
        "argument": SEED_FLAG,
        "argument_problem": problem,
        "arguments": [{"argument": SEED_FLAG, "argument_problem": problem}],
    }))
}

/// The touch keys of this process's root configuration.
pub(super) fn lab_touch() -> LabTouch {
    #[cfg(test)]
    if let Some(path) = std::env::var_os(TEST_TOUCH_CONFIG_ENV) {
        return match std::fs::read(&path) {
            Ok(configuration) => LabTouch::from_runtime_configuration(&configuration),
            Err(error) => panic!("test touch configuration {path:?}: {error}"),
        };
    }
    LabTouch::from_process_installation()
}

/// How a Lab press draws its points: from `seed` with the root configuration's touch keys; `None`
/// without a seed, so each box's centre is pressed.
pub(super) fn press_draw(seed: Option<u64>) -> CliOutcome<Option<DriveDraw>> {
    let Some(seed) = seed else {
        return Ok(None);
    };
    Ok(Some(DriveDraw {
        seed,
        touch: lab_touch().sampling()?,
    }))
}

/// How an offline run draws its steps' points: from `seed`, or 0 without one, with the root
/// configuration's touch keys.
pub(super) fn offline_draw(seed: Option<u64>) -> CliOutcome<OfflineDraw> {
    let _ = seed;
    Ok(OfflineDraw {
        seed: 0,
        touch: lab_touch().sampling()?,
    })
}
