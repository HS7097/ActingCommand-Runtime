// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #410 item 1 (board #410, approved by Alice 2026-10-15; the 0.12 end): every
// ac_lab_observe and ac_lab_do answer states the frame's coordinate system, an optional JPEG
// preview is given by its path and scale, an action attaches its result by default, and no image
// is inlined. MCP keeps A5's shapes and budgets: the answer is actinglab's data unchanged.
//
// - Parity (MCP-0 command table v1.1 §2.3, apps key 41): `preview` maps one to one to actinglab's
//   `--preview` on observe and do, after `--json --via mcp` and the command word, as an operator
//   types it; the actinglab child is never started, the test reads the command the tool would run.
// - The budget (apps key 66): an answer over the output budget is still exported whole (the one
//   overflow rule, review F2), and it still states the frame inline: `frame_coordinates`,
//   `frame_path`, `preview` and `after_attachment` stay in the exported answer's place when they
//   fit, verbatim, while the answer stays within the budget.

use super::super::{DO, OBSERVE, Program, command_end, command_line, fits_as_job};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::PathBuf;

const ALIAS: &str = "neutral.instance";

fn program() -> Program {
    Program {
        actinglab: PathBuf::from("neutral-install")
            .join("tools")
            .join("actinglab.exe"),
        root: PathBuf::from("neutral-install"),
        config: PathBuf::from("neutral-install").join("actingd.config.json"),
        installation: None,
        state_root: Ok(PathBuf::from("neutral-state")),
    }
}

fn arguments(members: Value) -> Map<String, Value> {
    members.as_object().cloned().unwrap_or_default()
}

// The words the tool's actinglab child would be started with.
fn words(line: &[String]) -> Vec<String> {
    program()
        .command(line)
        .get_args()
        .map(|word| word.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn gate_labobs_ac_lab_observe_and_ac_lab_do_pass_preview_to_actinglab_as_the_cli_takes_it() {
    let observe = command_line(
        &OBSERVE,
        &arguments(json!({
            "instance": ALIAS,
            "capture": true,
            "zip": "neutral-package.zip",
            "preview": true,
        })),
    )
    .unwrap_or_else(|error| panic!("ac_lab_observe was refused: {}", error.to_value()));
    assert_eq!(
        words(&observe),
        [
            "--json",
            "--via",
            "mcp",
            "observe",
            "--instance",
            ALIAS,
            "--zip",
            "neutral-package.zip",
            "--capture",
            "--preview",
        ],
        "ac_lab_observe with preview"
    );
    let act = command_line(
        &DO,
        &arguments(json!({
            "instance": ALIAS,
            "capture": true,
            "zip": "neutral-package.zip",
            "tap": "640,360",
            "preview": true,
        })),
    )
    .unwrap_or_else(|error| panic!("ac_lab_do was refused: {}", error.to_value()));
    assert_eq!(
        words(&act),
        [
            "--json",
            "--via",
            "mcp",
            "do",
            "--instance",
            ALIAS,
            "--zip",
            "neutral-package.zip",
            "--tap",
            "640,360",
            "--capture",
            "--preview",
        ],
        "ac_lab_do with preview"
    );
    // Without it, no preview is asked for.
    let plain = command_line(
        &OBSERVE,
        &arguments(json!({"instance": ALIAS, "capture": true, "zip": "neutral-package.zip"})),
    )
    .unwrap_or_else(|error| panic!("ac_lab_observe was refused: {}", error.to_value()));
    assert!(
        !words(&plain).iter().any(|word| word == "--preview"),
        "{plain:?}"
    );
}

// An ac_lab_do answer as actinglab gives it after an action on an unknown page, with rows far
// over the 11 KiB payload budget beside it.
fn oversized_do_answer() -> Value {
    let rows = (0..800)
        .map(|index| format!("neutral-ledger-row-{index:04}"))
        .collect::<Vec<_>>();
    let coordinates = json!({
        "unit": "device_pixel",
        "width": 1920,
        "height": 1080,
        "orientation": "landscape",
        "origin": "top_left",
        "applies_to": ["tap", "tap_rect", "swipe"],
    });
    json!({
        "req_id": "request_labobs_gate",
        "executed": true,
        "frame_coordinates": coordinates,
        "after_attachment": {
            "kind": "frame",
            "frame_coordinates": coordinates,
            "frame_path": "C:/Users/neutral/AppData/Local/ActingCommand/actinglab/frames/0a1b.png",
            "preview": {
                "path": "C:/Users/neutral/AppData/Local/ActingCommand/actinglab/frames/0a1b.preview.jpg",
                "width": 1024,
                "height": 576,
                "scale": 1.875,
            },
        },
        "ledger": {"events": rows},
    })
}

#[test]
fn gate_labobs_an_exported_answer_still_states_its_frame_inline_within_the_budget() {
    for (command, tool) in [(&DO, "ac_lab_do"), (&OBSERVE, "ac_lab_observe")] {
        let data = oversized_do_answer();
        let handle = "lab_job_labobs_gate";
        let (result, warnings) = command_end(
            command,
            handle,
            Ok(data.clone()),
            Some("request_labobs_gate"),
            Vec::new(),
        );
        let answer =
            result.unwrap_or_else(|error| panic!("{tool} answered an error: {}", error.to_value()));
        assert_eq!(answer["overflowed"], json!(true), "{tool}: {answer}");
        let path = answer["export"]["path"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool}: no export: {answer}"));
        let exported: Value = serde_json::from_slice(
            &fs::read(path).unwrap_or_else(|error| panic!("{tool}: the export: {error}")),
        )
        .unwrap_or_else(|error| panic!("{tool}: the export is not JSON: {error}"));
        assert_eq!(exported, data, "{tool}: the export is not the whole answer");
        for part in ["frame_coordinates", "after_attachment"] {
            assert_eq!(answer[part], data[part], "{tool}: {part} inline: {answer}");
        }
        assert!(
            answer.get("ledger").is_none(),
            "{tool}: the rows stay in the export: {answer}"
        );
        assert!(
            fits_as_job(handle, &answer, &warnings),
            "{tool}: the answer is over the budget"
        );
    }
}
