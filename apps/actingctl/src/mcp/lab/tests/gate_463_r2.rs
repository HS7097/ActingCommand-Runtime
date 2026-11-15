// SPDX-License-Identifier: AGPL-3.0-only

// #463 R2 (contracts/lab-recording.md, "Endings"): `ac_record_mark` takes the request form of an
// ending, and of an ending to give up, and hands it to actinglab `record mark --request-json` as it
// is, so the MCP tool and the command line (`record mark --ending`, whose flags actinglab turns into
// the same request) reach the Lab's one `record_mark` with one request and answer the same. The
// actinglab child is never started: the test reads the command the tool would run.

use super::super::{Program, RECORD_MARK, command_line};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const ALIAS: &str = "neutral.instance";

#[test]
fn r2_ac_record_mark_hands_an_ending_request_to_actinglab_unchanged() {
    let program = Program {
        actinglab: PathBuf::from("neutral-install")
            .join("tools")
            .join("actinglab.exe"),
        root: PathBuf::from("neutral-install"),
        config: PathBuf::from("neutral-install").join("actingd.config.json"),
        installation: None,
        state_root: Ok(PathBuf::from("neutral-state")),
    };
    for request in [
        json!({
            "schema_version": "actingcommand.lab-record-mark.v1",
            "ending": {
                "frame": "frames/short.png",
                "samples": ["frames/short_2.png"],
                "page": "short",
                "outcome_key": "neutral_task_short",
                "add": [{"id": "ending/short", "family": "color",
                         "region": {"x": 60, "y": 50, "width": 4, "height": 4}}],
                "reuse": ["state/list"]
            }
        }),
        json!({
            "schema_version": "actingcommand.lab-record-mark.v1",
            "remove_ending": "neutral_task_short"
        }),
    ] {
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
    }
}
