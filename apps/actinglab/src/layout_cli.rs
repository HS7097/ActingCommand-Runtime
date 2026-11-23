// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-L0 (`contracts/display-adapter.md` section 14; register §1.521): `actinglab
//! layout probe`, the operation table's row `layout probe` (MCP `ac_layout_probe`), offline. Its
//! command line is parsed by its read list ([`LAYOUT_PROBE_READS`]); the package is read as the
//! offline `recognize` reads it (`contained_resources::load`) and the scene by the Lab
//! (`actingcommand_lab::Lab::layout_probe`), which answers where each template mark of the chosen
//! pages went on the saved frame. Nothing is captured, pressed, sent or written.

use super::lab2_cli::refuse_repeated_options;
use super::{CliError, CliOutcome, FlagArgs};
use crate::flag_args::reads::{FlagReads, ReadPart};
use actingcommand_lab::{
    LAYOUT_PROBE_DEFAULT_TOLERANCE_PX, LayoutProbeRequest, ReadonlyRecognitionInput,
};
use serde_json::{Value, json};
use std::time::Duration;

/// The flags `layout probe` reads: the saved frame, the page and the tolerance, and the package by
/// its path; it takes no positional word.
pub(super) const LAYOUT_PROBE_READS: FlagReads = FlagReads {
    row: "layout probe",
    parts: &[
        &ReadPart {
            options: &["--scene", "--page", "--tolerance-px"],
            switches: &[],
            refused: &[],
        },
        &super::contained_resources::PACKAGE_LOCATOR_READS,
    ],
    positionals: &[],
};

/// actinglab's `layout <subcommand>`: `probe`.
pub(super) fn run_layout(subcommand: &str, args: &[String]) -> CliOutcome<Value> {
    match subcommand {
        "probe" => run_probe(args),
        other => Err(CliError::usage(format!(
            "unknown actinglab command: layout {other}"
        ))),
    }
}

/// `layout probe --scene <png> (--package <path> | --zip <file>) [--page <id>] [--tolerance-px
/// <n>]`: every option given once, the scene required, the tolerance a whole number of pixels
/// (`LAYOUT_PROBE_DEFAULT_TOLERANCE_PX` without it).
fn run_probe(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_reading(args, &LAYOUT_PROBE_READS)?;
    refuse_repeated_options(&flags, "layout probe", &[])?;
    let scene_path = flags.required_path("--scene")?;
    let page = match flags.optional("--page") {
        Some(value) if value == "true" => {
            return Err(CliError::usage("--page names a page of the package"));
        }
        given => given,
    };
    let tolerance_px = match flags.optional("--tolerance-px") {
        None => LAYOUT_PROBE_DEFAULT_TOLERANCE_PX,
        Some(text) => text.parse::<u32>().map_err(|_| {
            CliError::usage(format!(
                "--tolerance-px takes a whole number of pixels from 0 to {}, got {text}",
                u32::MAX
            ))
            .with_details(json!({
                "argument": "--tolerance-px",
                "argument_problem": "wrong_type",
            }))
        })?,
    };
    let resources = super::contained_resources::load(&flags, "layout-probe")?;
    let mut lab = super::env_detection::build_readonly_lab()?;
    let response = lab.layout_probe(LayoutProbeRequest {
        input: ReadonlyRecognitionInput {
            resources,
            scene: None,
            scene_path: Some(scene_path),
            sample_scenes: Vec::new(),
            capture_config: None,
            require_fresh: false,
            fresh_delay: Duration::ZERO,
        },
        page,
        tolerance_px,
    })?;
    serde_json::to_value(response).map_err(|error| {
        CliError::device(format!(
            "failed to serialize the layout probe's answer: {error}"
        ))
    })
}
