// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R1 (register §1.350; contracts/standard-pack-goals.md; coordinator work order
// `rd-1114/brief-455-gr1.md`): the Tools read a game root's goal declaration `goals.json` after
// the composition they already run.
// - `standard-pack check` reports each of the thirteen refusals at its pointer, one fixture each,
//   as a `standard_pack_root_invalid` warning naming `goals.json`, and the root is checked: a
//   refused file never fails it;
// - a root with a valid file answers as the same root without it in every member but
//   `root_files`, which lists `goals.json`;
// - a refused file's problems come in file order, and the answer is otherwise the answer without
//   the file;
// - an entry that governs a package whose control declaration has an irreversible capability is
//   refused (`goal_governs_irreversible`) by the check and by `package bundle --goals`, while a
//   package whose capability is not irreversible may be governed;
// - `package bundle --goals` lays an accepted file beside `bundle.json` byte for byte, the rest of
//   its output as without the flag, judging no table (the bundle holds none), and refuses a
//   refused file as a refused root file, with `--out` not written.
// A made-up game `neutral`, server `test`, 16x9; every package, pool, rule, table and text is a
// placeholder.

use crate::{
    PackageBundleRequest, StandardPackCheckRequest, StandardPackCheckResponse,
    StandardPackRootFiles, check_standard_pack_root, package_bundle,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{LabError, LabResult};
use actingcommand_policy::BENEFIT_FIXED_VALUE_SUBSYSTEM;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package most entries govern; its rule writes a fixed value.
const COLLECT: &str = "neutral.collect";
/// A package whose rule writes no fixed value.
const SPEND: &str = "neutral.spend";
/// A package whose control declaration provides an irreversible capability.
const DISCARD: &str = "neutral.discard";
/// A package whose control declaration provides a capability that is not irreversible.
const KEEP: &str = "neutral.keep";
/// A pool whose valuation has a step and a gap weight.
const VALUED: &str = "pool.neutral.points";
/// A pool whose valuation has no gap weight.
const UNVALUED: &str = "pool.neutral.stock";
/// The table an `ordered` entry takes its values from.
const TABLE: &str = "neutral_members";
const ROOT_INVALID: &str = "standard_pack_root_invalid";

/// Writes the package `package_id` as a package directory: its control (with `provides` when
/// given), its resource table and its one task, from page `home` (a blue first pixel) to page
/// `page_b` (a red one).
fn write_pack(directory: &Path, package_id: &str, provides: Option<Value>) {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if let Some(provides) = provides {
        control["provides"] = provides;
    }
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "G-R1 fixture",
        "coordinate_space": {"width": 16, "height": 9},
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
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("pack file parent")).expect("pack directory");
        fs::write(path, serde_json::to_vec_pretty(&value).expect("pack JSON")).expect("pack file");
    }
}

/// The four packages of the root, each with what its control provides.
fn packs() -> [(&'static str, Option<Value>); 4] {
    [
        (COLLECT, None),
        (SPEND, None),
        (
            DISCARD,
            Some(
                json!([{"capability": "cap_discard", "irreversible": true, "parameters": [
                {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}]}]),
            ),
        ),
        (KEEP, Some(json!([{"capability": "cap_keep"}]))),
    ]
}

/// A `schedule.json` rule running the package of its own id on an hourly clock.
fn rule(id: &str) -> Value {
    json!({
        "id": id,
        "package_id": id,
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
            "task_id": id,
            "outcome_key": "completed",
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
    })
}

/// A pool of constant capacity 1000 read from `resource.<the id without "pool.">`, valued 1000
/// milli a unit, with a gap weight when `gap`.
fn pool(id: &str, gap: bool) -> Value {
    let mut valuation =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    if gap {
        valuation["gap"] = json!({"rule": "shortfall_linear", "weight_milli": 1000});
    }
    json!({
        "id": id,
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": valuation
    })
}

/// The game's schedule: the rule of [`COLLECT`] with a fixed value, the rule of [`SPEND`]
/// without one, the two pools, and the two governed-only packages held.
fn schedule() -> Value {
    let mut collect = rule(COLLECT);
    collect["fixed_value_milli"] = json!(12_000);
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [BENEFIT_FIXED_VALUE_SUBSYSTEM],
        "rules": [collect, rule(SPEND)],
        "pools": [pool(VALUED, true), pool(UNVALUED, false)],
        "activity": [],
        "timeline": [],
        "held": [
            {"package_id": DISCARD, "reason": "Fixture: governed by a goal entry only."},
            {"package_id": KEEP, "reason": "Fixture: governed by a goal entry only."}
        ]
    })
}

/// The table [`TABLE`]: the keys `m01`, `m02`, `m03`.
fn table() -> Value {
    json!({
        "schema_version": "actingcommand.table.v1",
        "game": GAME,
        "table_id": TABLE,
        "row_key": ["member"],
        "columns": [
            {"name": "member", "type": "key"},
            {"name": "level", "type": "integer", "min": 0, "max": 100}
        ],
        "rows": [["m01", 1], ["m02", 2], ["m03", 3]]
    })
}

/// A `goals.json` that keeps every rule against this root: one entry of each of the eight types.
fn goals() -> Value {
    json!({
        "schema_version": "actingcommand.goals.v1",
        "game": GAME,
        "entries": [
            {"name": "stock", "type": "heading", "title": {"en": "Stock"}},
            {"name": "about", "type": "note", "title": {"en": "About"},
             "help": {"en": "Text to read."}},
            {"name": "points", "type": "resource", "governs": {"pool": VALUED},
             "title": {"en": "Points to hold"},
             "rate": {"floor_milli_per_day": 120000, "ceiling_milli_per_day": 900000}},
            {"name": "keep", "type": "integer", "governs": {"packages": [COLLECT]},
             "title": {"en": "Points to keep"}, "min": 0, "max": 200, "step": 10, "default": 50},
            {"name": "route", "type": "choice", "governs": {"packages": [COLLECT, KEEP]},
             "title": {"en": "Route"},
             "options": [{"value": "a", "title": {"en": "Route A"}},
                         {"value": "b", "title": {"en": "Route B"}}],
             "default": "a"},
            {"name": "enabled", "type": "switch", "governs": {"packages": [SPEND]},
             "title": {"en": "Spend"}, "default": false},
            {"name": "members", "type": "ordered", "governs": {"packages": [COLLECT]},
             "title": {"en": "Members"},
             "from_table": {"table": TABLE, "names": {"m01": {"en": "First member"}}},
             "min_count": 0, "max_count": 2, "default": ["m02"]},
            {"name": "weight", "type": "tier", "governs": {"rules": [COLLECT]},
             "title": {"en": "Weight"},
             "options": [{"value": "normal", "title": {"en": "Normal"}, "scale_milli": 1000},
                         {"value": "more", "title": {"en": "More"}, "scale_milli": 2000}],
             "default": "normal"}
        ]
    })
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

/// An installed game root `<temporary>/packages/neutral/`: `bundle.json` listing the four packages,
/// each in its directory, `schedule.json` and the table; no `goals.json`.
fn installed() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join("packages").join(GAME);
    fs::create_dir_all(&game_root).expect("game root");
    let mut listed = Vec::new();
    for (package_id, provides) in packs() {
        write_pack(&game_root.join(package_id), package_id, provides);
        listed.push(json!({
            "package_id": package_id, "server": SERVER, "entry_task_id": "task",
            "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
        }));
    }
    write_json(
        &game_root.join("bundle.json"),
        &json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": listed}),
    );
    write_json(&game_root.join("schedule.json"), &schedule());
    write_json(
        &game_root.join("tables").join(format!("{TABLE}.json")),
        &table(),
    );
    (temp, game_root)
}

fn check(game_root: &Path) -> StandardPackCheckResponse {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_owned(),
    })
    .unwrap_or_else(|error| panic!("the root is checked: {}: {}", error.code, error.message))
}

/// The `standard_pack_root_invalid` warnings of an answer, each `{code, values}`.
fn root_invalid_warnings(warnings: &[Outcome]) -> Vec<Value> {
    warnings
        .iter()
        .filter(|warning| warning.code().as_str() == ROOT_INVALID)
        .map(|warning| json!({"code": warning.code().as_str(), "values": warning.values()}))
        .collect()
}

/// The warning `standard_pack_root_invalid` of `goals.json` in `game_root` with `problem` at
/// `pointer`.
fn goal_warning(game_root: &Path, problem: &str, pointer: &str) -> Value {
    let path = std::path::absolute(game_root)
        .expect("absolute game root")
        .join("goals.json");
    json!({
        "code": ROOT_INVALID,
        "values": {
            "game": GAME,
            "path": path.to_string_lossy(),
            "standard_pack_problem": problem,
            "json_pointer": pointer
        }
    })
}

/// The answer as JSON without `root_files`.
fn without_root_files(answer: &StandardPackCheckResponse) -> Value {
    let mut value = serde_json::to_value(answer).expect("the answer");
    value
        .as_object_mut()
        .expect("an object")
        .shift_remove("root_files")
        .expect("the answer lists its root files");
    value
}

/// [`goals`] changed by `change`.
fn goals_with(change: impl Fn(&mut Value)) -> Value {
    let mut value = goals();
    change(&mut value);
    value
}

// Each refusal of the check, one fixture each: the valid file changed at one entry is refused with
// exactly one `standard_pack_root_invalid` warning naming `goals.json`, its word and its pointer,
// and the root is checked, `goals.json` among its root files.
#[test]
fn gr1_check_reports_each_refusal_at_its_pointer() {
    let (_temp, game_root) = installed();
    let cases: [(&str, Value, &str); 13] = [
        (
            "goal_name_duplicate",
            goals_with(|value| value["entries"][1]["name"] = json!("stock")),
            "/entries/1/name",
        ),
        (
            "goal_type_unknown",
            goals_with(|value| value["entries"][1]["type"] = json!("slider")),
            "/entries/1/type",
        ),
        (
            "goal_block_mismatch",
            goals_with(|value| value["entries"][3]["block"] = json!("goal_row")),
            "/entries/3/block",
        ),
        (
            "goal_pool_unknown",
            goals_with(|value| value["entries"][2]["governs"]["pool"] = json!("pool.neutral.none")),
            "/entries/2/governs/pool",
        ),
        (
            "goal_pool_unvalued",
            goals_with(|value| value["entries"][2]["governs"]["pool"] = json!(UNVALUED)),
            "/entries/2/governs/pool",
        ),
        (
            "goal_package_unknown",
            goals_with(|value| {
                value["entries"][4]["governs"]["packages"] = json!([COLLECT, "neutral.none"]);
            }),
            "/entries/4/governs/packages/1",
        ),
        (
            "goal_governs_irreversible",
            goals_with(|value| value["entries"][5]["governs"]["packages"] = json!([DISCARD])),
            "/entries/5/governs/packages/0",
        ),
        (
            "goal_rule_unknown",
            goals_with(|value| value["entries"][7]["governs"]["rules"] = json!(["neutral.none"])),
            "/entries/7/governs/rules/0",
        ),
        (
            "goal_tier_without_fixed_value",
            goals_with(|value| value["entries"][7]["governs"]["rules"] = json!([SPEND])),
            "/entries/7/governs/rules/0",
        ),
        (
            "goal_range_invalid",
            goals_with(|value| value["entries"][3]["min"] = json!(300)),
            "/entries/3/min",
        ),
        (
            "goal_default_invalid",
            goals_with(|value| value["entries"][4]["default"] = json!("c")),
            "/entries/4/default",
        ),
        (
            "goal_table_unknown",
            goals_with(|value| {
                value["entries"][6]["from_table"]["table"] = json!("neutral_none");
            }),
            "/entries/6/from_table/table",
        ),
        (
            "goal_rate_invalid",
            goals_with(|value| {
                value["entries"][2]["rate"]["ceiling_milli_per_day"] = json!(100000);
            }),
            "/entries/2/rate/ceiling_milli_per_day",
        ),
    ];
    for (problem, file, pointer) in cases {
        write_json(&game_root.join("goals.json"), &file);
        let checked = check(&game_root);
        assert_eq!(checked.status, "checked", "{problem}");
        assert_eq!(
            checked.root_files,
            ["bundle.json", "schedule.json", "goals.json"],
            "{problem}"
        );
        assert_eq!(
            root_invalid_warnings(&checked.warnings),
            [goal_warning(&game_root, problem, pointer)],
            "{problem}"
        );
    }
}

// A root with a valid file answers as the same root without it in every member but `root_files`,
// which lists `goals.json` after the files it listed; no warning names the file.
#[test]
fn gr1_check_answers_alike_with_and_without_a_valid_file() {
    let (_temp, game_root) = installed();
    let without = check(&game_root);
    assert_eq!(without.root_files, ["bundle.json", "schedule.json"]);
    write_json(&game_root.join("goals.json"), &goals());
    let with = check(&game_root);
    assert_eq!(with.status, "checked");
    assert_eq!(
        with.root_files,
        ["bundle.json", "schedule.json", "goals.json"]
    );
    assert!(
        root_invalid_warnings(&with.warnings).is_empty(),
        "{:?}",
        with.warnings
    );
    assert_eq!(without_root_files(&with), without_root_files(&without));
}

// A refused file's problems are reported in file order, the reader's as well as the references',
// and the root is not failed: the answer is the answer without the file but for `root_files` and
// those warnings, which come last.
#[test]
fn gr1_check_reports_a_refused_file_in_file_order_and_never_fails_the_root() {
    let (_temp, game_root) = installed();
    let without = check(&game_root);
    let reader = goals_with(|value| {
        value["entries"][4]["default"] = json!("c");
        value["entries"][1]["name"] = json!("stock");
        value["entries"][3]["min"] = json!(300);
        value["entries"][2]["rate"]["ceiling_milli_per_day"] = json!(100000);
    });
    let references = goals_with(|value| {
        value["entries"][7]["governs"]["rules"] = json!(["neutral.none"]);
        value["entries"][5]["governs"]["packages"] = json!([DISCARD]);
        value["entries"][2]["governs"]["pool"] = json!("pool.neutral.none");
        value["entries"][4]["governs"]["packages"] = json!(["neutral.none", COLLECT]);
    });
    let cases = [
        (
            "the reader's",
            reader,
            [
                ("goal_name_duplicate", "/entries/1/name"),
                ("goal_rate_invalid", "/entries/2/rate/ceiling_milli_per_day"),
                ("goal_range_invalid", "/entries/3/min"),
                ("goal_default_invalid", "/entries/4/default"),
            ],
        ),
        (
            "the references'",
            references,
            [
                ("goal_pool_unknown", "/entries/2/governs/pool"),
                ("goal_package_unknown", "/entries/4/governs/packages/0"),
                ("goal_governs_irreversible", "/entries/5/governs/packages/0"),
                ("goal_rule_unknown", "/entries/7/governs/rules/0"),
            ],
        ),
    ];
    for (case, file, problems) in cases {
        write_json(&game_root.join("goals.json"), &file);
        let refused = check(&game_root);
        assert_eq!(refused.status, "checked", "{case}");
        let expected = problems
            .iter()
            .map(|(problem, pointer)| goal_warning(&game_root, problem, pointer))
            .collect::<Vec<_>>();
        assert_eq!(root_invalid_warnings(&refused.warnings), expected, "{case}");
        // The goal warnings come last; without them the answer is the one without the file.
        let mut answer = without_root_files(&refused);
        let warnings = answer["warnings"]
            .as_array_mut()
            .expect("the warnings")
            .split_off(without.warnings.len());
        assert_eq!(warnings.len(), expected.len(), "{case}");
        assert_eq!(answer, without_root_files(&without), "{case}");
    }
}

/// The inputs of one `package bundle` with `--schedule`: the applications table, one source
/// directory per package below `<temporary>/sources`, `schedule.json` and `--out`
/// `<temporary>/bundle`; no `--goals`.
fn bundle_inputs() -> (TempDir, PackageBundleRequest) {
    let temp = TempDir::new().expect("temporary directory");
    let applications = temp.path().join("applications.json");
    write_json(
        &applications,
        &json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {SERVER: {"application_id": "com.example.neutral", "label": "Test"}}
        }),
    );
    let packs_root = temp.path().join("sources");
    for (package_id, provides) in packs() {
        write_pack(&packs_root.join(package_id), package_id, provides);
    }
    let schedule_file = temp.path().join("schedule.json");
    write_json(&schedule_file, &schedule());
    let request = PackageBundleRequest {
        applications,
        packs_root,
        out: temp.path().join("bundle"),
        source: None,
        maintenance: None,
        root_files: StandardPackRootFiles {
            schedule: Some(schedule_file),
            ..StandardPackRootFiles::default()
        },
    };
    (temp, request)
}

/// `package bundle` of `request` with `--goals` at `<temporary>/goals-source.json` holding `file`.
fn bundle_with_goals(
    temp: &TempDir,
    request: &PackageBundleRequest,
    file: &Value,
) -> (PathBuf, LabResult<crate::PackageBundleResponse>) {
    let source = temp.path().join("goals-source.json");
    write_json(&source, file);
    let mut with_goals = request.clone();
    with_goals.root_files.goals = Some(source.clone());
    (source, package_bundle(with_goals))
}

/// A refusal's code and the values of the outcome it carries.
fn refusal(error: &LabError) -> (String, Value) {
    let Some(details) = error.details.as_ref() else {
        panic!("{}: {} carries no outcome", error.code, error.message);
    };
    let outcome = &details["outcome"];
    assert_eq!(outcome["code"], error.code.as_str(), "{}", error.message);
    (error.code.clone(), outcome["values"].clone())
}

/// Every file below `directory` by its `/`-separated path, with its bytes.
fn files_below(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![(directory.to_path_buf(), String::new())];
    while let Some((current, prefix)) = pending.pop() {
        for entry in fs::read_dir(&current).expect("directory listing") {
            let entry = entry.expect("directory entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().expect("entry type").is_dir() {
                pending.push((entry.path(), relative));
            } else {
                files.insert(relative, fs::read(entry.path()).expect("file bytes"));
            }
        }
    }
    files
}

// The safety line kept out at the check: an entry that governs a package whose control
// declaration provides an irreversible capability is `goal_governs_irreversible` at that package,
// for `standard-pack check` (a warning; the root is checked) and for `package bundle --goals` (the
// bundle refused, `--out` not written); a package whose capability is not irreversible may be
// governed by the same entry.
#[test]
fn gr1_governs_irreversible_is_refused_by_the_check_and_the_bundle() {
    let governing = |package_id: &str| {
        let package_id = package_id.to_owned();
        goals_with(move |value| {
            value["entries"][5]["governs"]["packages"] = json!([COLLECT, package_id]);
        })
    };
    let (_temp, game_root) = installed();
    write_json(&game_root.join("goals.json"), &governing(KEEP));
    let accepted = check(&game_root);
    assert!(
        root_invalid_warnings(&accepted.warnings).is_empty(),
        "{:?}",
        accepted.warnings
    );
    write_json(&game_root.join("goals.json"), &governing(DISCARD));
    let refused = check(&game_root);
    assert_eq!(refused.status, "checked");
    assert_eq!(
        root_invalid_warnings(&refused.warnings),
        [goal_warning(
            &game_root,
            "goal_governs_irreversible",
            "/entries/5/governs/packages/1"
        )]
    );

    let (temp, request) = bundle_inputs();
    let (_, accepted) = bundle_with_goals(&temp, &request, &governing(KEEP));
    accepted
        .unwrap_or_else(|error| panic!("a reversible package: {}: {}", error.code, error.message));
    let (temp, request) = bundle_inputs();
    let (source, refused) = bundle_with_goals(&temp, &request, &governing(DISCARD));
    let error = refused.expect_err("an irreversible package refuses the bundle");
    let (code, values) = refusal(&error);
    assert_eq!(code, ROOT_INVALID, "{}", error.message);
    assert_eq!(
        values,
        json!({
            "game": GAME,
            "path": source.to_string_lossy(),
            "standard_pack_problem": "goal_governs_irreversible",
            "json_pointer": "/entries/5/governs/packages/1"
        })
    );
    assert!(!request.out.exists(), "--out was written");
}

// `package bundle --goals` lays an accepted file beside `bundle.json` byte for byte; the rest of
// the output, `bundle.json` and its index included, is what the bundle without the flag holds. The
// bundle holds no table, so the `ordered` entry's table is not judged. A refused file refuses the
// bundle as a refused root file does, at the source file, with `--out` not written.
#[test]
fn gr1_package_bundle_lays_the_file_and_refuses_a_refused_one() {
    let (temp, request) = bundle_inputs();
    let plain_out = temp.path().join("plain");
    let plain = package_bundle(PackageBundleRequest {
        out: plain_out.clone(),
        ..request.clone()
    })
    .unwrap_or_else(|error| panic!("without --goals: {}: {}", error.code, error.message));
    let (source, laid) = bundle_with_goals(&temp, &request, &goals());
    let laid =
        laid.unwrap_or_else(|error| panic!("with --goals: {}: {}", error.code, error.message));
    assert_eq!(laid.status, "written");
    assert_eq!(
        fs::read(request.out.join("goals.json")).expect("the laid goals.json"),
        fs::read(&source).expect("the source goals.json")
    );
    let mut others = files_below(&request.out);
    others.remove("goals.json").expect("goals.json is laid");
    assert_eq!(others, files_below(&plain_out));
    assert_eq!(
        serde_json::to_value(&laid.index).expect("index JSON"),
        serde_json::to_value(&plain.index).expect("index JSON")
    );
    assert_eq!(
        fs::read_dir(&request.out)
            .expect("the bundle")
            .map(|entry| entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(
            [
                "applications.json",
                "bundle.json",
                "goals.json",
                "packs",
                "schedule.json"
            ]
            .map(str::to_owned)
        )
    );

    let (temp, request) = bundle_inputs();
    let (source, refused) = bundle_with_goals(
        &temp,
        &request,
        &goals_with(|value| {
            value["entries"][2]["rate"]["ceiling_milli_per_day"] = json!(100000);
        }),
    );
    let error = refused.expect_err("a refused file refuses the bundle");
    let (code, values) = refusal(&error);
    assert_eq!(code, ROOT_INVALID, "{}", error.message);
    assert_eq!(
        values,
        json!({
            "game": GAME,
            "path": source.to_string_lossy(),
            "standard_pack_problem": "goal_rate_invalid",
            "json_pointer": "/entries/2/rate/ceiling_milli_per_day"
        })
    );
    assert!(!request.out.exists(), "--out was written");
}
