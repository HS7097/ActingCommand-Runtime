// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-4 (register §1.411; contracts/standard-pack-authoring.md, "A held decision by
// two flags"; coordinator work order `rd-1115/brief-463-s34-adopt-held.md`, ruling 3): `actinglab
// standard-pack adopt` takes `--held-reason <text>` and `--held-reference <text>` in place of
// `--decide`, never beside it.
// - `s34_the_held_flags_beside_decide_or_the_reference_alone_are_refused_before_anything_is_read`:
//   `--decide` with `--held-reason`, in either order and with `--held-reference` too, is
//   `validation_failed`, exit 2, no data, naming `--held-reason` with `argument_problem`
//   `conflicting`; `--decide` with `--held-reference` alone, and `--held-reference` without
//   `--held-reason`, the same naming `--held-reference`. Each comes before anything is read: the
//   checkout, the package, the decision document and the state directory the line names do not
//   exist, so a call that read any of them would be refused `adopt_refused`, and none of them is
//   made.
// - Carried from the independent review of its pull request:
//   `s34_the_held_flags_on_the_command_line_reach_the_adoption_with_their_two_texts`: on a made-up
//   checkout (P1's: the game `neutral`, server `test`, one package and its rule), a line with a
//   valid `--held-reason` and `--held-reference` adopts the package as held, exit 0: the answer's
//   held entry and the one `schedule.json` holds carry exactly the two texts, and no rule.
// No Runtime, no device, no clock: an isolated environment, paths that do not exist and, for the
// adoption, a made-up checkout in it.

use super::env_lock;
use super::gate_434_lab5b4::Isolated;
use crate::run_cli;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

#[test]
fn s34_the_held_flags_beside_decide_or_the_reference_alone_are_refused_before_anything_is_read() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let repo = isolated.path("neutral-repo");
    let package = isolated.path("neutral-package");
    let decision = isolated.path("neutral-decision.json");
    let state = isolated.path("neutral-state");
    let adopt = [
        "--json",
        "standard-pack",
        "adopt",
        "--repo",
        repo.as_str(),
        "--package",
        package.as_str(),
        "--state-dir",
        state.as_str(),
    ];
    let decide = ["--decide", decision.as_str()];
    let reason = ["--held-reason", "Not run through on a real device yet."];
    let reference = ["--held-reference", "neutral reference 1"];
    for (flags, named) in [
        ([&decide[..], &reason[..]].concat(), "--held-reason"),
        ([&reason[..], &decide[..]].concat(), "--held-reason"),
        (
            [&decide[..], &reason[..], &reference[..]].concat(),
            "--held-reason",
        ),
        ([&decide[..], &reference[..]].concat(), "--held-reference"),
        (reference.to_vec(), "--held-reference"),
    ] {
        let line = [&adopt[..], &flags[..]].concat();
        let result = run_cli(line.clone(), true);
        let shown = result.envelope_json();
        assert_eq!(result.exit_code(), 2, "{line:?}: {shown}");
        assert!(result.envelope.data.is_none(), "{line:?}: {shown}");
        let error = result
            .envelope
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("{line:?} was not refused: {shown}"));
        assert_eq!(error.code, "validation_failed", "{line:?}: {shown}");
        let details = error.details.clone().unwrap_or(Value::Null);
        assert_eq!(details["argument"], named, "{line:?}: {shown}");
        assert_eq!(
            details["argument_problem"], "conflicting",
            "{line:?}: {shown}"
        );
    }
    for path in [&repo, &package, &decision, &state] {
        assert!(!Path::new(path).exists(), "{path} was made");
    }
    assert!(
        !isolated.runtime.exists(),
        "a refusal reached the Runtime's state root"
    );
}

/// The package the checkout holds with its rule, and the package adopted.
const EXISTING: &str = "neutral.existing";
const ADOPTED: &str = "neutral.daily";
/// The key of a normal end the made-up packages declare.
const OUTCOME_KEY: &str = "task_done";
const REASON: &str = "Not run through on a real device yet.";
const REFERENCE: &str = "neutral reference 1";

/// Writes `value` at `path` in the canonical form (two-space indentation, a final newline).
fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
    let mut bytes = serde_json::to_vec_pretty(value).expect("JSON");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("the file");
}

/// P1's made-up package `package_id` of the game `neutral`, server `test`, at `directory`: a route
/// from `home` (a blue first pixel) to `page_b` (a red one), one tap, the key [`OUTCOME_KEY`].
fn write_package(directory: &Path, package_id: &str) {
    write_json(
        &directory.join("control.json"),
        &json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": package_id,
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 9},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        }),
    );
    write_json(
        &directory.join("resources/operations/resources.json"),
        &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
    );
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    write_json(
        &directory.join("resources/operations/task/task.json"),
        &json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "goal": "S3-4 fixture",
            "coordinate_space": {"width": 16, "height": 9},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
            "page_rules": {
                "home": {"required": ["home_colour"]},
                "page_b": {"required": ["page_b_colour"]}
            },
            "entry_page": "home",
            "target_page": "page_b",
            "scheduling_outcome": {"mappings": [{
                "outcome_key": OUTCOME_KEY,
                "effect": "no_designated_effect",
                "terminal_pages": ["page_b"]
            }]},
            "operations": [{
                "id": "go", "from": "home", "to": "page_b",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true, "retryable": false,
                "post_delay_ms": 200
            }]
        }),
    );
}

/// P1's made-up checkout at `repo`: `applications.json`, `maintenance.json` (`[]`), the package
/// [`EXISTING`] and `schedule.json` with its hourly business rule and a valued pool.
fn write_checkout(repo: &Path) {
    write_json(
        &repo.join("applications.json"),
        &json!({
            "schema_version": "actingcommand.applications.v1",
            "game": "neutral",
            "servers": {"test": {"application_id": "com.example.neutral", "label": "Test"}}
        }),
    );
    write_json(&repo.join("maintenance.json"), &json!([]));
    write_package(&repo.join("packs").join(EXISTING), EXISTING);
    let every_hour = json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": 3_600_000,
            "anchor_ms": 0
        }
    });
    write_json(
        &repo.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "rules": [{
                "id": EXISTING,
                "package_id": EXISTING,
                "priority": 100,
                "trigger": every_hour,
                "feedback_stop": {
                    "kind": "outcome",
                    "task_id": EXISTING,
                    "outcome_key": OUTCOME_KEY,
                    "comparison": "eq",
                    "value": {"type": "boolean", "value": true}
                },
                "consumes": [],
                "produces": [],
                "on_failure": {
                    "action": "continue",
                    "retry_limit": 2,
                    "retry_backoff_ms": 30000,
                    "escalation_threshold": 3
                },
                "sensitive": false,
                "next_run_clamp_ms": 86400000,
                "yield_points": [],
                "expected_duration_ms": 60000,
                "cooldown_ms": 1000,
                "load_profile": {"kind": "light"},
                "strategic_weight_milli": 1000
            }],
            "pools": [{
                "id": "pool.neutral.points",
                "capacity": 1000,
                "projection": {"kind": "none"},
                "observation": {"kind": "fact", "fact_key": "resource.neutral.points"},
                "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
                "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
                "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
            }]
        }),
    );
}

#[test]
fn s34_the_held_flags_on_the_command_line_reach_the_adoption_with_their_two_texts() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let repo = isolated.path("neutral-repo");
    let package = isolated.path("neutral-package");
    let state = isolated.path("neutral-state");
    write_checkout(Path::new(&repo));
    write_package(Path::new(&package), ADOPTED);
    let line = [
        "--json",
        "standard-pack",
        "adopt",
        "--repo",
        repo.as_str(),
        "--package",
        package.as_str(),
        "--held-reason",
        REASON,
        "--held-reference",
        REFERENCE,
        "--state-dir",
        state.as_str(),
    ];

    let result = run_cli(line, true);

    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 0, "{shown}");
    let data = result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("the adoption answers: {shown}"));
    let entry = json!({"package_id": ADOPTED, "reason": REASON, "reference": REFERENCE});
    assert_eq!(data["status"], "adopted", "{shown}");
    assert_eq!(data["class"], "held", "{shown}");
    assert_eq!(data["held"], entry, "{shown}");
    assert!(data.get("rule").is_none(), "{shown}");
    let schedule = serde_json::from_slice::<Value>(
        &fs::read(Path::new(&repo).join("schedule.json")).expect("schedule.json"),
    )
    .expect("schedule.json reads");
    assert_eq!(schedule["held"], json!([entry]), "{shown}");
    assert_eq!(
        schedule["rules"].as_array().map(Vec::len),
        Some(1),
        "no rule is added: {shown}"
    );
    assert!(
        !isolated.runtime.exists(),
        "the adoption reached the Runtime's state root"
    );
}
