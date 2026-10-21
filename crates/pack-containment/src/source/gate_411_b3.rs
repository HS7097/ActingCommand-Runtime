// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B3 logic test (B-train plan §2.3, §5.3 row T4): the declaration gate admits 1 to
// 7 `alternatives` on a color probe and on a template's `color_check`, and refuses 8 colors, a
// repeated color (another alternative or `expected`) and a component above 255 with
// `resource_declaration_invalid` at the member.

use super::{Bundle, declaration_file_requests};
use serde_json::{Value, json};
use std::path::PathBuf;

/// A task schema `0.9` task whose one anchor's `color_check` declares `check` and whose one
/// color probe declares `probe` as its `alternatives`; both expect `[200, 40, 40]`.
fn task(check: Value, probe: Value) -> Bundle {
    Bundle {
        task_id: "colors".to_owned(),
        dir: PathBuf::from("tasks/colors"),
        data: json!({
            "schema_version": "0.9", "task_id": "colors", "game": "game-a",
            "server_scope": ["server-a"],
            "coordinate_space": {"width": 64, "height": 40},
            "anchors": [{
                "id": "home", "template": "assets/marker.png",
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 32, "height": 20}},
                "color_check": {"region": {"x": 10, "y": 1, "width": 4, "height": 4},
                                "expected": [200, 40, 40], "alternatives": check}
            }],
            "color_probes": [{
                "id": "state/square",
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 4, "height": 4}},
                "expected": [200, 40, 40], "alternatives": probe
            }],
            "operations": []
        }),
    }
}

#[test]
fn b3_t4_eight_repeated_or_out_of_range_alternatives_are_refused_at_the_member() {
    let seven = json!([
        [1, 2, 3],
        [4, 5, 6],
        [7, 8, 9],
        [10, 11, 12],
        [13, 14, 15],
        [16, 17, 18],
        [19, 20, 21]
    ]);
    declaration_file_requests(&[task(seven.clone(), seven.clone())])
        .expect("seven alternatives on a check and on a probe are admitted");

    let mut eight = seven.clone();
    eight
        .as_array_mut()
        .expect("an array")
        .push(json!([22, 23, 24]));
    for (case, invalid) in [
        ("eight colors", eight),
        ("an alternative listed twice", json!([[1, 2, 3], [1, 2, 3]])),
        ("a copy of expected", json!([[4, 5, 6], [200, 40, 40]])),
        ("a component above 255", json!([[1, 2, 256]])),
    ] {
        for (check, probe, member) in [
            (
                invalid.clone(),
                seven.clone(),
                "/anchors/0/color_check/alternatives",
            ),
            (
                seven.clone(),
                invalid.clone(),
                "/color_probes/0/alternatives",
            ),
        ] {
            let Err(error) = declaration_file_requests(&[task(check, probe)]) else {
                panic!("{case} at {member} is admitted");
            };
            assert_eq!(error.code, "resource_declaration_invalid", "{case}");
            let issue = error.details.expect("a declaration issue");
            assert_eq!(issue["field_path"], member, "{case}");
            assert_eq!(issue["reason"], "invalid_value", "{case}");
        }
    }
}
