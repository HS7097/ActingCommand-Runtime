// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 CV-2 (register §1.508; contracts/pack-catalog.md, "Who writes it"; coordinator work
// order `rd-1122/brief-477-cv2.md`, its rulings 3 and 7 and its test 6): `standard-pack catalog
// --cut` cuts a root over to its version 2 `pack-catalog.json`, and the tools compose the root it
// leaves with the catalog.
// - the two compositions the same: the file is written, `catalog_business` named and every
//   business member taken off, `standard-pack compose` of the root cut over gives the digest the
//   cut answered, and a second line answers the root done, `unchanged` (1);
// - a rule whose part says otherwise: nothing is written and the refusal names the rule, by the
//   catalog's words; a root without a catalog is refused at it, `catalog_missing` (2);
// - `--plan`: nothing is written, the switched file is answered (3).
// Fix round 1 (the coordinator's ruling on the independent review's second point): on a switched
// root the catalog's writer never takes a business part from the rules, which write none:
// - a missing catalog is not made, `validation_failed`, the root named switched (4);
// - a catalog of version 1 is neither converted nor brought up to date (5);
// - a new package's entry holds its display only, the other entries as written (6).
// A made-up game `neutral`, server `test`; every package, pool, rule and key is a placeholder. No
// clock is read and nothing sleeps.

use crate::{
    StandardPackCatalogCutRequest, StandardPackCatalogCutResponse, StandardPackCatalogRequest,
    StandardPackCatalogResponse, StandardPackComposeRequest, compose_standard_packs,
    cut_standard_pack_catalog, write_standard_pack_catalog,
};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_policy::{BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package, and its rule, that produces [`POINTS`].
const COLLECT: &str = "neutral.collect";
/// The package, and its rule, that consumes [`POINTS`] on a value the model wrote.
const SPEND: &str = "neutral.spend";
const POINTS: &str = "pool.neutral.points";
/// The business members a rule of a root cut over writes none of.
const BUSINESS: [&str; 6] = [
    "consumes",
    "produces",
    "limited",
    "realization",
    "fixed_value_origin",
    "fixed_value_milli",
];

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("the file")).expect("JSON")
}

/// One effect on [`POINTS`].
fn effect(direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": POINTS,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A `schedule.json` rule `id` of its own package on an hourly clock, with its effects.
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

/// The root's `schedule.json`, its rules writing their business part: [`COLLECT`] produces 2,
/// [`SPEND`] consumes 1 on a fixed value of 3 000 milli the model wrote, and is limited.
fn schedule() -> Value {
    let mut spend = rule(SPEND, vec![effect("consume", 1)], Vec::new());
    spend["fixed_value_milli"] = json!(3000);
    spend["fixed_value_origin"] = json!("model");
    spend["limited"] = json!(true);
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [BENEFIT_FIXED_VALUE_SUBSYSTEM],
        "rules": [rule(COLLECT, Vec::new(), vec![effect("produce", 2)]), spend],
        "pools": [{
            "id": POINTS,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.neutral.points"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
        }],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// The root's version 2 catalog: [`COLLECT`]'s entry as its rule writes it, [`SPEND`]'s consuming
/// `spent` a run, beside the rule's value and mark.
fn catalog(spent: u64) -> Value {
    json!({
        "schema_version": "actingcommand.pack-catalog.v2",
        "game": GAME,
        "packs": [
            {"package_id": COLLECT, "consumes": [], "produces": [effect("produce", 2)]},
            {
                "package_id": SPEND,
                "consumes": [effect("consume", spent)],
                "produces": [],
                "value": {"milli": 3000, "origin": "model"},
                "limited": true
            }
        ]
    })
}

/// A built game root `<temporary>/packs/neutral/`: `bundle.json` of [`COLLECT`] and [`SPEND`],
/// [`schedule`], and `catalog` when given.
fn game_root(catalog: Option<&Value>) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("packs").join(GAME);
    let packs = [COLLECT, SPEND]
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id, "server": SERVER, "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    write_json(
        &root.join("bundle.json"),
        &json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": packs}),
    );
    write_json(&root.join("schedule.json"), &schedule());
    if let Some(catalog) = catalog {
        write_json(&root.join("pack-catalog.json"), catalog);
    }
    (temp, root)
}

fn cut(root: &Path, plan: bool) -> LabResult<StandardPackCatalogCutResponse> {
    cut_standard_pack_catalog(StandardPackCatalogCutRequest {
        root: root.to_owned(),
        plan,
    })
}

fn done(root: &Path, plan: bool) -> StandardPackCatalogCutResponse {
    cut(root, plan).unwrap_or_else(|error| panic!("the cut: {}: {}", error.code, error.message))
}

/// The refusal's details.
fn details(error: &LabError) -> Value {
    error.details.clone().unwrap_or(Value::Null)
}

/// The bytes of the root's `schedule.json`.
fn schedule_bytes(root: &Path) -> Vec<u8> {
    fs::read(root.join("schedule.json")).expect("the file")
}

/// Test 6, equal results (ruling 7): the root composes the same with its rules' business part
/// taken from the catalog, so `--cut` writes `schedule.json` (`written`): `requires` names
/// `catalog_business` after `benefit_fixed_value`, no rule writes a business member, every other
/// member is kept. `standard-pack compose` of the root cut over, for the instance the cut composes
/// with, reads the catalog and gives the digest the cut answered (ruling 3). A second line answers
/// the root done, `unchanged`, and writes nothing.
#[test]
fn cv2_cut_equal_results_write_the_file_and_a_second_line_answers_done() {
    let (temp, root) = game_root(Some(&catalog(1)));
    let answer = done(&root, false);
    assert_eq!(answer.status, "written");
    assert_eq!(answer.rules, [COLLECT, SPEND]);
    let digest = answer
        .rules_digest
        .clone()
        .expect("the digest of both compositions");
    let switched = read_json(&root.join("schedule.json"));
    assert_eq!(
        switched["requires"],
        json!([BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM])
    );
    for written in switched["rules"].as_array().expect("the rules") {
        for member in BUSINESS {
            assert!(written.get(member).is_none(), "{member}: {written}");
        }
        assert_eq!(written["priority"], 100, "{written}");
    }

    let composed = compose_standard_packs(StandardPackComposeRequest {
        root: temp.path().join("packs"),
        instances: vec![(
            format!("standard-pack-check.{SERVER}"),
            GAME.to_owned(),
            SERVER.to_owned(),
        )],
        out: temp.path().join("composed"),
    })
    .unwrap_or_else(|error| {
        panic!(
            "the root cut over composes: {}: {}",
            error.code, error.message
        )
    });
    assert_eq!(composed.rules_digest, digest);

    let before = schedule_bytes(&root);
    let again = done(&root, false);
    assert_eq!(again.status, "unchanged");
    assert!(again.rules.is_empty() && again.rules_digest.is_none());
    assert_eq!(schedule_bytes(&root), before);
}

/// Test 6, a difference (ruling 7): [`SPEND`]'s entry consumes 2 where its rule consumes 1, so the
/// cut writes nothing and is refused `validation_failed`, naming the rule: `quantity_differs` on
/// `consumes` of [`POINTS`], the catalog's word, in `details.differences`, and as one warning in
/// `details.warnings`. A root without a catalog is refused at it, `standard_pack_root_invalid`
/// `catalog_missing`, and nothing is written either.
#[test]
fn cv2_cut_a_difference_writes_nothing_and_names_the_rule() {
    let (_temp, root) = game_root(Some(&catalog(2)));
    let before = schedule_bytes(&root);
    let error = cut(&root, false).expect_err("the cut is refused");
    assert_eq!(error.code, "validation_failed", "{}", error.message);
    let details = details(&error);
    assert_eq!(
        details["differences"],
        json!([{
            "package_id": SPEND,
            "pool_id": POINTS,
            "side": "consumes",
            "difference": "quantity_differs",
            "rule_id": SPEND
        }]),
        "{details}"
    );
    assert_eq!(
        details["warnings"].as_array().map(Vec::len),
        Some(1),
        "{details}"
    );
    assert_eq!(schedule_bytes(&root), before);

    let (_bare, root) = game_root(None);
    let before = schedule_bytes(&root);
    let error = cut(&root, false).expect_err("a root without a catalog is refused");
    assert_eq!(
        error.code, "standard_pack_root_invalid",
        "{}",
        error.message
    );
    let outcome = self::details(&error)["outcome"].clone();
    assert_eq!(
        outcome["values"]["standard_pack_problem"],
        "catalog_missing"
    );
    assert!(
        outcome["values"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("pack-catalog.json")),
        "{outcome}"
    );
    assert_eq!(schedule_bytes(&root), before);
}

/// Test 6, `--plan` (ruling 7): the cut that would be written is answered, `planned`, the
/// switched `schedule.json` as its `document`, and nothing is written.
#[test]
fn cv2_cut_plan_writes_nothing() {
    let (_temp, root) = game_root(Some(&catalog(1)));
    let before = schedule_bytes(&root);
    let answer = done(&root, true);
    assert_eq!(answer.status, "planned");
    assert!(answer.rules_digest.is_some());
    let document = answer.document.expect("the switched file");
    assert_eq!(
        document["requires"],
        json!([BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM])
    );
    assert!(
        document["rules"][1].get("fixed_value_milli").is_none(),
        "{document}"
    );
    assert_eq!(schedule_bytes(&root), before);
}

/// A third package, run on a root cut over by a rule of its own, without an entry in the catalog.
const NEW: &str = "neutral.new";

/// A plain line of `standard-pack catalog` on `root`, with `--schema-version 2` when `version_2`.
fn write(root: &Path, version_2: bool) -> LabResult<StandardPackCatalogResponse> {
    write_standard_pack_catalog(StandardPackCatalogRequest {
        root: root.to_owned(),
        plan: false,
        refresh: Vec::new(),
        displays: None,
        values: None,
        version_2,
    })
}

/// The root of [`game_root`] with its version 2 catalog, cut over by `--cut`.
fn switched_root() -> (TempDir, PathBuf) {
    let (temp, root) = game_root(Some(&catalog(1)));
    assert_eq!(done(&root, false).status, "written");
    (temp, root)
}

/// `error` is the writer's refusal of a line on a switched root: `validation_failed`, the root
/// named switched and its business part to be restored, not rebuilt.
fn refused_as_switched(error: &LabError) {
    assert_eq!(error.code, "validation_failed", "{}", error.message);
    assert!(
        error.message.contains("the root is switched")
            && error.message.contains("restored, not rebuilt"),
        "{}",
        error.message
    );
}

/// Fix round 1 (4): a switched root whose catalog is gone (as it answers `catalog_missing`) does
/// not get one made of its rules, which write no business part: the line is refused, with
/// `--schema-version 2` too, and nothing is written.
#[test]
fn cv2_fr1_the_writer_makes_no_catalog_for_a_switched_root() {
    let (_temp, root) = switched_root();
    let file = root.join("pack-catalog.json");
    fs::remove_file(&file).expect("the catalog removed");
    for version_2 in [false, true] {
        let error = write(&root, version_2).expect_err("the line is refused");
        refused_as_switched(&error);
        assert!(!file.exists(), "nothing is written");
    }
}

/// Fix round 1 (5): a switched root whose catalog is of version 1 (as it answers
/// `catalog_version_1`) has it neither converted by `--schema-version 2` nor brought up to date
/// without it: each line is refused and the file keeps its bytes.
#[test]
fn cv2_fr1_the_writer_converts_no_version_1_catalog_of_a_switched_root() {
    let (_temp, root) = switched_root();
    let file = root.join("pack-catalog.json");
    write_json(
        &file,
        &json!({
            "schema_version": "actingcommand.pack-catalog.v1",
            "game": GAME,
            "packs": [
                {"package_id": COLLECT, "consumes": [], "produces": [effect("produce", 2)]},
                {"package_id": SPEND, "consumes": [effect("consume", 1)], "produces": []}
            ]
        }),
    );
    let before = fs::read(&file).expect("the file");
    for version_2 in [true, false] {
        let error = write(&root, version_2).expect_err("the line is refused");
        refused_as_switched(&error);
        assert_eq!(fs::read(&file).expect("the file"), before);
    }
}

/// Fix round 1 (6): a package new to a switched root (in `bundle.json`, run by a rule that writes
/// no business member) gets an entry of its display only, no business part, which is none, as for
/// a package without an entry; the entries there are kept as written.
#[test]
fn cv2_fr1_a_new_package_of_a_switched_root_gets_its_display_only() {
    let (_temp, root) = switched_root();
    let bundle_path = root.join("bundle.json");
    let mut bundle = read_json(&bundle_path);
    bundle["packs"]
        .as_array_mut()
        .expect("the packs")
        .push(json!({
            "package_id": NEW, "server": SERVER, "entry_task_id": "task",
            "path": format!("packs/{NEW}"), "file_count": 3, "byte_count": 64
        }));
    write_json(&bundle_path, &bundle);
    let schedule_path = root.join("schedule.json");
    let mut schedule = read_json(&schedule_path);
    let mut new_rule = rule(NEW, Vec::new(), Vec::new());
    for member in BUSINESS {
        new_rule.as_object_mut().expect("a rule").remove(member);
    }
    schedule["rules"]
        .as_array_mut()
        .expect("the rules")
        .push(new_rule);
    write_json(&schedule_path, &schedule);
    let file = root.join("pack-catalog.json");
    let before = read_json(&file);
    let answer = write(&root, false)
        .unwrap_or_else(|error| panic!("the line: {}: {}", error.code, error.message));
    assert_eq!(answer.status, "written");
    assert_eq!(answer.added, [NEW]);
    let after = read_json(&file);
    assert_eq!(after["packs"][0], before["packs"][0]);
    assert_eq!(after["packs"][1], before["packs"][1]);
    assert_eq!(after["packs"][2], json!({"package_id": NEW}));
}
