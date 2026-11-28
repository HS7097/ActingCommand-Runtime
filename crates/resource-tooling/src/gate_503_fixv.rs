// SPDX-License-Identifier: AGPL-3.0-only

// #503 FIXV (0.12.101; register §1.550; the coordinator's rulings on the owner's direction of
// 11-28, in its work order; contracts/pack-catalog.md, "Version 2"): the value of a version 2
// `pack-catalog.json` takes the signed range a rule's fixed value takes, as the compiler does.
// - `standard-pack catalog --schema-version 2` and `--refresh` write a rule's value of 0 or below
//   into its entry as it is, and find no difference between the two; `--values` writes a negative
//   value, and one equal to its rule's is no difference (1);
// - a root whose rule's negative value is taken from its catalog composes the same rules as the
//   file that writes it, so `standard-pack catalog --cut` switches it, and `standard-pack compose`
//   of the root cut over gives the digest the cut answered (2).
// The value beside a production of a pool whose valuation is pending, and the bounds, are in the
// adapted `gate_477_cv1::cv1_the_value_and_the_marks_are_taken_and_each_refusal_has_its_word`.
// A made-up game `neutral`, server `test`; every package, pool, rule and key is a placeholder. No
// clock is read and nothing sleeps.

use crate::{
    StandardPackCatalogCutRequest, StandardPackCatalogRequest, StandardPackCatalogResponse,
    StandardPackComposeRequest, compose_standard_packs, cut_standard_pack_catalog,
    write_standard_pack_catalog,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{
    BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM, FIXED_VALUE_ANY_SUBSYSTEM,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package, and its rule, that produces [`POINTS`] on a negative value the model wrote.
const COLLECT: &str = "neutral.collect";
/// The package, and its rule, that consumes [`POINTS`] on a value of 0.
const SPEND: &str = "neutral.spend";
const POINTS: &str = "pool.neutral.points";
const CATALOG: &str = "pack-catalog.json";
const VALUES: &str = "actingcommand.pack-catalog-values.v1";
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

/// One effect on [`POINTS`] of `amount` a run, `direction` `produce` or `consume`.
fn effect(direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": POINTS,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A `schedule.json` rule `id` of its own package on an hourly clock, with its effects and its
/// fixed value.
fn rule(id: &str, consumes: Vec<Value>, produces: Vec<Value>, value: i64) -> Value {
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
        "strategic_weight_milli": 1000,
        "fixed_value_milli": value
    })
}

/// The root's `schedule.json`: [`COLLECT`] produces 2 a run on a value of −2 500 milli the model
/// wrote, [`SPEND`] consumes 1 a run on a value of `spend_value`; it names `benefit_fixed_value`
/// and `fixed_value_any`.
fn schedule(spend_value: i64) -> Value {
    let mut collect = rule(COLLECT, Vec::new(), vec![effect("produce", 2)], -2_500);
    collect["fixed_value_origin"] = json!("model");
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [BENEFIT_FIXED_VALUE_SUBSYSTEM, FIXED_VALUE_ANY_SUBSYSTEM],
        "rules": [collect, rule(SPEND, vec![effect("consume", 1)], Vec::new(), spend_value)],
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

/// A built game root of a working copy, `<temporary>/packs/neutral/`, with `bundle.json` of
/// [`COLLECT`] and [`SPEND`] and the `schedule.json` of [`schedule`] (the writer admits no
/// package).
fn game_root(spend_value: i64) -> (TempDir, PathBuf) {
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
    write_json(&root.join("schedule.json"), &schedule(spend_value));
    (temp, root)
}

/// One line of the writer on `root`: `--refresh` of `refresh`, `--values` of `values`,
/// `--schema-version 2` with `version_2`.
fn write(
    root: &Path,
    refresh: &[&str],
    values: Option<&Path>,
    version_2: bool,
) -> StandardPackCatalogResponse {
    write_standard_pack_catalog(StandardPackCatalogRequest {
        root: root.to_owned(),
        plan: false,
        refresh: refresh
            .iter()
            .map(|package_id| (*package_id).to_owned())
            .collect(),
        displays: None,
        values: values.map(Path::to_path_buf),
        version_2,
    })
    .unwrap_or_else(|error| panic!("the writer: {}: {}", error.code, error.message))
}

/// A values file of `packs` beside the root's temporary directory.
fn values_file(directory: &Path, packs: Value) -> PathBuf {
    let path = directory.join("values").join("values.json");
    write_json(&path, &json!({"schema_version": VALUES, "packs": packs}));
    path
}

/// The values of each warning of `code` in `warnings`.
fn warned(warnings: &[Outcome], code: SharedCode) -> Vec<Value> {
    warnings
        .iter()
        .filter(|warning| warning.is(code))
        .map(|warning| json!(warning.values()))
        .collect()
}

/// (1) `--schema-version 2` converts the root's rules into version 2 entries, [`COLLECT`]'s value
/// −2 500 with its origin and [`SPEND`]'s 0, each as written, with no difference found; `--refresh`
/// of both then agrees and writes nothing. `--values` gives [`SPEND`] −700, written as it is, which
/// differs from its rule's 0 (`value_differs`, the one difference); a second line gives it 0 again,
/// its rule's value, and no difference is found.
#[test]
fn fixv_the_writer_writes_a_value_of_any_sign_back_and_finds_no_false_difference() {
    let (temp, root) = game_root(0);
    // A file of version 1 first, as `gate_477_cv1` (3) does: `converted` names the entries of a
    // version 1 file the line converts, and a root with no file has none to convert.
    let plain = write(&root, &[], None, false);
    assert_eq!(plain.status, "written");
    assert!(plain.converted.is_none());
    let converted = write(&root, &[], None, true);
    assert_eq!(converted.status, "written");
    assert_eq!(
        converted.converted,
        Some(vec![COLLECT.to_owned(), SPEND.to_owned()])
    );
    let written = read_json(&root.join(CATALOG));
    assert_eq!(
        written["packs"][0]["value"],
        json!({"milli": -2500, "origin": "model"})
    );
    assert_eq!(written["packs"][1]["value"], json!({"milli": 0}));
    assert!(
        converted.differences.is_empty(),
        "{:?}",
        converted.differences
    );

    let refreshed = write(&root, &[COLLECT, SPEND], None, false);
    assert_eq!(refreshed.status, "unchanged");
    let states = serde_json::to_value(&refreshed.refreshed).expect("refreshed");
    for (index, package_id) in [COLLECT, SPEND].iter().enumerate() {
        assert_eq!(states[index]["package_id"], *package_id, "{states}");
        assert_eq!(states[index]["state"], "agrees", "{states}");
    }
    assert_eq!(read_json(&root.join(CATALOG)), written);

    let other = values_file(
        temp.path(),
        json!([{"package_id": SPEND, "value": {"milli": -700}}]),
    );
    let answer = write(&root, &[], Some(other.as_path()), false);
    assert_eq!(answer.status, "written");
    assert_eq!(
        read_json(&root.join(CATALOG))["packs"][1]["value"],
        json!({"milli": -700})
    );
    assert_eq!(
        warned(
            &answer.warnings,
            SharedCode::StandardPackCatalogScheduleDiffers
        ),
        vec![
            json!({"game": GAME, "package_id": SPEND, "catalog_difference": "value_differs",
                    "task_id": SPEND})
        ]
    );

    let same = values_file(
        temp.path(),
        json!([{"package_id": SPEND, "value": {"milli": 0}}]),
    );
    let answer = write(&root, &[], Some(same.as_path()), false);
    assert_eq!(answer.status, "written");
    assert_eq!(
        read_json(&root.join(CATALOG))["packs"][1]["value"],
        json!({"milli": 0})
    );
    assert!(
        warned(
            &answer.warnings,
            SharedCode::StandardPackCatalogScheduleDiffers
        )
        .is_empty(),
        "{:?}",
        answer.warnings
    );
}

/// (2) A root whose [`SPEND`] writes −3 000 milli, its version 2 catalog written from its rules:
/// with the rules' business part taken from the catalog it composes the rules the file composes,
/// so `--cut` writes `schedule.json` (`written`), `requires` naming `catalog_business` after the two
/// words of the fixed value and no rule writing a business member; `standard-pack compose` of the
/// root cut over reads the negative value from the catalog and gives the digest the cut answered.
#[test]
fn fixv_a_negative_value_taken_from_the_catalog_composes_as_the_file_that_writes_it() {
    let (temp, root) = game_root(-3_000);
    assert_eq!(write(&root, &[], None, true).status, "written");
    assert_eq!(
        read_json(&root.join(CATALOG))["packs"][1]["value"],
        json!({"milli": -3000})
    );
    let answer = cut_standard_pack_catalog(StandardPackCatalogCutRequest {
        root: root.clone(),
        plan: false,
    })
    .unwrap_or_else(|error| panic!("the cut: {}: {}", error.code, error.message));
    assert_eq!(answer.status, "written");
    assert_eq!(answer.rules, [COLLECT, SPEND]);
    let digest = answer
        .rules_digest
        .clone()
        .expect("the digest of both compositions");
    let switched = read_json(&root.join("schedule.json"));
    assert_eq!(
        switched["requires"],
        json!([
            BENEFIT_FIXED_VALUE_SUBSYSTEM,
            FIXED_VALUE_ANY_SUBSYSTEM,
            CATALOG_BUSINESS_SUBSYSTEM
        ])
    );
    for written in switched["rules"].as_array().expect("the rules") {
        for member in BUSINESS {
            assert!(written.get(member).is_none(), "{member}: {written}");
        }
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
}
