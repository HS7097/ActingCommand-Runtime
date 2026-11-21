// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R1 (register §1.356; contracts/standard-pack-goals.md; coordinator work orders
// `rd-1114/brief-455-gr1.md` and `rd-1114/brief-1694-fix1.md`): the Tools read a game root's goal
// declaration `goals.json` after the composition they already run.
// - `standard-pack check` reports each refusal it can give at its pointer, one fixture each, as a
//   `standard_pack_root_invalid` warning naming `goals.json`, and the root is checked: a refused
//   file never fails it;
// - a root with a valid file answers as the same root without it in every member but
//   `root_files`, which lists `goals.json`;
// - a refused file's problems come in file order, and the answer is otherwise the answer without
//   the file;
// - the safety line at the check and the bundle: a package whose control declaration has an
//   irreversible capability can be reached by no entry;
// - `package bundle --goals` lays an accepted file beside `bundle.json` byte for byte, the rest of
//   its output as without the flag, and refuses a refused file as a refused root file, with
//   `--out` not written;
// - fix round 1 (the independent review's two ways around the safety line): a `tier` entry over a
//   rule whose package provides the irreversible capability, and an entry that reaches a package
//   providing unmarked a capability another package marks irreversible, are each
//   `goal_governs_irreversible` at their pointer and nothing else, for the check and the bundle;
// - fix round 2 (`rd-1114/brief-1694-fix2.md`, the differential review's way around the safety
//   line): a `tier` entry over the head of a chain whose second follower runs the package that
//   provides the irreversible capability is `goal_governs_irreversible` at the head's pointer and
//   nothing else, for the check and the bundle, while the same chain with reversible followers
//   passes both.
// Workflow #463 V1 (register §1.378; contracts/package-variables.md; coordinator work order
// `rd-1114/brief-463-v1-package-variables.md`) replaced the four entry types that governed packages
// by the placement `variable`, so these tests were adapted, each to what it pins: the packages
// declare the variables the valid file places where it held the four types; the refusals the
// check can still give are reported one fixture each (`goal_package_unknown` cannot be: the
// composition refuses a rule of an unknown package first, and no entry takes a table any more),
// `goal_variable_unknown` among them; the safety line of the irreversible package is kept by
// admission (such a package declaring a variable fails the check and the bundle), and a
// placement whose variable a package providing the irreversible capability unmarked declares is
// `goal_governs_irreversible` at the placement's `name`. V1 fix round 1
// (`rd-1114/brief-1723-fix1.md`): the refusal of the irreversible package's variable names
// `variables_on_irreversible`, and the root check reports the unmarked provider's variable
// `variable_irreversible_by_root`.
// Workflow #455 G-SLx (0.12.78; register §1.501; coordinator work order
// `rd-1121/brief-455-locks.md`, rulings 1 and 2) lifts that safety line: a package that provides
// an irreversible capability may declare variables, and a goal entry may govern it. The four
// tests that pinned the line left the gate map for four `gslx_` tests on the same fixtures: the
// package that marks the capability declares a variable and the root and the bundle pass; a
// `tier` over its rule, a placement of a variable of the package that provides it unmarked (which
// applies) and a `tier` over the head of a chain it follows are accepted by the check and the
// bundle; and on each such root every other refusal of the check is the one before. A fifth
// shows that a root none of whose packages marks a capability irreversible answers as before.
// The first test lost its last fixture, `goal_governs_irreversible`, which nothing gives now.
// A made-up game `neutral`, server `test`, 16x9; every package, pool, rule, variable and text is a
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
/// The package most entries reach; its rule writes a fixed value.
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
/// A table of the root, which (Workflow #463 V1) no entry reads any more.
const TABLE: &str = "neutral_members";
const ROOT_INVALID: &str = "standard_pack_root_invalid";

/// One package of the root: its id, what its control provides and (Workflow #463 V1) the
/// variables it declares.
#[derive(Clone)]
struct Pack {
    id: &'static str,
    provides: Option<Value>,
    variables: Option<Value>,
}

/// Writes the package `pack` as a package directory: its control (with `provides` and, Workflow
/// #463 V1, `variables` when it has them), its resource table and its one task, from page `home`
/// (a blue first pixel) to page `page_b` (a red one).
fn write_pack(directory: &Path, pack: &Pack) {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": pack.id,
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
    if let Some(provides) = &pack.provides {
        control["provides"] = provides.clone();
    }
    if let Some(variables) = &pack.variables {
        control["variables"] = variables.clone();
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

/// Workflow #463 V1: the variables the valid file places, where it held the four types that
/// governed packages: [`COLLECT`] declares `keep` (a number) and `order` (an order of three),
/// [`KEEP`] `route` (a choice) and [`SPEND`] `enabled` (a switch).
fn variables_of(package_id: &str) -> Option<Value> {
    match package_id {
        COLLECT => Some(json!([
            {"name": "keep", "type": "integer", "title": {"en": "Points to keep"},
             "min": 0, "max": 200, "step": 10, "default": 50},
            {"name": "order", "type": "ordered", "title": {"en": "Members"},
             "options": [{"value": "m01", "title": {"en": "First member"}},
                         {"value": "m02", "title": {"en": "Second member"}},
                         {"value": "m03", "title": {"en": "Third member"}}],
             "min_count": 0, "max_count": 2, "default": ["m02"]}
        ])),
        KEEP => Some(json!([
            {"name": "route", "type": "choice", "title": {"en": "Route"},
             "options": [{"value": "a", "title": {"en": "Route A"}},
                         {"value": "b", "title": {"en": "Route B"}}],
             "default": "a"}
        ])),
        SPEND => Some(json!([
            {"name": "enabled", "type": "switch", "title": {"en": "Spend"}, "default": false}
        ])),
        _ => None,
    }
}

/// The four packages of the root, each with what its control provides and the variables it
/// declares.
fn packs() -> [Pack; 4] {
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
    .map(|(id, provides)| Pack {
        id,
        provides,
        variables: variables_of(id),
    })
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

/// G-R1 fix round 1: [`schedule`] with a rule of [`DISCARD`], the package that provides the
/// irreversible capability, writing a fixed value; [`DISCARD`] no longer held.
fn schedule_running_discard() -> Value {
    let mut value = schedule();
    let mut discard = rule(DISCARD);
    discard["fixed_value_milli"] = json!(12_000);
    value["rules"]
        .as_array_mut()
        .expect("the rules")
        .push(discard);
    value["held"]
        .as_array_mut()
        .expect("the held packages")
        .retain(|held| held["package_id"] != DISCARD);
    value
}

/// G-R1 fix round 1: [`packs`] with [`KEEP`] providing, without the mark, the capability
/// [`DISCARD`] marks irreversible, with the same parameter.
fn packs_sharing_the_irreversible_capability() -> [Pack; 4] {
    let mut packs = packs();
    packs[3].provides = Some(json!([{"capability": "cap_discard", "parameters": [
        {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}]}]));
    packs
}

/// A table of the root: the keys `m01`, `m02`, `m03`.
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

/// A `goals.json` that keeps every rule against this root: one entry of each of the types and
/// (Workflow #463 V1) a placement of each variable the packages declare, at the positions the
/// four replaced types held; version 2 since V1 fix round 3.
fn goals() -> Value {
    json!({
        "schema_version": "actingcommand.goals.v2",
        "game": GAME,
        "entries": [
            {"name": "stock", "type": "heading", "title": {"en": "Stock"}},
            {"name": "about", "type": "note", "title": {"en": "About"},
             "help": {"en": "Text to read."}},
            {"name": "points", "type": "resource", "governs": {"pool": VALUED},
             "title": {"en": "Points to hold"},
             "rate": {"floor_milli_per_day": 120000, "ceiling_milli_per_day": 900000}},
            {"name": "keep", "type": "variable", "block": "number"},
            {"name": "route", "type": "variable"},
            {"name": "enabled", "type": "variable", "block": "switch"},
            {"name": "order", "type": "variable"},
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
    installed_with(packs(), &schedule())
}

/// [`installed`] of the packages `packs` and the schedule `schedule`.
fn installed_with(packs: [Pack; 4], schedule: &Value) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join("packages").join(GAME);
    fs::create_dir_all(&game_root).expect("game root");
    let mut listed = Vec::new();
    for pack in packs {
        write_pack(&game_root.join(pack.id), &pack);
        listed.push(json!({
            "package_id": pack.id, "server": SERVER, "entry_task_id": "task",
            "path": format!("packs/{}", pack.id), "file_count": 3, "byte_count": 64
        }));
    }
    write_json(
        &game_root.join("bundle.json"),
        &json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": listed}),
    );
    write_json(&game_root.join("schedule.json"), schedule);
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

/// Each refusal of the check it can still give (Workflow #463 V1), one fixture each (fix round 1:
/// `goal_out_of_limits` among them; V1: `goal_variable_unknown`): the valid file changed at one
/// entry, with the word and the pointer of its one problem.
fn refusal_cases() -> [(&'static str, Value, &'static str); 12] {
    [
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
            "goal_variable_unknown",
            goals_with(|value| value["entries"][4]["name"] = json!("none")),
            "/entries/4/name",
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
            goals_with(|value| value["entries"][7]["options"][0]["scale_milli"] = json!(0)),
            "/entries/7/options/0/scale_milli",
        ),
        (
            "goal_default_invalid",
            goals_with(|value| value["entries"][7]["default"] = json!("less")),
            "/entries/7/default",
        ),
        (
            "goal_rate_invalid",
            goals_with(|value| {
                value["entries"][2]["rate"]["ceiling_milli_per_day"] = json!(100000);
            }),
            "/entries/2/rate/ceiling_milli_per_day",
        ),
        // Fix round 1: a title of 129 bytes, one above its limit.
        (
            "goal_out_of_limits",
            goals_with(|value| value["entries"][0]["title"]["en"] = json!("a".repeat(129))),
            "/entries/0/title/en",
        ),
    ]
}

/// Each of [`refusal_cases`] written as the game root's `goals.json` is refused with exactly one
/// `standard_pack_root_invalid` warning naming `goals.json`, its word and its pointer, and the
/// root is checked, `goals.json` among its root files. The last case's file is left in place.
fn assert_each_refusal(game_root: &Path) {
    for (problem, file, pointer) in refusal_cases() {
        write_json(&game_root.join("goals.json"), &file);
        let checked = check(game_root);
        assert_eq!(checked.status, "checked", "{problem}");
        assert_eq!(
            checked.root_files,
            ["bundle.json", "schedule.json", "goals.json"],
            "{problem}"
        );
        assert_eq!(
            root_invalid_warnings(&checked.warnings),
            [goal_warning(game_root, problem, pointer)],
            "{problem}"
        );
    }
}

// Each refusal of the check it can still give, one fixture each (`refusal_cases`): exactly one
// warning at its pointer, the root checked. Workflow #455 G-SLx: the fixture that ended this
// test, a `tier` over the rule of the package of the irreversible capability
// (`goal_governs_irreversible`), is one the check accepts now
// (`gslx_a_tier_over_a_rule_of_an_irreversible_package_is_accepted`).
#[test]
fn gr1_check_reports_each_refusal_at_its_pointer() {
    let (_temp, game_root) = installed();
    assert_each_refusal(&game_root);
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
        value["entries"][7]["default"] = json!("less");
        value["entries"][1]["name"] = json!("stock");
        value["entries"][3]["block"] = json!("goal_row");
        value["entries"][2]["rate"]["ceiling_milli_per_day"] = json!(100000);
    });
    let references = goals_with(|value| {
        value["entries"][7]["governs"]["rules"] = json!(["neutral.none"]);
        value["entries"][5]["name"] = json!("none");
        value["entries"][2]["governs"]["pool"] = json!("pool.neutral.none");
        value["entries"][4]["name"] = json!("absent");
    });
    let cases = [
        (
            "the reader's",
            reader,
            [
                ("goal_name_duplicate", "/entries/1/name"),
                ("goal_rate_invalid", "/entries/2/rate/ceiling_milli_per_day"),
                ("goal_block_mismatch", "/entries/3/block"),
                ("goal_default_invalid", "/entries/7/default"),
            ],
        ),
        (
            "the references'",
            references,
            [
                ("goal_pool_unknown", "/entries/2/governs/pool"),
                ("goal_variable_unknown", "/entries/4/name"),
                ("goal_variable_unknown", "/entries/5/name"),
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
    bundle_inputs_with(packs(), &schedule())
}

/// [`bundle_inputs`] of the packages `packs` and the schedule `schedule`.
fn bundle_inputs_with(packs: [Pack; 4], schedule: &Value) -> (TempDir, PackageBundleRequest) {
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
    for pack in packs {
        write_pack(&packs_root.join(pack.id), &pack);
    }
    let schedule_file = temp.path().join("schedule.json");
    write_json(&schedule_file, schedule);
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

/// [`goals`] with a placement of the variable `discarded`, which [`declaring_discard`]'s package
/// of the irreversible capability declares, after its other entries.
fn goals_placing_discarded() -> Value {
    goals_with(|value| {
        value["entries"]
            .as_array_mut()
            .expect("the entries")
            .push(json!({"name": "discarded", "type": "variable", "block": "switch"}));
    })
}

/// Workflow #455 G-SLx: [`packs`] with [`DISCARD`], whose control marks its capability
/// irreversible, declaring the switch `discarded`.
fn declaring_discard() -> [Pack; 4] {
    let mut packs = packs();
    packs[2].variables = Some(json!([
        {"name": "discarded", "type": "switch", "title": {"en": "Discard"}, "default": false}
    ]));
    packs
}

/// `package bundle --goals` of `goals` on the packages `packs` and the schedule `schedule`, which
/// must lay it beside `bundle.json` byte for byte (Workflow #455 G-SLx, where G-R1 refused such a
/// bundle `goal_governs_irreversible`).
fn assert_bundle_lays(packs: [Pack; 4], schedule: &Value, goals: &Value) {
    let (temp, request) = bundle_inputs_with(packs, schedule);
    let (source, laid) = bundle_with_goals(&temp, &request, goals);
    let laid = laid.unwrap_or_else(|error| panic!("the bundle: {}: {}", error.code, error.message));
    assert_eq!(laid.status, "written");
    assert_eq!(
        fs::read(request.out.join("goals.json")).expect("the laid goals.json"),
        fs::read(&source).expect("the source goals.json")
    );
}

/// The `standard_pack_root_invalid` warnings `check` gives of `game_root` with `goals` as its
/// `goals.json`, and its variable listing.
fn checked_with(game_root: &Path, goals: &Value) -> (Vec<Value>, Value) {
    write_json(&game_root.join("goals.json"), goals);
    let checked = check(game_root);
    assert_eq!(checked.status, "checked");
    (
        root_invalid_warnings(&checked.warnings),
        serde_json::to_value(&checked.variable_provenance).expect("the listing"),
    )
}

// Workflow #455 G-SLx (ruling 1, where G-R1 and V1 kept the line here): a package whose control
// declaration marks its capability irreversible declares a variable, `discarded`, which the valid
// file places. `standard-pack check` reads the root and the file without a warning (no
// `variables_on_irreversible`, no `goal_governs_irreversible`), the variable applies and is
// listed by its default's origin; every other refusal of the check on that root is the one
// before; `package bundle --goals` lays the file.
#[test]
fn gslx_a_package_that_marks_its_capability_irreversible_declares_a_variable_a_goal_places() {
    let (_temp, game_root) = installed_with(declaring_discard(), &schedule());
    let (warnings, listing) = checked_with(&game_root, &goals_placing_discarded());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        listing,
        json!({"pack": ["keep", "order", "discarded", "route", "enabled"], "model": []})
    );
    assert_each_refusal(&game_root);
    assert_bundle_lays(declaring_discard(), &schedule(), &goals_placing_discarded());
}

// `package bundle --goals` lays an accepted file beside `bundle.json` byte for byte; the rest of
// the output, `bundle.json` and its index included, is what the bundle without the flag holds. Its
// placements are held to the variables the bundle's packs declare. A refused file refuses the
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

/// [`goals`] with its `tier` entry governing `rules`.
fn tier_over(rules: &[&str]) -> Value {
    let rules = json!(rules);
    goals_with(move |value| value["entries"][7]["governs"]["rules"] = rules.clone())
}

/// The variable listing of the root of [`packs`] with the valid file: every variable applies.
fn every_variable_applies() -> Value {
    json!({"pack": ["keep", "order", "route", "enabled"], "model": []})
}

// Workflow #455 G-SLx (ruling 2, where G-R1 fix round 1, item 1, refused it
// `goal_governs_irreversible`): on a root where the package that marks the irreversible capability
// runs a rule that writes a fixed value, a `tier` over that rule beside the rule of
// `neutral.collect` is accepted by the check (no warning) and by `package bundle --goals` (the
// file laid), as the tier over the rule of `neutral.collect` alone is; every other refusal of the
// check on that root is the one before, the `tier` over a rule without a fixed value among them
// (`goal_tier_without_fixed_value`), and so is the `tier` over that rule and a rule the schedule
// does not have (`goal_rule_unknown` at the second only).
#[test]
fn gslx_a_tier_over_a_rule_of_an_irreversible_package_is_accepted() {
    let (_temp, game_root) = installed_with(packs(), &schedule_running_discard());
    let accepted: [&[&str]; 3] = [&[COLLECT], &[COLLECT, DISCARD], &[DISCARD]];
    for rules in accepted {
        let (warnings, listing) = checked_with(&game_root, &tier_over(rules));
        assert!(warnings.is_empty(), "{rules:?}: {warnings:?}");
        assert_eq!(listing, every_variable_applies(), "{rules:?}");
    }
    let (warnings, _) = checked_with(&game_root, &tier_over(&[DISCARD, "neutral.none"]));
    assert_eq!(
        warnings,
        [goal_warning(
            &game_root,
            "goal_rule_unknown",
            "/entries/7/governs/rules/1"
        )]
    );
    assert_each_refusal(&game_root);
    assert_bundle_lays(
        packs(),
        &schedule_running_discard(),
        &tier_over(&[COLLECT, DISCARD]),
    );
}

// Workflow #455 G-SLx (rulings 1 and 2, where G-R1 fix round 1, item 2, and V1 fix round 1 kept
// the line here): `neutral.keep` provides, without the mark, the capability `neutral.discard`
// marks irreversible, so it counts as able to do something irreversible by the whole root. Its
// variable `route` applies (no `variable_irreversible_by_root`, listed by its default's origin
// and under no `not_applying`), the valid file's placement of it is accepted (no
// `goal_governs_irreversible`), for the check and for `package bundle --goals`, exactly as on the
// root where `neutral.keep` provides a capability of its own; every other refusal of the check
// on that root is the one before.
#[test]
fn gslx_an_unmarked_provider_of_an_irreversible_capability_is_governed_and_its_variable_applies() {
    let (_plain, plain_root) = installed();
    let (warnings, listing) = checked_with(&plain_root, &goals());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(listing, every_variable_applies());

    let (_temp, game_root) =
        installed_with(packs_sharing_the_irreversible_capability(), &schedule());
    let (warnings, listing) = checked_with(&game_root, &goals());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(listing, every_variable_applies());
    assert_each_refusal(&game_root);
    assert_bundle_lays(
        packs_sharing_the_irreversible_capability(),
        &schedule(),
        &goals(),
    );
}

/// G-R1 fix round 2: an edge, `dependency_completed` of `task_id` accepting `succeeded` alone,
/// with `since_own_run: true`, as `gate_cov1` writes one.
fn edge(task_id: &str) -> Value {
    json!({
        "kind": "dependency_completed",
        "task_id": task_id,
        "terminal_states": ["succeeded"],
        "since_own_run": true
    })
}

/// G-R1 fix round 2: [`schedule`] with a two-step chain behind the rule of [`COLLECT`], its head,
/// which writes a fixed value: the rule of [`KEEP`] follows the head (its hourly clock and an edge
/// to the head, under `all`) and the rule of `second` follows the rule of [`KEEP`] (an edge to it,
/// under `all`), in place of any rule `second` had. Neither follower writes a value, so the
/// composition lists both as riding with the head; neither package is held.
fn schedule_with_chain(second: &str) -> Value {
    let mut value = schedule();
    let mut first = rule(KEEP);
    let clock = first["trigger"].take();
    first["trigger"] = json!({"kind": "all", "predicates": [clock, edge(COLLECT)]});
    let mut follower = rule(second);
    follower["trigger"] = json!({"kind": "all", "predicates": [edge(KEEP)]});
    let rules = value["rules"].as_array_mut().expect("the rules");
    rules.retain(|existing| existing["id"] != second);
    rules.push(first);
    rules.push(follower);
    value["held"]
        .as_array_mut()
        .expect("the held packages")
        .retain(|held| held["package_id"] != KEEP && held["package_id"] != second);
    value
}

// Workflow #455 G-SLx (ruling 2, where G-R1 fix round 2 refused it `goal_governs_irreversible`):
// the valid file's `tier` entry governs the rule of `neutral.collect`, the head of a two-step
// chain. When the second follower runs `neutral.discard`, the package that marks the irreversible
// capability, the composition still lists both followers with the head, and the tier is accepted
// by the check (no warning) and by `package bundle --goals` (the file laid), as on the same chain
// whose second follower runs `neutral.spend`; every other refusal of the check on that root is
// the one before.
#[test]
fn gslx_a_tier_over_a_chain_head_whose_follower_runs_an_irreversible_package_is_accepted() {
    let (_reversible, reversible_root) = installed_with(packs(), &schedule_with_chain(SPEND));
    let (warnings, _) = checked_with(&reversible_root, &goals());
    assert!(warnings.is_empty(), "{warnings:?}");

    let (_temp, game_root) = installed_with(packs(), &schedule_with_chain(DISCARD));
    write_json(&game_root.join("goals.json"), &goals());
    let accepted = check(&game_root);
    assert_eq!(accepted.status, "checked");
    assert_eq!(
        serde_json::to_value(&accepted.benefit_followers).expect("the followers"),
        json!([
            {"task_id": KEEP, "rides_with": COLLECT},
            {"task_id": DISCARD, "rides_with": COLLECT}
        ])
    );
    assert!(
        root_invalid_warnings(&accepted.warnings).is_empty(),
        "{:?}",
        accepted.warnings
    );
    assert_each_refusal(&game_root);
    assert_bundle_lays(packs(), &schedule_with_chain(DISCARD), &goals());
}

// Workflow #455 G-SLx (the brief's test 4): a root none of whose packages marks a capability
// irreversible (`neutral.discard` provides its capability unmarked), checked with the valid file,
// gives no warning and lists every variable by its default's origin, gives each refusal of the
// check exactly as before, and `package bundle --goals` lays the file, as on main. The tests of
// G-R1 and V1 that this slice leaves unchanged pin the rest of those answers.
#[test]
fn gslx_a_root_without_an_irreversible_capability_answers_as_before() {
    let reversible = || {
        let mut packs = packs();
        packs[2].provides = Some(json!([{"capability": "cap_discard", "parameters": [
            {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}]}]));
        packs
    };
    let (_temp, game_root) = installed_with(reversible(), &schedule());
    let (warnings, listing) = checked_with(&game_root, &goals());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(listing, every_variable_applies());
    assert_each_refusal(&game_root);
    assert_bundle_lays(reversible(), &schedule(), &goals());
}
