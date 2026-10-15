// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-4 (REC-M v1.2.1 §11, §12): `ac_record_mark` takes one request object and
// hands it to actinglab `record mark --request-json` as it is, so a click that follows a template
// mark (`click.follow`) and a template mark's `scales`, `mask` and `unique` reach the Lab
// unchanged. The actinglab child is never started: the test reads the command the tool would
// run.

use super::super::{Program, RECORD_MARK, command_line};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const ALIAS: &str = "neutral.instance";

#[test]
fn gate_ac_record_mark_hands_a_click_follow_request_to_actinglab_unchanged() {
    let program = Program {
        actinglab: PathBuf::from("neutral-install")
            .join("tools")
            .join("actinglab.exe"),
        root: PathBuf::from("neutral-install"),
        config: PathBuf::from("neutral-install").join("actingd.config.json"),
        installation: None,
        state_root: Ok(PathBuf::from("neutral-state")),
    };
    let request = json!({
        "schema_version": "actingcommand.lab-record-mark.v1",
        "frame": "frames/primary.png",
        "samples": ["frames/moved.png"],
        "add": [{
            "id": "ui/a",
            "family": "template",
            "region": {"x": 20, "y": 20, "width": 12, "height": 12},
            "search": {"x": 0, "y": 10, "width": 96, "height": 30},
            "scales": {"factors_milli": [1000, 750]},
            "mask": {"type": "range", "lower": 150, "upper": 255},
            "unique": {"margin_milli": 50}
        }],
        "click": {"region": {"x": 24, "y": 24, "width": 4, "height": 4}, "follow": "ui/a"}
    });
    let mut arguments = Map::new();
    arguments.insert("instance".to_owned(), json!(ALIAS));
    arguments.insert("request".to_owned(), request.clone());

    let line = match command_line(&RECORD_MARK, &arguments) {
        Ok(line) => line,
        Err(error) => panic!("ac_record_mark was refused: {}", error.to_value()),
    };
    let command = program.command(&line);
    let words = command
        .get_args()
        .map(|word| word.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(
        words[..5],
        ["--json", "--via", "mcp", "record", "mark"],
        "{words:?}"
    );
    let position = words
        .iter()
        .position(|word| word == "--request-json")
        .unwrap_or_else(|| panic!("no --request-json in {words:?}"));
    let passed: Value = serde_json::from_str(&words[position + 1])
        .unwrap_or_else(|error| panic!("the request is not JSON: {error}"));
    assert_eq!(passed, request);
    assert_eq!(passed.pointer("/click/follow"), Some(&json!("ui/a")));
}
