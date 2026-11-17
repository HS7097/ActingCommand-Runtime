// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 PC-1 (register §1.438; contracts/pack-catalog.md; coordinator work order
// `rd-1116/brief-477-pc1-pack-catalog.md`, the tests of `check`, of the writer and of the
// composition):
// - `standard-pack check` gives each finding of a game root's `pack-catalog.json` with its code: a
//   root without the file is the note `standard_pack_catalog_absent`; a display defect is the
//   warning `standard_pack_catalog_display_invalid`; each difference from the rules of
//   `schedule.json` the warning `standard_pack_catalog_schedule_differs` naming the package, the
//   pool, the side, which file lacks it and (when a rule names it) the rule; the packages without an
//   entry are counted and listed (`standard_pack_catalog_coverage`); a leftover `ui.json` is the
//   warning `standard_pack_ui_retired`;
// - a business part that is wrong refuses the root (`standard_pack_root_invalid` at its pointer)
//   and leaves every pack of it not ready (readiness's S7 fails); a display defect changes no
//   pack's readiness;
// - the writer `standard-pack catalog` builds the file from `bundle.json` and `schedule.json`
//   alone, takes a leftover `ui.json`'s display, changes nothing when run again, keeps an existing
//   entry's business part and reports its differences, writes nothing with `--plan`, and refuses a
//   root inside an installation;
// - the composition of the root is the same, by its catalog hash and its rules digest, with and
//   without the file: nothing the Runtime runs reads it.
// Fix round 1 (coordinator work order `rd-1116/brief-477-pc1-fix2.md`, after the independent
// review):
// - the writer takes a resource repository's checkout, a directory not named as its game with
//   `applications.json`, `packs/` (and a `README.md` there) and `schedule.json` but no
//   `bundle.json`: the game from `applications.json`, the registry the build's `package_bundle`
//   makes, the file written at the checkout's root and nothing else written there;
// - what the writer leaves out (an entry of a package `bundle.json` does not register, a pool
//   `schedule.json` does not declare) is answered in `removed` and `dropped` and warned of,
//   `standard_pack_catalog_left_out` with the word, the package, the pool and the pointer in the
//   existing file.
// A made-up game `neutral`, server `test`, 16x9; every package, pool, rule and text is a
// placeholder. No clock is read and nothing sleeps.

use crate::{
    StandardPackCatalogRequest, StandardPackCheckRequest, StandardPackCheckResponse,
    StandardPackComposeRequest, StandardPackReadyRequest, check_standard_pack_root,
    compose_standard_packs, judge_standard_pack_readiness, write_standard_pack_catalog,
};
use actingcommand_contract::LabError;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package whose rule produces [`POINTS`].
const COLLECT: &str = "neutral.collect";
/// The package whose rule consumes [`POINTS`].
const SPEND: &str = "neutral.spend";
/// A pool the two rules name.
const POINTS: &str = "pool.neutral.points";
/// A pool no rule names.
const STOCK: &str = "pool.neutral.stock";
const CATALOG: &str = "pack-catalog.json";

/// Writes the package `package_id` as a package directory: its control, its resource table and
/// its one task, from page `home` (a blue first pixel) to page `page_b` (a red one).
fn write_pack(directory: &Path, package_id: &str) {
    let control = json!({
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
        "goal": "PC-1 fixture",
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
        write_json(&directory.join(path), &value);
    }
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

/// One effect of a rule on `pool`: `direction` `produce` or `consume`, `amount` a run.
fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A `schedule.json` rule running the package of its own id on an hourly clock.
fn rule(id: &str, consumes: Vec<Value>, produces: Vec<Value>) -> Value {
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
        "consumes": consumes,
        "produces": produces,
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

/// A pool of constant capacity 1000 read from `resource.<the id without "pool.">`, valued.
fn pool(id: &str) -> Value {
    json!({
        "id": id,
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
    })
}

/// The game's schedule: [`COLLECT`]'s rule produces 2 of [`POINTS`] a run, [`SPEND`]'s consumes
/// 1; both pools declared.
fn schedule() -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": [
            rule(COLLECT, Vec::new(), vec![effect(POINTS, "produce", 2)]),
            rule(SPEND, vec![effect(POINTS, "consume", 1)], Vec::new())
        ],
        "pools": [pool(POINTS), pool(STOCK)],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// The bundle index of [`COLLECT`] and [`SPEND`].
fn bundle() -> Value {
    let packs = [COLLECT, SPEND]
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id, "server": SERVER, "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": packs})
}

/// An installed game root `<temporary>/packages/neutral/`: `bundle.json`, both packages in their
/// directories and `schedule.json`; no `pack-catalog.json`.
fn installed() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join("packages").join(GAME);
    for package_id in [COLLECT, SPEND] {
        write_pack(&game_root.join(package_id), package_id);
    }
    write_json(&game_root.join("bundle.json"), &bundle());
    write_json(&game_root.join("schedule.json"), &schedule());
    (temp, game_root)
}

/// A catalog of [`GAME`] with `packs` and one group.
fn catalog(packs: Value) -> Value {
    json!({
        "schema_version": "actingcommand.pack-catalog.v1",
        "game": GAME,
        "groups": [{"id": "daily", "title": {"en": "Daily"}}],
        "packs": packs
    })
}

/// The entry of `package_id` whose business part is exactly its rule's.
fn matching_entry(package_id: &str) -> Value {
    let (consumes, produces) = if package_id == COLLECT {
        (
            json!([]),
            json!([{"pool_id": POINTS, "quantity": {"kind": "constant", "amount": 2}}]),
        )
    } else {
        (
            json!([{"pool_id": POINTS, "quantity": {"kind": "constant", "amount": 1}}]),
            json!([]),
        )
    };
    json!({
        "package_id": package_id,
        "title": {"en": "Fixture"},
        "summary": {"en": "A fixture package."},
        "group": "daily",
        "consumes": consumes,
        "produces": produces
    })
}

fn check(game_root: &Path) -> StandardPackCheckResponse {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_owned(),
    })
    .unwrap_or_else(|error| panic!("the root is checked: {}: {}", error.code, error.message))
}

/// The warnings of `answer` with a code of this slice, each `{code, values}`.
fn pc1_warnings(warnings: &[Outcome]) -> Vec<Value> {
    let codes = [
        SharedCode::StandardPackCatalogDisplayInvalid,
        SharedCode::StandardPackCatalogLeftOut,
        SharedCode::StandardPackCatalogScheduleDiffers,
        SharedCode::StandardPackUiRetired,
    ];
    warnings
        .iter()
        .filter(|warning| codes.iter().any(|code| warning.is(*code)))
        .map(|warning| json!({"code": warning.code().as_str(), "values": warning.values()}))
        .collect()
}

fn absolute(path: &Path) -> String {
    std::path::absolute(path)
        .expect("an absolute path")
        .display()
        .to_string()
}

/// Check: each finding with its code. Without the file the answer's `catalog` is the note
/// `standard_pack_catalog_absent`, nothing is listed and no warning of this slice is given. With a
/// file whose `COLLECT` entry names a group that does not exist, consumes a pool no rule of it
/// consumes and lacks the pool its rule produces, and no entry for `SPEND`, beside a leftover
/// `ui.json`: the root is checked, the file among its root files, and the warnings are the display
/// defect, the two differences (consumes first; the one a rule names with its rule) and the
/// leftover file; the note `standard_pack_catalog_coverage` counts `SPEND`, listed.
#[test]
fn pc1_check_gives_each_finding_with_its_code() {
    let (_temp, game_root) = installed();
    let without = check(&game_root);
    assert_eq!(
        without.catalog.note.code().as_str(),
        SharedCode::StandardPackCatalogAbsent.as_str()
    );
    assert_eq!(without.catalog.note.value("game"), Some(&json!(GAME)));
    assert_eq!(
        without.catalog.note.value("path"),
        Some(&json!(absolute(&game_root.join(CATALOG))))
    );
    assert_eq!(without.catalog.unlisted, None);
    assert!(!without.root_files.contains(&CATALOG));
    assert_eq!(pc1_warnings(&without.warnings), Vec::<Value>::new());

    let mut collect = matching_entry(COLLECT);
    collect["group"] = json!("weekly");
    collect["consumes"] = json!([{"pool_id": STOCK}]);
    collect["produces"] = json!([]);
    write_json(&game_root.join(CATALOG), &catalog(json!([collect])));
    write_json(
        &game_root.join("ui.json"),
        &json!({"schema_version": "actingcommand.ui.v1", "game": GAME, "groups": [], "packs": []}),
    );

    let answer = check(&game_root);

    assert!(
        answer.root_files.contains(&CATALOG),
        "{:?}",
        answer.root_files
    );
    let path = absolute(&game_root.join(CATALOG));
    assert_eq!(
        pc1_warnings(&answer.warnings),
        vec![
            json!({"code": "standard_pack_catalog_display_invalid", "values": {
                "game": GAME, "path": path, "standard_pack_problem": "catalog_group_unknown",
                "json_pointer": "/packs/0/group"}}),
            json!({"code": "standard_pack_catalog_schedule_differs", "values": {
                "game": GAME, "package_id": COLLECT, "pool_id": STOCK,
                "catalog_side": "consumes", "catalog_difference": "schedule_lacks"}}),
            json!({"code": "standard_pack_catalog_schedule_differs", "values": {
                "game": GAME, "package_id": COLLECT, "pool_id": POINTS,
                "catalog_side": "produces", "catalog_difference": "catalog_lacks",
                "task_id": COLLECT}}),
            json!({"code": "standard_pack_ui_retired", "values": {
                "game": GAME, "path": absolute(&game_root.join("ui.json"))}}),
        ]
    );
    assert_eq!(
        answer.catalog.note.code().as_str(),
        SharedCode::StandardPackCatalogCoverage.as_str()
    );
    assert_eq!(answer.catalog.note.value("count"), Some(&json!(1)));
    assert_eq!(answer.catalog.note.value("path"), Some(&json!(path)));
    assert_eq!(answer.catalog.unlisted, Some(vec![SPEND.to_owned()]));
}

/// Each pack's readiness and its thirteen items as JSON, by package.
fn readiness(game_root: &Path) -> Vec<(String, Value, Value)> {
    let answer = judge_standard_pack_readiness(&StandardPackReadyRequest {
        root: game_root.to_owned(),
        declared_package: None,
        installation: None,
    })
    .unwrap_or_else(|error| panic!("the root is judged: {}: {}", error.code, error.message));
    serde_json::to_value(answer).expect("the readiness")["packs"]
        .as_array()
        .expect("the packs")
        .iter()
        .map(|pack| {
            (
                pack["package_id"]
                    .as_str()
                    .expect("a package id")
                    .to_owned(),
                pack["readiness"].clone(),
                pack["items"].clone(),
            )
        })
        .collect()
}

/// The state of the item `item` of `items`.
fn item_state<'a>(items: &'a Value, item: &str) -> &'a Value {
    &items
        .as_array()
        .expect("the items")
        .iter()
        .find(|answer| answer["item"] == item)
        .unwrap_or_else(|| panic!("no item {item}: {items}"))["state"]
}

fn refusal(error: &LabError) -> Value {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Check and readiness: a business part that is wrong (a package the bundle does not register)
/// refuses the root, `standard_pack_root_invalid` naming the file, its problem and its pointer, and
/// leaves every pack not ready, its S7 (`root_checked`) failing; a display defect alone (a package
/// naming a group that does not exist, its business part its rules') is checked and changes no
/// pack's readiness or item from the root's without the file.
#[test]
fn pc1_a_wrong_business_part_makes_the_root_not_ready_and_a_display_defect_does_not() {
    let (_temp, game_root) = installed();
    let before = readiness(&game_root);
    assert_eq!(before.len(), 2);

    let mut absent = matching_entry(COLLECT);
    absent["package_id"] = json!("neutral.absent");
    write_json(&game_root.join(CATALOG), &catalog(json!([absent])));
    let error = check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.clone(),
    })
    .expect_err("a wrong business part refuses the root");
    assert_eq!(error.code, "standard_pack_root_invalid");
    let outcome = refusal(&error);
    assert_eq!(
        outcome["values"]["standard_pack_problem"],
        "package_not_listed"
    );
    assert_eq!(outcome["values"]["json_pointer"], "/packs/0/package_id");
    assert_eq!(
        outcome["values"]["path"],
        json!(absolute(&game_root.join(CATALOG)))
    );
    for (package_id, readiness, items) in readiness(&game_root) {
        assert_eq!(readiness, "waiting", "{package_id}");
        assert_eq!(item_state(&items, "root_checked"), "fails", "{package_id}");
    }

    let mut defect = matching_entry(COLLECT);
    defect["group"] = json!("weekly");
    write_json(
        &game_root.join(CATALOG),
        &catalog(json!([defect, matching_entry(SPEND)])),
    );
    let checked = check(&game_root);
    assert_eq!(
        pc1_warnings(&checked.warnings).len(),
        1,
        "{:?}",
        checked.warnings
    );
    assert_eq!(
        readiness(&game_root),
        before,
        "a display defect changes no readiness"
    );
}

/// A game root of a working copy: `<temporary>/repo/neutral/` with `bundle.json` and
/// `schedule.json` only (the writer admits no package).
fn working_root() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("repo").join(GAME);
    write_json(&root.join("bundle.json"), &bundle());
    write_json(&root.join("schedule.json"), &schedule());
    (temp, root)
}

fn write(root: &Path, plan: bool) -> crate::StandardPackCatalogResponse {
    write_standard_pack_catalog(StandardPackCatalogRequest {
        root: root.to_owned(),
        plan,
        refresh: Vec::new(),
        displays: None,
    })
    .unwrap_or_else(|error| panic!("the writer: {}: {}", error.code, error.message))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("the file")).expect("JSON")
}

/// The writer from `bundle.json` and `schedule.json` alone: one entry per package, no display, the
/// business part its rules' (a constant where every rule states one), no difference and no
/// warning; run again it finds the file holding its bytes and writes nothing. With a leftover
/// `ui.json` (and no catalog) a plan takes its display, groups and `display`, warns of the file and
/// writes nothing.
#[test]
fn pc1_the_writer_builds_the_file_from_the_root_and_a_second_run_changes_nothing() {
    let (_temp, root) = working_root();
    let first = write(&root, false);
    assert_eq!(first.status, "written");
    assert!(first.changed);
    assert_eq!(first.added, vec![COLLECT.to_owned(), SPEND.to_owned()]);
    assert!(first.kept.is_empty() && first.differences.is_empty());
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);
    assert_eq!(
        read_json(&root.join(CATALOG)),
        json!({
            "schema_version": "actingcommand.pack-catalog.v1",
            "game": GAME,
            "packs": [
                {"package_id": COLLECT, "consumes": [],
                 "produces": [{"pool_id": POINTS, "quantity": {"kind": "constant", "amount": 2}}]},
                {"package_id": SPEND,
                 "consumes": [{"pool_id": POINTS, "quantity": {"kind": "constant", "amount": 1}}],
                 "produces": []}
            ]
        })
    );
    let written = fs::read(root.join(CATALOG)).expect("the file");

    let second = write(&root, false);
    assert_eq!(second.status, "unchanged");
    assert!(!second.changed);
    assert!(second.added.is_empty());
    assert_eq!(second.kept, vec![COLLECT.to_owned(), SPEND.to_owned()]);
    assert_eq!(fs::read(root.join(CATALOG)).expect("the file"), written);

    let (_other, root) = working_root();
    let ui = json!({
        "schema_version": "actingcommand.ui.v1",
        "game": GAME,
        "groups": [{"id": "daily", "title": {"en": "Daily"}}],
        "packs": [{"package_id": SPEND, "title": {"en": "Spend"},
                   "summary": {"en": "Spends points."}, "group": "daily", "risk": ["costly"]}],
        "display": {"aspect_ratios": ["16:9"]}
    });
    write_json(&root.join("ui.json"), &ui);
    let planned = write(&root, true);
    assert_eq!(planned.status, "planned");
    assert_eq!(planned.display_from_ui, vec![SPEND.to_owned()]);
    let document = planned.document.expect("a plan answers its document");
    assert_eq!(document["groups"], ui["groups"]);
    assert_eq!(document["display"], ui["display"]);
    let spend = &document["packs"][1];
    for member in ["title", "summary", "group", "risk"] {
        assert_eq!(spend[member], ui["packs"][0][member], "{member}");
    }
    assert_eq!(document["packs"][0].get("title"), None);
    assert_eq!(
        planned
            .warnings
            .iter()
            .map(|warning| warning.code().as_str())
            .collect::<Vec<_>>(),
        vec![SharedCode::StandardPackUiRetired.as_str()]
    );
    assert!(!root.join(CATALOG).exists(), "a plan writes nothing");
}

/// The writer over an existing file: an entry already there keeps its business part as written,
/// and each difference from the rules is reported (in the answer and as a warning), never
/// overwritten; a package without an entry gets one. `--plan` writes nothing. A root inside an
/// installation (a directory along it holding `install\state.json`) is refused
/// `standard_pack_catalog_root_refused` naming the root and the installation, before anything is
/// read or written.
#[test]
fn pc1_the_writer_keeps_an_entry_reports_its_differences_and_refuses_an_installation() {
    let (_temp, root) = working_root();
    let mut kept = matching_entry(COLLECT);
    kept["produces"] = json!([{"pool_id": STOCK}]);
    write_json(&root.join(CATALOG), &catalog(json!([kept.clone()])));
    let before = fs::read(root.join(CATALOG)).expect("the file");

    let planned = write(&root, true);
    assert_eq!(planned.status, "planned");
    assert!(planned.changed);
    assert_eq!(
        fs::read(root.join(CATALOG)).expect("the file"),
        before,
        "a plan writes nothing"
    );

    let answer = write(&root, false);
    assert_eq!(answer.status, "written");
    assert_eq!(answer.kept, vec![COLLECT.to_owned()]);
    assert_eq!(answer.added, vec![SPEND.to_owned()]);
    let written = read_json(&root.join(CATALOG));
    assert_eq!(written["packs"][0], kept, "the entry is kept as written");
    assert_eq!(
        serde_json::to_value(&answer.differences).expect("the differences"),
        json!([
            {"package_id": COLLECT, "pool_id": POINTS, "side": "produces",
             "difference": "catalog_lacks", "rule_id": COLLECT},
            {"package_id": COLLECT, "pool_id": STOCK, "side": "produces",
             "difference": "schedule_lacks"}
        ])
    );
    let differs = answer
        .warnings
        .iter()
        .filter(|warning| warning.is(SharedCode::StandardPackCatalogScheduleDiffers))
        .count();
    assert_eq!(differs, 2, "{:?}", answer.warnings);

    let temp = TempDir::new().expect("temporary directory");
    let installation = temp.path().join("installed");
    fs::create_dir_all(installation.join("install")).expect("the install folder");
    fs::write(installation.join("install").join("state.json"), b"{}").expect("state.json");
    let inside = installation.join("packages").join(GAME);
    write_json(&inside.join("bundle.json"), &bundle());
    let error = write_standard_pack_catalog(StandardPackCatalogRequest {
        root: inside.clone(),
        plan: false,
        refresh: Vec::new(),
        displays: None,
    })
    .expect_err("a root inside an installation is refused");
    assert_eq!(error.code, "standard_pack_catalog_root_refused");
    let outcome = refusal(&error);
    assert_eq!(outcome["values"]["path"], json!(absolute(&inside)));
    assert_eq!(
        outcome["values"]["install_root"],
        json!(absolute(&installation))
    );
    assert!(!inside.join(CATALOG).exists());
}

/// The names of the entries of `directory`, sorted.
fn names(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .expect("the directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// Fix round 1, the review's first must-fix: the writer on a resource repository's checkout, a
/// directory named `checkout-not-named-as-game` holding `applications.json` (game `neutral`),
/// `packs/` with both packages' directories and a `README.md`, and `schedule.json`; no
/// `bundle.json`. The answer's game is `applications.json`'s, both packages get an entry (the
/// registry the build's `package_bundle` makes, the `README.md` no package), the file is written at
/// the checkout's root with the same document a built root of the same packages gets, and the
/// checkout holds nothing else new: no `bundle.json`, no scratch.
#[test]
fn pc1_the_writer_takes_a_resource_repository_s_checkout() {
    let temp = TempDir::new().expect("temporary directory");
    let checkout = temp.path().join("checkout-not-named-as-game");
    write_json(
        &checkout.join("applications.json"),
        &json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {SERVER: {"application_id": "com.example.neutral", "label": "Test"}}
        }),
    );
    for package_id in [COLLECT, SPEND] {
        write_pack(&checkout.join("packs").join(package_id), package_id);
    }
    fs::write(checkout.join("packs").join("README.md"), b"# packs\n").expect("README.md");
    write_json(&checkout.join("schedule.json"), &schedule());

    let answer = write(&checkout, false);

    assert_eq!(answer.status, "written");
    assert_eq!(answer.game, GAME);
    assert_eq!(answer.root, absolute(&checkout));
    assert_eq!(answer.path, absolute(&checkout.join(CATALOG)));
    assert_eq!(answer.added, vec![COLLECT.to_owned(), SPEND.to_owned()]);
    assert!(answer.warnings.is_empty(), "{:?}", answer.warnings);
    assert_eq!(
        read_json(&checkout.join(CATALOG)),
        json!({
            "schema_version": "actingcommand.pack-catalog.v1",
            "game": GAME,
            "packs": [
                {"package_id": COLLECT, "consumes": [],
                 "produces": [{"pool_id": POINTS, "quantity": {"kind": "constant", "amount": 2}}]},
                {"package_id": SPEND,
                 "consumes": [{"pool_id": POINTS, "quantity": {"kind": "constant", "amount": 1}}],
                 "produces": []}
            ]
        })
    );
    assert_eq!(
        names(&checkout),
        ["applications.json", CATALOG, "packs", "schedule.json"],
        "the checkout holds only the file it did not hold before"
    );
    assert_eq!(
        names(&checkout.join("packs")),
        ["README.md", COLLECT, SPEND]
    );
}

/// Fix round 1, the review's "may fix": what the writer leaves out of a file it brings up to date is
/// answered and warned of. The existing file's entry of `COLLECT` lists a pool `schedule.json` does
/// not declare and its second entry is of a package `bundle.json` does not register: the written
/// file keeps `COLLECT` without that pool and lists no entry for the other package; the answer's
/// `dropped` and `removed` name them; and each is one `standard_pack_catalog_left_out`, in the
/// existing file's order, with its word, package, pool and pointer there.
#[test]
fn pc1_the_writer_warns_of_what_it_removes_and_drops() {
    let (_temp, root) = working_root();
    let mut collect = matching_entry(COLLECT);
    collect["produces"]
        .as_array_mut()
        .expect("the produces")
        .push(json!({"pool_id": "pool.neutral.gone"}));
    let gone = json!({"package_id": "neutral.gone", "consumes": [], "produces": []});
    write_json(&root.join(CATALOG), &catalog(json!([collect, gone])));

    let answer = write(&root, false);

    assert_eq!(answer.status, "written");
    assert_eq!(answer.kept, vec![COLLECT.to_owned()]);
    assert_eq!(answer.added, vec![SPEND.to_owned()]);
    assert_eq!(answer.removed, vec!["neutral.gone".to_owned()]);
    assert_eq!(
        serde_json::to_value(&answer.dropped).expect("the dropped pools"),
        json!([{"package_id": COLLECT, "side": "produces", "pool_id": "pool.neutral.gone"}])
    );
    let written = read_json(&root.join(CATALOG));
    assert_eq!(written["packs"][0], matching_entry(COLLECT));
    assert_eq!(written["packs"][1]["package_id"], SPEND);
    assert_eq!(written["packs"].as_array().map(Vec::len), Some(2));
    let path = absolute(&root.join(CATALOG));
    assert_eq!(
        pc1_warnings(&answer.warnings),
        vec![
            json!({"code": "standard_pack_catalog_left_out", "values": {
                "game": GAME, "path": path, "standard_pack_problem": "catalog_pool_undeclared",
                "json_pointer": "/packs/0/produces/1/pool_id", "package_id": COLLECT,
                "pool_id": "pool.neutral.gone"}}),
            json!({"code": "standard_pack_catalog_left_out", "values": {
                "game": GAME, "path": path, "standard_pack_problem": "package_not_listed",
                "json_pointer": "/packs/1/package_id", "package_id": "neutral.gone"}}),
        ]
    );
}

/// The composition: the root composed for one bound instance gives the same catalog hash and the
/// same rules digest with and without `pack-catalog.json`.
#[test]
fn pc1_the_composition_is_the_same_with_and_without_the_file() {
    let (temp, game_root) = installed();
    let compose = |out: &str| {
        compose_standard_packs(StandardPackComposeRequest {
            root: temp.path().join("packages"),
            instances: vec![("node-a".to_owned(), GAME.to_owned(), SERVER.to_owned())],
            out: temp.path().join(out),
        })
        .unwrap_or_else(|error| panic!("composed: {}: {}", error.code, error.message))
    };
    let without = compose("without");
    write_json(
        &game_root.join(CATALOG),
        &catalog(json!([matching_entry(COLLECT), matching_entry(SPEND)])),
    );
    let with = compose("with");
    assert_eq!(with.catalog_hash, without.catalog_hash);
    assert_eq!(with.rules_digest, without.rules_digest);
}
