// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 CV-1 (register §1.507; contracts/pack-catalog.md, "Version 2"; coordinator work
// order `rd-1122/brief-477-cv1.md`, its tests, and the coordinator's ruling 7 that replaced the
// work order's seventh, per-rule business parts): `pack-catalog.json` of version 2 carries a
// package's business part in full, as `schedule.json`'s rules write it today.
// - each form of an effect's quantity reads and is written back as it was read, and the bounds are
//   a rule's effect's, one case at each (1);
// - the fixed value and the two marks are taken, and each refusal of the work order's ruling 2 has
//   its word at its pointer (2);
// - `standard-pack catalog --schema-version 2` converts a version 1 file and `--refresh` writes the
//   rules' business part into the entry: the entry equals the rule (3);
// - rules of one package that differ get a per-rule part each, and the entry no own part (4);
// - `--values` replaces the value and the marks of the parts it names; a value for a package the
//   registry does not hold is refused as a display file's title for one is, and a line that writes
//   version 1 refuses the option (5); it names a rule's part as well as an entry's (6);
// - `standard-pack check` warns of a difference between a rule and the part that stands for it and
//   refuses nothing for it (7); a rule is compared with its own per-rule part where the entry has
//   one, and a per-rule part of no rule of the package is refused by the reader and left out by the
//   writer, as an entry of an unknown package is (8);
// - a root with a version 1 file is written and answered byte for byte as before (9).
// A made-up game `neutral`, server `test`; every package, pool, rule, key and text is a
// placeholder. No clock is read and nothing sleeps.

use crate::{
    StandardPackCatalogRequest, StandardPackCatalogResponse, StandardPackCheckRequest,
    check_standard_pack_root, write_standard_pack_catalog,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    CatalogScheduleFacts, CatalogValue, CatalogVersion, LabError, LabResult, ScheduleValueOrigin,
    StandardPackCatalog, StandardPackProblem, StandardPackRegistry, StandardPackRootIssue,
    parse_standard_pack_catalog_with, parse_standard_pack_schedule,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package whose rules produce [`POINTS`].
const COLLECT: &str = "neutral.collect";
/// A second rule of [`COLLECT`].
const COLLECT_B: &str = "neutral.collect.b";
/// The package whose rule consumes [`POINTS`].
const SPEND: &str = "neutral.spend";
const POINTS: &str = "pool.neutral.points";
const STOCK: &str = "pool.neutral.stock";
const GEMS: &str = "pool.neutral.gems";
/// A pool whose valuation `schedule.json` leaves pending.
const VALUED: &str = "pool.neutral.valued";
const CATALOG: &str = "pack-catalog.json";
const VERSION_2: &str = "actingcommand.pack-catalog.v2";
const VALUES: &str = "actingcommand.pack-catalog-values.v1";
/// The largest whole quantity of an effect, 2^53 - 1.
const MAX: u64 = 9_007_199_254_740_991;

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("the file")).expect("JSON")
}

/// One effect on `pool`: `direction` `produce` or `consume`, `amount` as written.
fn effect(pool: &str, direction: &str, amount: Value) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// One effect on `pool` whose quantity is `milli` thousandths expected a run.
fn expected(pool: &str, direction: &str, milli: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "expected_amount_milli": milli,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A consumption of [`STOCK`] read from a fact, with `fallback`.
fn read_from_fact(fallback: u64) -> Value {
    effect(
        STOCK,
        "consume",
        json!({"type": "fact", "fact_key": "resource.neutral.per_run", "fallback": fallback}),
    )
}

/// A `schedule.json` rule `id` running `package` on an hourly clock.
fn rule(id: &str, package: &str, consumes: Vec<Value>, produces: Vec<Value>) -> Value {
    json!({
        "id": id,
        "package_id": package,
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

/// [`COLLECT`]'s rule: it produces 2 of [`POINTS`] a run.
fn collect_rule() -> Value {
    rule(
        COLLECT,
        COLLECT,
        Vec::new(),
        vec![effect(POINTS, "produce", json!(2))],
    )
}

/// The second rule of [`COLLECT`]: it produces `amount` of [`POINTS`] a run.
fn collect_b_rule(amount: u64) -> Value {
    rule(
        COLLECT_B,
        COLLECT,
        Vec::new(),
        vec![effect(POINTS, "produce", json!(amount))],
    )
}

/// [`SPEND`]'s rule: it consumes 1 of [`POINTS`] a run.
fn spend_rule() -> Value {
    rule(
        SPEND,
        SPEND,
        vec![effect(POINTS, "consume", json!(1))],
        Vec::new(),
    )
}

/// A pool of constant capacity 1000 read from `resource.<the id without "pool.">`, without a
/// valuation.
fn pool(id: &str) -> Value {
    json!({
        "id": id,
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    })
}

/// A `schedule.json` of `rules`, with [`POINTS`] and [`STOCK`].
fn schedule(rules: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
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

fn registry() -> StandardPackRegistry {
    StandardPackRegistry::from_bundle_bytes(&serde_json::to_vec(&bundle()).expect("JSON"), GAME)
        .expect("the registry")
}

/// What the reader holds a catalog to of a `schedule.json` of `rules` and the pools
/// [`POINTS`], [`STOCK`], [`GEMS`], [`VALUED`] (its valuation pending) and `more`.
fn facts(rules: Vec<Value>, more: &[String]) -> CatalogScheduleFacts {
    let mut pools = vec![
        json!({"id": POINTS}),
        json!({"id": STOCK}),
        json!({"id": GEMS}),
        json!({"id": VALUED, "valuation": "pending"}),
    ];
    pools.extend(more.iter().map(|id| json!({"id": id})));
    let schedule = json!({"schema_version": "actingcommand.schedule.v1", "rules": rules,
                          "pools": pools});
    let read = parse_standard_pack_schedule(&serde_json::to_vec(&schedule).expect("JSON"))
        .expect("the schedule reads");
    CatalogScheduleFacts::of(Some(&read))
}

/// A version 2 catalog of [`GAME`] with `packs`.
fn version_2(packs: Value) -> Value {
    json!({"schema_version": VERSION_2, "game": GAME, "packs": packs})
}

/// An entry of `package_id` whose own part is `consumes` and `produces`.
fn entry(package_id: &str, consumes: Value, produces: Value) -> Value {
    json!({"package_id": package_id, "consumes": consumes, "produces": produces})
}

fn read(
    document: &Value,
    facts: &CatalogScheduleFacts,
) -> Result<StandardPackCatalog, Vec<StandardPackRootIssue>> {
    parse_standard_pack_catalog_with(
        &serde_json::to_vec(document).expect("JSON"),
        GAME,
        &registry(),
        Some(facts),
    )
}

/// The first error of the reader's refusal of `document`, by its word and its pointer.
fn first_error(document: &Value, facts: &CatalogScheduleFacts) -> (StandardPackProblem, String) {
    let issues = read(document, facts).expect_err("the catalog is refused");
    let first = issues.first().expect("an error");
    (first.problem, first.pointer.clone())
}

/// (1) Each quantity form reads, and the business part is written back as it was read; the bounds
/// are a rule's effect's.
#[test]
fn cv1_each_quantity_form_round_trips_and_the_bounds_are_a_rule_s() {
    let facts = facts(Vec::new(), &[]);
    let written = entry(
        COLLECT,
        json!([effect(POINTS, "consume", json!(3)), read_from_fact(2)]),
        json!([
            expected(GEMS, "produce", 400),
            effect(VALUED, "produce", json!("pending"))
        ]),
    );
    let catalog = read(&version_2(json!([written.clone()])), &facts).expect("each form reads");
    assert_eq!(catalog.version(), CatalogVersion::V2);
    let read_entry = catalog.entry(COLLECT).expect("the entry");
    let parts = read_entry.parts.as_ref().expect("a version 2 entry");
    assert!(parts.rules.is_empty());
    let business = json!({"consumes": written["consumes"], "produces": written["produces"]});
    assert_eq!(
        read_entry.business_document(),
        business,
        "written back as read"
    );
    assert_eq!(
        catalog
            .index()
            .producing
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec![GEMS.to_owned(), VALUED.to_owned()]
    );

    let reads = |consumes: Value, produces: Value| {
        read(
            &version_2(json!([entry(COLLECT, consumes, produces)])),
            &facts,
        )
        .is_ok()
    };
    let refused = |consumes: Value, produces: Value| {
        first_error(
            &version_2(json!([entry(COLLECT, consumes, produces)])),
            &facts,
        )
    };
    let out_of_limits =
        |pointer: &str| (StandardPackProblem::CatalogOutOfLimits, pointer.to_owned());
    let malformed = |pointer: &str| (StandardPackProblem::Malformed, pointer.to_owned());
    // A constant: 1 to 2^53 - 1.
    for amount in [1, MAX] {
        assert!(
            reads(json!([effect(POINTS, "consume", json!(amount))]), json!([])),
            "{amount}"
        );
    }
    for amount in [0, MAX + 1] {
        assert_eq!(
            refused(json!([effect(POINTS, "consume", json!(amount))]), json!([])),
            out_of_limits("/packs/0/consumes/0/amount"),
            "{amount}"
        );
    }
    // An expectation in thousandths: 0 to 2^53 - 1.
    for milli in [0, MAX] {
        assert!(
            reads(json!([]), json!([expected(GEMS, "produce", milli)])),
            "{milli}"
        );
    }
    assert_eq!(
        refused(json!([]), json!([expected(GEMS, "produce", MAX + 1)])),
        out_of_limits("/packs/0/produces/0/expected_amount_milli")
    );
    // A consumption read from a fact: its fallback 1 to 2^53 - 1, on `consumes` only.
    for fallback in [1, MAX] {
        assert!(
            reads(json!([read_from_fact(fallback)]), json!([])),
            "{fallback}"
        );
    }
    for fallback in [0, MAX + 1] {
        assert_eq!(
            refused(json!([read_from_fact(fallback)]), json!([])),
            out_of_limits("/packs/0/consumes/0/amount/fallback"),
            "{fallback}"
        );
    }
    let mut produced_from_fact = read_from_fact(2);
    produced_from_fact["direction"] = json!("produce");
    assert_eq!(
        refused(json!([]), json!([produced_from_fact])),
        malformed("/packs/0/produces/0/amount")
    );
    // The evidence: a confidence of 0 to 1000.
    let mut sure = effect(POINTS, "consume", json!(1));
    sure["confidence_milli"] = json!(1000);
    assert!(reads(json!([sure.clone()]), json!([])));
    sure["confidence_milli"] = json!(1001);
    assert_eq!(
        refused(json!([sure]), json!([])),
        out_of_limits("/packs/0/consumes/0/confidence_milli")
    );
    // The direction is the side's, one quantity, and no member an effect does not have.
    assert_eq!(
        refused(json!([]), json!([effect(GEMS, "consume", json!(1))])),
        malformed("/packs/0/produces/0/direction")
    );
    let mut both = effect(POINTS, "consume", json!(1));
    both["expected_amount_milli"] = json!(400);
    assert_eq!(
        refused(json!([both]), json!([])),
        malformed("/packs/0/consumes/0")
    );
    let mut scoped = effect(POINTS, "consume", json!(1));
    scoped["scope"] = json!({"kind": "game", "game_id": GAME});
    assert_eq!(
        refused(json!([scoped]), json!([])),
        malformed("/packs/0/consumes/0")
    );
    // A side holds up to 128 effects, a rule's.
    let many = (0..129)
        .map(|index| format!("pool.neutral.n{index}"))
        .collect::<Vec<_>>();
    let wide = self::facts(Vec::new(), &many);
    let side = |count: usize| {
        Value::Array(
            many.iter()
                .take(count)
                .map(|pool| effect(pool, "consume", json!(1)))
                .collect(),
        )
    };
    assert!(
        read(
            &version_2(json!([entry(COLLECT, side(128), json!([]))])),
            &wide
        )
        .is_ok()
    );
    assert_eq!(
        first_error(
            &version_2(json!([entry(COLLECT, side(129), json!([]))])),
            &wide
        ),
        out_of_limits("/packs/0/consumes")
    );
}

/// (2) The value and the marks: taken; each refusal of the work order's ruling 2 one case.
#[test]
fn cv1_the_value_and_the_marks_are_taken_and_each_refusal_has_its_word() {
    let facts = facts(Vec::new(), &[]);
    let with = |members: Value, produces: Value| {
        let mut written = entry(COLLECT, json!([]), produces);
        for (name, value) in members.as_object().expect("members") {
            written[name] = value.clone();
        }
        version_2(json!([written]))
    };
    let produce = |pool: &str| json!([effect(pool, "produce", json!(1))]);
    let catalog = read(
        &with(
            json!({"value": {"milli": MAX, "origin": "model"}, "limited": true}),
            produce(STOCK),
        ),
        &facts,
    )
    .expect("a value with its origin and the limited mark");
    let own = catalog
        .entry(COLLECT)
        .and_then(|read_entry| read_entry.parts.as_ref())
        .and_then(|parts| parts.own.as_ref())
        .expect("an own part");
    assert_eq!(
        own.value,
        Some(CatalogValue {
            milli: MAX,
            origin: ScheduleValueOrigin::Model
        })
    );
    assert!(own.limited && !own.realization);
    assert!(
        read(
            &with(json!({"value": {"milli": 1}}), produce(STOCK)),
            &facts
        )
        .is_ok()
    );
    assert!(
        read(
            &with(
                json!({"realization": true, "limited": true}),
                produce(VALUED)
            ),
            &facts
        )
        .is_ok()
    );

    let cases = [
        (
            json!({"value": {"milli": 0}}),
            produce(STOCK),
            StandardPackProblem::CatalogOutOfLimits,
            "/packs/0/value/milli",
        ),
        (
            json!({"value": {"milli": MAX + 1}}),
            produce(STOCK),
            StandardPackProblem::CatalogOutOfLimits,
            "/packs/0/value/milli",
        ),
        (
            json!({"value": {"milli": 1, "origin": "pack"}}),
            produce(STOCK),
            StandardPackProblem::Malformed,
            "/packs/0/value/origin",
        ),
        (
            json!({"limited": false}),
            produce(STOCK),
            StandardPackProblem::Malformed,
            "/packs/0/limited",
        ),
        (
            json!({"realization": "true"}),
            produce(STOCK),
            StandardPackProblem::Malformed,
            "/packs/0/realization",
        ),
        (
            json!({"value": {"milli": 1}, "realization": true}),
            produce(STOCK),
            StandardPackProblem::CatalogValueConflict,
            "/packs/0/value",
        ),
        (
            json!({"value": {"milli": 1}}),
            produce(VALUED),
            StandardPackProblem::CatalogValueConflict,
            "/packs/0/value",
        ),
    ];
    for (members, produces, problem, pointer) in cases {
        assert_eq!(
            first_error(&with(members.clone(), produces), &facts),
            (problem, pointer.to_owned()),
            "{members}"
        );
    }
}

/// A built game root of a working copy, `<temporary>/repo/neutral/`, with `bundle.json` and a
/// `schedule.json` of `rules` (the writer admits no package).
fn working_root(rules: Vec<Value>) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("repo").join(GAME);
    write_json(&root.join("bundle.json"), &bundle());
    write_json(&root.join("schedule.json"), &schedule(rules));
    (temp, root)
}

/// One line of the writer on `root`.
#[derive(Default)]
struct Line<'a> {
    refresh: &'a [&'a str],
    values: Option<&'a Path>,
    version_2: bool,
}

fn line(root: &Path, options: Line<'_>) -> LabResult<StandardPackCatalogResponse> {
    write_standard_pack_catalog(StandardPackCatalogRequest {
        root: root.to_owned(),
        plan: false,
        refresh: options
            .refresh
            .iter()
            .map(|package_id| (*package_id).to_owned())
            .collect(),
        displays: None,
        values: options.values.map(Path::to_path_buf),
        version_2: options.version_2,
    })
}

fn write(root: &Path, options: Line<'_>) -> StandardPackCatalogResponse {
    line(root, options)
        .unwrap_or_else(|error| panic!("the writer: {}: {}", error.code, error.message))
}

/// The refusal's outcome, as its details carry it.
fn refusal(error: &LabError) -> Value {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The values of each warning of `code` in `warnings`.
fn warned(warnings: &[Outcome], code: SharedCode) -> Vec<Value> {
    warnings
        .iter()
        .filter(|warning| warning.is(code))
        .map(|warning| json!(warning.values()))
        .collect()
}

/// [`COLLECT`]'s rule with its fixed value of the model's and the limited mark, and [`SPEND`]'s
/// with the realization mark.
fn valued_rules() -> Vec<Value> {
    let mut collect = collect_rule();
    collect["fixed_value_milli"] = json!(12000);
    collect["fixed_value_origin"] = json!("model");
    collect["limited"] = json!(true);
    let mut spend = spend_rule();
    spend["realization"] = json!(true);
    vec![collect, spend]
}

/// The business part [`valued_rules`] give each package, as version 2 writes it.
fn collect_part() -> Value {
    json!({
        "consumes": [],
        "produces": [effect(POINTS, "produce", json!(2))],
        "value": {"milli": 12000, "origin": "model"},
        "limited": true
    })
}

fn spend_part() -> Value {
    json!({
        "consumes": [effect(POINTS, "consume", json!(1))],
        "produces": [],
        "realization": true
    })
}

/// `part` as the entry of `package_id`.
fn entry_of(package_id: &str, part: &Value) -> Value {
    let mut written = json!({"package_id": package_id});
    for (name, value) in part.as_object().expect("a part") {
        written[name] = value.clone();
    }
    written
}

/// (3) `--schema-version 2` converts a version 1 file, each entry's business part its rules':
/// the quantities, the evidence, the fixed value with its origin and the marks; `--refresh` of a
/// package of a version 2 file writes the same; a second line changes nothing.
#[test]
fn cv1_version_2_and_refresh_write_the_rules_business_part_into_the_entry() {
    let (_temp, root) = working_root(valued_rules());
    let plain = write(&root, Line::default());
    assert_eq!(plain.status, "written");
    assert!(plain.converted.is_none() && plain.values_written.is_none());
    assert_eq!(
        read_json(&root.join(CATALOG))["schema_version"],
        "actingcommand.pack-catalog.v1"
    );

    let converted = write(
        &root,
        Line {
            version_2: true,
            ..Line::default()
        },
    );
    assert_eq!(converted.status, "written");
    assert_eq!(
        converted.converted,
        Some(vec![COLLECT.to_owned(), SPEND.to_owned()])
    );
    let expected = json!({
        "schema_version": VERSION_2,
        "game": GAME,
        "packs": [entry_of(COLLECT, &collect_part()), entry_of(SPEND, &spend_part())]
    });
    assert_eq!(
        read_json(&root.join(CATALOG)),
        expected,
        "each entry equals its rule"
    );
    assert!(
        converted.differences.is_empty(),
        "{:?}",
        converted.differences
    );

    let refreshed = write(
        &root,
        Line {
            refresh: &[COLLECT],
            ..Line::default()
        },
    );
    assert_eq!(refreshed.status, "unchanged");
    assert_eq!(
        serde_json::to_value(&refreshed.refreshed).expect("refreshed"),
        json!([{"package_id": COLLECT, "state": "agrees", "before": collect_part(),
                "after": collect_part()}])
    );
    let again = write(
        &root,
        Line {
            version_2: true,
            ..Line::default()
        },
    );
    assert_eq!(again.status, "unchanged");
    assert_eq!(again.converted, Some(Vec::new()));
    assert_eq!(read_json(&root.join(CATALOG)), expected);
}

/// The per-rule parts [`COLLECT`]'s two rules give when the second produces `amount`.
fn collect_rule_parts(amount: u64) -> Value {
    json!([
        {"rule_id": COLLECT, "consumes": [], "produces": [effect(POINTS, "produce", json!(2))]},
        {"rule_id": COLLECT_B, "consumes": [], "produces": [effect(POINTS, "produce", json!(amount))]}
    ])
}

/// (4) The coordinator's ruling 7, `--refresh`: two rules of one package that differ each get
/// their per-rule part, the entry no own part; each rule is then compared with its own part.
#[test]
fn cv1_rules_that_differ_get_a_part_each_and_the_entry_no_own_part() {
    let (_temp, root) = working_root(vec![collect_rule(), collect_b_rule(5), spend_rule()]);
    let answer = write(
        &root,
        Line {
            refresh: &[COLLECT],
            version_2: true,
            ..Line::default()
        },
    );
    assert_eq!(answer.status, "written");
    let written = read_json(&root.join(CATALOG));
    assert_eq!(
        written["packs"][0],
        json!({"package_id": COLLECT, "rules": collect_rule_parts(5)}),
        "no own part is guessed"
    );
    assert_eq!(
        serde_json::to_value(&answer.refreshed).expect("refreshed"),
        json!([{"package_id": COLLECT, "state": "added",
                "after": {"rules": collect_rule_parts(5)}}])
    );
    assert!(answer.differences.is_empty(), "{:?}", answer.differences);
}

/// A values file of `packs` beside `root`'s temporary directory.
fn values_file(directory: &Path, packs: Value) -> PathBuf {
    let path = directory.join("values").join("values.json");
    write_json(&path, &json!({"schema_version": VALUES, "packs": packs}));
    path
}

/// (5) `--values`: the value and marks land in the named entry, replaced whole; a value for a
/// package the registry does not hold is refused as a display file's title for one is,
/// `standard_pack_catalog_display_refused` with `package_not_listed`, nothing written; on a line
/// that writes version 1 the option is a usage error.
#[test]
fn cv1_values_land_in_the_named_entry_and_one_of_an_unknown_package_is_refused() {
    let (temp, root) = working_root(valued_rules());
    write(
        &root,
        Line {
            version_2: true,
            ..Line::default()
        },
    );
    let path = values_file(
        temp.path(),
        json!([{"package_id": SPEND, "value": {"milli": 300}, "limited": true}]),
    );
    let answer = write(
        &root,
        Line {
            values: Some(path.as_path()),
            ..Line::default()
        },
    );
    assert_eq!(answer.status, "written");
    assert_eq!(
        serde_json::to_value(&answer.values_written).expect("values_written"),
        json!([{"package_id": SPEND}])
    );
    let written = read_json(&root.join(CATALOG));
    assert_eq!(
        written["packs"][1],
        json!({"package_id": SPEND, "consumes": [effect(POINTS, "consume", json!(1))],
               "produces": [], "value": {"milli": 300}, "limited": true}),
        "the value and the marks replaced whole, the realization mark with them"
    );
    assert_eq!(written["packs"][0], entry_of(COLLECT, &collect_part()));
    let differs = warned(
        &answer.warnings,
        SharedCode::StandardPackCatalogScheduleDiffers,
    );
    let words = ["value_differs", "limited_differs", "realization_differs"];
    assert_eq!(
        differs,
        words
            .iter()
            .map(
                |word| json!({"game": GAME, "package_id": SPEND, "catalog_difference": word,
                               "task_id": SPEND})
            )
            .collect::<Vec<_>>(),
        "the rule's differences, no pool and no side"
    );

    let before = fs::read(root.join(CATALOG)).expect("the file");
    let unknown = values_file(
        temp.path(),
        json!([{"package_id": "neutral.unknown", "value": {"milli": 1}}]),
    );
    let error = line(
        &root,
        Line {
            values: Some(unknown.as_path()),
            ..Line::default()
        },
    )
    .expect_err("a value for an unknown package is refused");
    assert_eq!(error.code, "standard_pack_catalog_display_refused");
    let outcome = refusal(&error);
    assert_eq!(
        outcome["values"]["standard_pack_problem"],
        "package_not_listed"
    );
    assert_eq!(outcome["values"]["json_pointer"], "/packs/0/package_id");
    assert_eq!(
        outcome["values"]["path"],
        json!(unknown.display().to_string())
    );
    assert_eq!(fs::read(root.join(CATALOG)).expect("the file"), before);

    let (other_temp, version_1) = working_root(valued_rules());
    write(&version_1, Line::default());
    let file = values_file(other_temp.path(), json!([{"package_id": SPEND}]));
    let error = line(
        &version_1,
        Line {
            values: Some(file.as_path()),
            ..Line::default()
        },
    )
    .expect_err("version 1 has no value");
    assert_eq!(error.code, "validation_failed");
}

/// (6) The coordinator's ruling 7, `--values`: an item names a rule's part as well as an entry's;
/// a rule whose part the entry's own part stands for gets its own part, a copy with the item's
/// value and marks; an item naming an own part the entry does not write is `catalog_part_absent`,
/// and one naming a rule that does not run the package `catalog_rule_unknown`.
#[test]
fn cv1_values_name_a_rule_s_part_as_well_as_an_entry_s() {
    let (temp, root) = working_root(vec![collect_rule(), collect_b_rule(5), spend_rule()]);
    write(
        &root,
        Line {
            version_2: true,
            ..Line::default()
        },
    );
    let path = values_file(
        temp.path(),
        json!([
            {"package_id": COLLECT, "rule_id": COLLECT_B, "value": {"milli": 7000}},
            {"package_id": SPEND, "rule_id": SPEND, "limited": true}
        ]),
    );
    let answer = write(
        &root,
        Line {
            values: Some(path.as_path()),
            ..Line::default()
        },
    );
    assert_eq!(
        serde_json::to_value(&answer.values_written).expect("values_written"),
        json!([{"package_id": COLLECT, "rule_id": COLLECT_B}, {"package_id": SPEND, "rule_id": SPEND}])
    );
    let written = read_json(&root.join(CATALOG));
    let mut parts = collect_rule_parts(5);
    parts[1]["value"] = json!({"milli": 7000});
    assert_eq!(
        written["packs"][0],
        json!({"package_id": COLLECT, "rules": parts})
    );
    assert_eq!(
        written["packs"][1],
        json!({
            "package_id": SPEND,
            "consumes": [effect(POINTS, "consume", json!(1))],
            "produces": [],
            "rules": [{"rule_id": SPEND, "consumes": [effect(POINTS, "consume", json!(1))],
                       "produces": [], "limited": true}]
        })
    );

    let before = fs::read(root.join(CATALOG)).expect("the file");
    for (item, problem, pointer) in [
        (
            json!({"package_id": COLLECT, "value": {"milli": 1}}),
            "catalog_part_absent",
            "/packs/0",
        ),
        (
            json!({"package_id": COLLECT, "rule_id": "neutral.collect.c", "limited": true}),
            "catalog_rule_unknown",
            "/packs/0/rule_id",
        ),
    ] {
        let file = values_file(temp.path(), json!([item]));
        let error = line(
            &root,
            Line {
                values: Some(file.as_path()),
                ..Line::default()
            },
        )
        .expect_err("refused");
        assert_eq!(error.code, "standard_pack_catalog_display_refused");
        let outcome = refusal(&error);
        assert_eq!(outcome["values"]["standard_pack_problem"], problem);
        assert_eq!(outcome["values"]["json_pointer"], pointer);
        assert_eq!(fs::read(root.join(CATALOG)).expect("the file"), before);
    }
}

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
        "goal": "CV-1 fixture",
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

/// A game root `<temporary>/packages/neutral/` that `standard-pack check` reads: `bundle.json`,
/// both packages in their directories, `schedule.json` of `rules` and `catalog` as its
/// `pack-catalog.json`.
fn checked_root(rules: Vec<Value>, catalog: &Value) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join("packages").join(GAME);
    for package_id in [COLLECT, SPEND] {
        write_pack(&game_root.join(package_id), package_id);
    }
    write_json(&game_root.join("bundle.json"), &bundle());
    write_json(&game_root.join("schedule.json"), &schedule(rules));
    write_json(&game_root.join(CATALOG), catalog);
    (temp, game_root)
}

fn check(game_root: &Path) -> LabResult<crate::StandardPackCheckResponse> {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_owned(),
    })
}

/// (7) `standard-pack check` reads version 2 and warns of a difference between a rule and the
/// part that stands for it (another quantity of a pool both name), refusing nothing for it.
#[test]
fn cv1_check_warns_of_a_difference_from_a_rule_and_refuses_nothing_for_it() {
    let catalog = version_2(json!([
        entry(
            COLLECT,
            json!([]),
            json!([effect(POINTS, "produce", json!(3))])
        ),
        entry(
            SPEND,
            json!([effect(POINTS, "consume", json!(1))]),
            json!([])
        )
    ]));
    let (_temp, root) = checked_root(vec![collect_rule(), spend_rule()], &catalog);
    let answer = check(&root)
        .unwrap_or_else(|error| panic!("the root is checked: {}: {}", error.code, error.message));
    assert_eq!(
        warned(
            &answer.warnings,
            SharedCode::StandardPackCatalogScheduleDiffers
        ),
        vec![json!({
            "game": GAME, "package_id": COLLECT, "pool_id": POINTS, "catalog_side": "produces",
            "catalog_difference": "quantity_differs", "task_id": COLLECT
        })]
    );
}

/// (8) The coordinator's ruling 7, `check`: a rule is compared with its own per-rule part where
/// the entry has one, else with the entry's own part; a per-rule part of no rule of the root that
/// runs the package is refused by the reader (`catalog_rule_unknown`, as an entry of a package the
/// registry does not hold is `package_not_listed`), and the writer leaves it out and warns of it.
#[test]
fn cv1_a_rule_is_compared_with_its_own_part_and_a_part_of_no_rule_is_refused_and_left_out() {
    let rules = vec![collect_rule(), collect_b_rule(5), spend_rule()];
    let collect = |parts: Value| {
        json!({"package_id": COLLECT, "consumes": [],
               "produces": [effect(POINTS, "produce", json!(2))], "rules": parts})
    };
    let spend = entry(
        SPEND,
        json!([effect(POINTS, "consume", json!(1))]),
        json!([]),
    );
    let b_part = |amount: u64| {
        json!({"rule_id": COLLECT_B, "consumes": [],
               "produces": [effect(POINTS, "produce", json!(amount))]})
    };
    let catalog = version_2(json!([collect(json!([b_part(4)])), spend.clone()]));
    let (_temp, root) = checked_root(rules, &catalog);
    let answer = check(&root)
        .unwrap_or_else(|error| panic!("the root is checked: {}: {}", error.code, error.message));
    assert_eq!(
        warned(
            &answer.warnings,
            SharedCode::StandardPackCatalogScheduleDiffers
        ),
        vec![json!({
            "game": GAME, "package_id": COLLECT, "pool_id": POINTS, "catalog_side": "produces",
            "catalog_difference": "quantity_differs", "task_id": COLLECT_B
        })],
        "the first rule agrees with the own part, the second differs from its own"
    );

    let unknown = json!({"rule_id": "neutral.collect.c", "consumes": [], "produces": []});
    let catalog = version_2(json!([collect(json!([b_part(5), unknown])), spend]));
    write_json(&root.join(CATALOG), &catalog);
    let error = check(&root).expect_err("a part of no rule is refused");
    assert_eq!(error.code, "standard_pack_root_invalid");
    let outcome = refusal(&error);
    assert_eq!(
        outcome["values"]["standard_pack_problem"],
        "catalog_rule_unknown"
    );
    assert_eq!(
        outcome["values"]["json_pointer"],
        "/packs/0/rules/1/rule_id"
    );

    let answer = write(&root, Line::default());
    assert_eq!(answer.status, "written");
    assert_eq!(
        read_json(&root.join(CATALOG))["packs"][0],
        collect(json!([b_part(5)])),
        "the part of no rule is left out"
    );
    let written_path = std::path::absolute(root.join(CATALOG)).expect("an absolute path");
    assert_eq!(
        warned(&answer.warnings, SharedCode::StandardPackCatalogLeftOut),
        vec![json!({
            "game": GAME,
            "path": written_path.display().to_string(),
            "standard_pack_problem": "catalog_rule_unknown",
            "json_pointer": "/packs/0/rules/1/rule_id",
            "package_id": COLLECT,
            "task_id": "neutral.collect.c"
        })]
    );
}

/// The golden of a version 1 file, written by the writer of 0.12.78 from the root of
/// [`cv1_a_version_1_root_is_written_and_answered_byte_for_byte_as_before`]: the kept entry as
/// written, the refreshed one its rule's, two-space indented, a line feed at the end.
const VERSION_1_GOLDEN: &str = r#"{
  "schema_version": "actingcommand.pack-catalog.v1",
  "game": "neutral",
  "groups": [
    {
      "id": "daily",
      "title": {
        "en": "Daily"
      }
    }
  ],
  "packs": [
    {
      "package_id": "neutral.collect",
      "title": {
        "en": "Fixture"
      },
      "summary": {
        "en": "A fixture package."
      },
      "group": "daily",
      "consumes": [],
      "produces": [
        {
          "pool_id": "pool.neutral.stock"
        }
      ]
    },
    {
      "package_id": "neutral.spend",
      "consumes": [
        {
          "pool_id": "pool.neutral.points",
          "quantity": {
            "kind": "constant",
            "amount": 1
          }
        }
      ],
      "produces": []
    }
  ]
}
"#;

/// (9) The work order's ruling 6: a root with a version 1 file is written and answered byte for
/// byte as before, PC-1's fixture (its rules, its kept entry with a display and a pool its rules do
/// not name) with PC-2's `--refresh` of a package without an entry: the file's bytes, the
/// differences, the refreshed package and the business part a reader (`packs`) carries.
#[test]
fn cv1_a_version_1_root_is_written_and_answered_byte_for_byte_as_before() {
    let (_temp, root) = working_root(vec![collect_rule(), spend_rule()]);
    write_json(
        &root.join(CATALOG),
        &json!({
            "schema_version": "actingcommand.pack-catalog.v1",
            "game": GAME,
            "groups": [{"id": "daily", "title": {"en": "Daily"}}],
            "packs": [{
                "package_id": COLLECT,
                "title": {"en": "Fixture"},
                "summary": {"en": "A fixture package."},
                "group": "daily",
                "consumes": [],
                "produces": [{"pool_id": STOCK}]
            }]
        }),
    );
    let answer = write(
        &root,
        Line {
            refresh: &[SPEND],
            ..Line::default()
        },
    );
    assert_eq!(answer.status, "written");
    let bytes = fs::read(root.join(CATALOG)).expect("the file");
    assert_eq!(
        String::from_utf8(bytes.clone()).expect("UTF-8"),
        VERSION_1_GOLDEN
    );
    assert_eq!(
        serde_json::to_string(&answer.differences).expect("differences"),
        r#"[{"package_id":"neutral.collect","pool_id":"pool.neutral.points","side":"produces","difference":"catalog_lacks","rule_id":"neutral.collect"},{"package_id":"neutral.collect","pool_id":"pool.neutral.stock","side":"produces","difference":"schedule_lacks"}]"#
    );
    assert_eq!(
        serde_json::to_string(&answer.refreshed).expect("refreshed"),
        r#"[{"package_id":"neutral.spend","state":"added","after":{"consumes":[{"pool_id":"pool.neutral.points","quantity":{"kind":"constant","amount":1}}],"produces":[]}}]"#
    );
    assert!(answer.converted.is_none() && answer.values_written.is_none());
    let facts = CatalogScheduleFacts::of(Some(
        &parse_standard_pack_schedule(
            &serde_json::to_vec(&schedule(vec![collect_rule(), spend_rule()])).expect("JSON"),
        )
        .expect("the schedule"),
    ));
    let catalog = parse_standard_pack_catalog_with(&bytes, GAME, &registry(), Some(&facts))
        .expect("the golden reads");
    assert_eq!(catalog.version(), CatalogVersion::V1);
    assert_eq!(
        serde_json::to_string(
            &catalog
                .entry(SPEND)
                .expect("the refreshed entry")
                .business_document()
        )
        .expect("the business part"),
        r#"{"consumes":[{"pool_id":"pool.neutral.points","quantity":{"kind":"constant","amount":1}}],"produces":[]}"#
    );
}
