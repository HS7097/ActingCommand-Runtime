// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #406 T4 (apps gate key 79; register §1.138; brief-T4 Requirement 5): `ac_lab_do` takes
// an optional `seed`, an unsigned integer, and passes it to actinglab as `--seed`, as an operator
// types it; its answer carries the offline `sampling` (and the seed) as actinglab gives it, since
// a Lab answer within the budget is actinglab's data unchanged; and its description says so. The
// actinglab child is never started: the test reads the command line the tool would run and the
// answer a job ends with.

use super::super::super::protocol::Era;
use super::super::super::tools::{definition, find};
use super::super::{DO, command_line, do_input, finish, plain};
use serde_json::{Map, Value, json};

fn arguments(members: Value) -> Map<String, Value> {
    members.as_object().cloned().unwrap_or_default()
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn t4_ac_lab_do_passes_the_seed_and_answers_the_sampling() {
    // `seed` is `--seed <n>`, among the valued flags in the tool's flag order; any unsigned
    // 64-bit value passes.
    for (seed, text) in [(json!(23), "23"), (json!(u64::MAX), "18446744073709551615")] {
        let line = command_line(
            &DO,
            &arguments(json!({
                "target": "neutral_button",
                "scene": "scene.png",
                "zip": "neutral.zip",
                "dry_run": true,
                "seed": seed,
            })),
        )
        .unwrap_or_else(|error| panic!("ac_lab_do with seed was refused: {}", error.to_value()));
        assert_eq!(
            line,
            words(&[
                "do",
                "neutral_button",
                "--scene",
                "scene.png",
                "--zip",
                "neutral.zip",
                "--seed",
                text,
                "--dry-run",
            ])
        );
    }

    // The schema takes it as a non-negative integer; the description names it and the sampling.
    let schema = do_input();
    assert_eq!(schema["properties"]["seed"]["type"], "integer", "{schema}");
    assert_eq!(schema["properties"]["seed"]["minimum"], 0, "{schema}");
    let tool = find("ac_lab_do").expect("ac_lab_do is in the tool table");
    let described = definition(tool, Era::Modern);
    let description = described["description"].as_str().expect("description");
    for named in [
        "seed",
        "--seed",
        "actual_click.sampling",
        "actual_click.seed",
    ] {
        assert!(description.contains(named), "{named}: {description}");
    }

    // The answer: actinglab's data, the offline sampling and seed included, unchanged.
    let answer = json!({
        "state": "planned",
        "executed": false,
        "actual_click": {
            "kind": "target_rect_center",
            "rect": {"x": 20, "y": 10, "width": 40, "height": 20},
            "point": {"x": 43, "y": 25},
            "seed": 23,
            "sampling": {
                "algorithm": "center_hotspot_normal_v1",
                "action_seed": 1_u64,
                "source_regions": [{"x": 20, "y": 10, "width": 40, "height": 20}],
                "parameters": {"sigma_ratio_milli": 250, "max_redraws": 8}
            }
        }
    });
    let (relayed, warnings) = finish(
        &plain("lab_t4"),
        Ok(answer.clone()),
        Some("req_t4"),
        Vec::new(),
    );
    let Ok(relayed) = relayed else {
        panic!("the answer was not relayed");
    };
    assert_eq!(relayed, answer);
    assert!(warnings.is_empty(), "{warnings:?}");
}
