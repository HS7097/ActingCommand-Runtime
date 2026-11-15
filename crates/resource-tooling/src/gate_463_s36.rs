// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-6 (register §1.413; contracts/standard-pack-schedule.md, "Readiness"; the
// design rd-1115/pack-stage3-design.md §4.2 "S3-6"; coordinator work order
// rd-1115/brief-463-s36-ready-held-reason.md): the readiness says why a pack is held, and lists
// on each pack's row the warnings of `standard-pack check` that name it.
// - `s36_a_held_pack_s2_names_its_reason_and_reference` (fix round 2: the hold is a member of
//   the row): a pack held with a reason and a reference fails S2 `held` alone, and its row's
//   `held` is `{reason, reference}` as its `held` entry writes them; a pack held with a reason
//   only, longer than 256 bytes within the file's 1024, and one held with a reason in Chinese
//   carry that reason byte for byte and no `reference` member at all; S2's outcome carries no
//   `held_*` value; a pack not held answers no `held`;
// - `s36_a_variable_that_does_not_apply_is_listed_on_the_declaring_pack_s_row`: two packs that
//   declare one variable differently each list, on their own row and in the check's order, the
//   check's `standard_pack_package_unclassified` naming them by `package_id` and its
//   `standard_pack_root_invalid` `variable_conflict` naming their `control.json` by `path`; the
//   answer's `warnings` is the check's, unchanged; a pack no warning names answers no `warnings`;
// - `s36_a_root_without_warnings_answers_as_before`: a root the check warns nothing of answers no
//   `warnings` member on any pack's row, each row with the members it had before;
// - `s36_a_warning_that_names_no_pack_is_on_no_row` (fix round 2): the composer's hint of a rule
//   that declares no value, by its `task_id` and without a `package_id` or a `path`, stays in the
//   answer's `warnings`, the check's own, and is on no pack's row.
// Made-up packs only; no device, no Runtime, no clock read, no sleep.

use crate::codes::{ReadinessItem, ReadinessProblem, ResourceToolingCode};
use crate::{
    ItemState, PackReadiness, StandardPackCheckRequest, StandardPackReadiness,
    StandardPackReadyRequest, check_standard_pack_root, judge_standard_pack_readiness,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{MAX_SCHEDULE_TEXT_BYTES, StandardPackProblem};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The business pack, with one rule.
const CLAIM: &str = "neutral.test.claim";
/// The return-home package of the server.
const HOME: &str = "neutral.test.home";
/// A pack held with a reason only.
const HELD: &str = "neutral.test.held";
/// Fix round 2: a pack held with a reason in Chinese only.
const HELD_NON_ASCII: &str = "neutral.test.held_non_ascii";
/// Two packs that declare the variable `extra` differently.
const DECLARES_A: &str = "neutral.test.declares_a";
const DECLARES_B: &str = "neutral.test.declares_b";
/// The business pack's one rule.
const RULE: &str = "neutral.test.claim.rule";
/// The pool the rule produces into, with a valuation.
const VALUED: &str = "pool.valued";
/// The reason and the reference the business pack is held with.
const REASON: &str = "waits for its first trial on a real application";
const REFERENCE: &str = "fixture reference 1";
/// Fix round 2: a reason in Chinese ("waits until its first trial on a real application passes").
const REASON_IN_CHINESE: &str = "等它在真实应用上第一次试跑通过后再放行";

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("file");
}

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

/// The pack `package_id` in `directory`: a 64x24 page graph whose page `home` is told by a blue
/// first pixel and `page_b` by a red one, one trusted click from `home` to `page_b` (as
/// `gate_464_p3` writes it), its control declaring `variables` when given.
fn write_pack(directory: &Path, package_id: &str, variables: Option<&Value>) {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 64, "height": 24},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if let Some(variables) = variables {
        control["variables"] = variables.clone();
    }
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "S3-6 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    write_json(&directory.join("control.json"), &control);
    write_json(
        &directory.join("resources/operations/resources.json"),
        &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
    );
    write_json(
        &directory.join("resources/operations/task/task.json"),
        &task,
    );
}

/// The business pack's rule on an hourly clock that produces one unit into the valued pool (as
/// `gate_464_p3` writes it).
fn rule() -> Value {
    json!({
        "id": RULE,
        "package_id": CLAIM,
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3600000,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": RULE,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [{
            "pool_id": VALUED,
            "direction": "produce",
            "amount": 1,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }],
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
    })
}

/// The installed game root `<temp>/packages/neutral`: the business pack and its rule, the
/// server's return-home package (named for `return_home`), the packs of `extra` with the
/// variables each declares, and `schedule.json` holding the entries of `held`.
fn write_root(temp: &TempDir, held: &[Value], extra: &[(&'static str, Option<Value>)]) -> PathBuf {
    let root = temp.path().join("packages").join(GAME);
    let packs = [(CLAIM, None), (HOME, None)]
        .into_iter()
        .chain(extra.iter().cloned())
        .collect::<Vec<_>>();
    let mut listed = Vec::new();
    for (package_id, variables) in &packs {
        write_pack(&root.join(package_id), package_id, variables.as_ref());
        listed.push(json!({
            "package_id": package_id, "server": SERVER, "entry_task_id": "task",
            "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
        }));
    }
    write_json(
        &root.join("bundle.json"),
        &json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": listed,
            "maintenance": [{"package_id": HOME, "server": SERVER, "uses": ["return_home"]}]
        }),
    );
    write_json(
        &root.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "rules": [rule()],
            "pools": [{
                "id": VALUED,
                "capacity": 1000,
                "projection": {"kind": "none"},
                "observation": {"kind": "fact", "fact_key": "resource.valued"},
                "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
                "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
                "valuation": {
                    "name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000
                }
            }],
            "activity": [],
            "timeline": [],
            "held": held
        }),
    );
    root
}

/// A switch named `extra` with its `default` (as `gate_463_v1` writes it).
fn extra_switch(default: bool) -> Value {
    json!([{"name": "extra", "type": "switch", "title": {"en": "Switch"}, "default": default}])
}

/// The readiness of `root` as `actinglab standard-pack ready` asks it: the game root alone.
fn judge(root: &Path) -> StandardPackReadiness {
    judge_standard_pack_readiness(&StandardPackReadyRequest {
        root: root.to_path_buf(),
        declared_package: None,
        installation: None,
    })
    .unwrap_or_else(|error| panic!("the readiness: {}: {}", error.code, error.message))
}

/// The row of the pack `package_id`.
fn pack<'a>(readiness: &'a StandardPackReadiness, package_id: &str) -> &'a PackReadiness {
    readiness
        .packs
        .iter()
        .find(|pack| pack.package_id == package_id)
        .unwrap_or_else(|| panic!("{package_id} is answered: {readiness:?}"))
}

/// The members of the row of the pack `package_id`, as the answer serializes it.
fn members(readiness: &StandardPackReadiness, package_id: &str) -> BTreeSet<String> {
    let row = serde_json::to_value(pack(readiness, package_id)).expect("the row");
    row.as_object()
        .unwrap_or_else(|| panic!("the row is an object: {row}"))
        .keys()
        .cloned()
        .collect()
}

/// Holds that S2 of `row` fails `held`, alone among the root's items that fail, its outcome
/// carrying no `held_*` value (fix round 2: what it carried before the slice); its outcome.
fn held_outcome(row: &PackReadiness) -> Outcome {
    let failing = row
        .items
        .iter()
        .filter(|answer| answer.state == ItemState::Fails)
        .map(|answer| answer.item)
        .collect::<Vec<_>>();
    assert_eq!(failing, [ReadinessItem::Classified], "{row:?}");
    let outcome = row
        .item(ReadinessItem::Classified)
        .and_then(|answer| answer.outcome.clone())
        .unwrap_or_else(|| panic!("S2 carries an outcome: {row:?}"));
    assert!(
        outcome.is(ResourceToolingCode::PackageDispatchWaiting),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("readiness_problem"),
        Some(&json!(ReadinessProblem::Held.as_str())),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("package_id"),
        Some(&json!(row.package_id)),
        "{outcome:?}"
    );
    assert!(
        !outcome.values().keys().any(|key| key.starts_with("held_")),
        "{outcome:?}"
    );
    outcome
}

#[test]
fn s36_a_held_pack_s2_names_its_reason_and_reference() {
    let long = ["held until its trial passes, then placed by hand"; 12].join(" / ");
    assert!(
        (257..=MAX_SCHEDULE_TEXT_BYTES).contains(&long.len()),
        "{}",
        long.len()
    );
    let temp = TempDir::new().expect("temporary directory");
    let root = write_root(
        &temp,
        &[
            json!({"package_id": CLAIM, "reason": REASON, "reference": REFERENCE}),
            json!({"package_id": HELD, "reason": long}),
            json!({"package_id": HELD_NON_ASCII, "reason": REASON_IN_CHINESE}),
        ],
        &[(HELD, None), (HELD_NON_ASCII, None)],
    );

    let readiness = judge(&root);

    for (package_id, reason, reference) in [
        (CLAIM, REASON, Some(REFERENCE)),
        (HELD, long.as_str(), None),
        (HELD_NON_ASCII, REASON_IN_CHINESE, None),
    ] {
        let row = pack(&readiness, package_id);
        held_outcome(row);
        let hold = row
            .held
            .as_ref()
            .unwrap_or_else(|| panic!("{package_id} answers its hold: {row:?}"));
        assert_eq!(hold.reason.as_bytes(), reason.as_bytes(), "{row:?}");
        assert_eq!(hold.reference.as_deref(), reference, "{row:?}");
        let answered = serde_json::to_value(row).expect("the row");
        let written = match reference {
            Some(entry_reference) => json!({"reason": reason, "reference": entry_reference}),
            None => json!({"reason": reason}),
        };
        assert_eq!(answered["held"], written, "{answered}");
        let classified = answered["items"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == "S2"))
            .cloned()
            .unwrap_or_else(|| panic!("S2 is answered: {answered}"));
        let values = classified
            .pointer("/outcome/values")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("S2 carries values: {classified}"));
        assert!(
            !values.keys().any(|key| key.starts_with("held_")),
            "{classified}"
        );
    }
    assert!(pack(&readiness, HOME).held.is_none());
    assert!(!members(&readiness, HOME).contains("held"));
}

#[test]
fn s36_a_variable_that_does_not_apply_is_listed_on_the_declaring_pack_s_row() {
    let temp = TempDir::new().expect("temporary directory");
    let root = write_root(
        &temp,
        &[],
        &[
            (DECLARES_A, Some(extra_switch(false))),
            (DECLARES_B, Some(extra_switch(true))),
        ],
    );

    let readiness = judge(&root);

    let checked = check_standard_pack_root(StandardPackCheckRequest { root: root.clone() })
        .unwrap_or_else(|error| panic!("the root is checked: {}: {}", error.code, error.message));
    assert_eq!(readiness.warnings, checked.warnings);
    for package_id in [DECLARES_A, DECLARES_B] {
        let row = pack(&readiness, package_id);
        assert_eq!(row.warnings.len(), 2, "{row:?}");
        assert!(
            row.warnings[0].is(SharedCode::StandardPackPackageUnclassified),
            "{row:?}"
        );
        assert_eq!(
            row.warnings[0].value("package_id"),
            Some(&json!(package_id)),
            "{row:?}"
        );
        let control = std::path::absolute(root.as_path())
            .expect("absolute game root")
            .join(package_id)
            .join("control.json");
        assert!(
            row.warnings[1].is(SharedCode::StandardPackRootInvalid),
            "{row:?}"
        );
        assert_eq!(
            json!(row.warnings[1].values()),
            json!({
                "game": GAME,
                "path": control.to_string_lossy(),
                "standard_pack_problem": StandardPackProblem::VariableConflict.as_str(),
                "json_pointer": "/variables/0"
            }),
            "{row:?}"
        );
        let places = row
            .warnings
            .iter()
            .map(|warning| {
                readiness
                    .warnings
                    .iter()
                    .position(|listed| listed == warning)
                    .unwrap_or_else(|| panic!("the answer lists {warning:?}"))
            })
            .collect::<Vec<_>>();
        assert!(places[0] < places[1], "{places:?}");
        assert!(members(&readiness, package_id).contains("warnings"));
    }
    for package_id in [CLAIM, HOME] {
        assert!(pack(&readiness, package_id).warnings.is_empty());
        assert!(!members(&readiness, package_id).contains("warnings"));
    }
}

#[test]
fn s36_a_root_without_warnings_answers_as_before() {
    let temp = TempDir::new().expect("temporary directory");
    let root = write_root(&temp, &[], &[]);

    let readiness = judge(&root);

    assert!(readiness.warnings.is_empty(), "{:?}", readiness.warnings);
    assert_eq!(
        members(&readiness, CLAIM),
        names(&["package_id", "server", "readiness", "items"])
    );
    assert_eq!(
        members(&readiness, HOME),
        names(&["package_id", "server", "readiness", "roles", "items"])
    );
}

#[test]
fn s36_a_warning_that_names_no_pack_is_on_no_row() {
    let temp = TempDir::new().expect("temporary directory");
    let root = write_root(&temp, &[], &[]);
    // The business pack's rule produces nothing: it declares no value, and the composer's hint
    // names the rule by its `task_id` (as `gate_464_p3`'s undeclared value does).
    let schedule = root.join("schedule.json");
    let mut written: Value =
        serde_json::from_slice(&fs::read(&schedule).expect("schedule.json")).expect("JSON");
    written["rules"][0]["produces"] = json!([]);
    write_json(&schedule, &written);

    let readiness = judge(&root);

    let checked = check_standard_pack_root(StandardPackCheckRequest { root: root.clone() })
        .unwrap_or_else(|error| panic!("the root is checked: {}: {}", error.code, error.message));
    assert_eq!(readiness.warnings, checked.warnings);
    assert_eq!(readiness.warnings.len(), 1, "{:?}", readiness.warnings);
    let hint = &readiness.warnings[0];
    assert!(
        hint.is(SharedCode::StandardPackBenefitUndeclared),
        "{hint:?}"
    );
    assert_eq!(hint.value("task_id"), Some(&json!(RULE)), "{hint:?}");
    assert_eq!(hint.value("package_id"), None, "{hint:?}");
    assert_eq!(hint.value("path"), None, "{hint:?}");
    assert_eq!(readiness.packs.len(), 2, "{readiness:?}");
    for row in &readiness.packs {
        assert!(row.warnings.is_empty(), "{row:?}");
        assert!(!members(&readiness, &row.package_id).contains("warnings"));
    }
}

/// `list` as a set of owned names.
fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}
