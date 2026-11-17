// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 PC-2 (register §1.462; contracts/pack-catalog.md, "Who writes it"; coordinator
// work order `rd-1117/brief-477-pc2-catalog-refresh.md`, its six tests): `standard-pack catalog`
// brings a named entry's business part up to date from the rules (`refresh`) and writes a named
// entry's display (`displays`).
// - the order agents use: an entry written while its package has no rule is empty; once the rule is
//   written a plain line changes nothing and warns `standard_pack_catalog_schedule_differs`; a line
//   with `refresh` writes the rule's pools into the entry, keeps its display and its place, and the
//   warning names the package no more (1);
// - an entry not named is untouched in the same line, though it differs from its rules (2);
// - `--plan` writes nothing and answers `refreshed` with the business part before and after (3);
// - `refresh` naming a package the registry does not hold, or one twice, is refused
//   `standard_pack_catalog_refresh_refused` with its word and nothing is written; a named package
//   without an entry is no refusal: it gets its new entry, answered `added` (4);
// - a display file replaces a named entry's display whole, its business part and its place
//   untouched; one whose display breaks a limit, or names a group the file does not declare, is
//   refused `standard_pack_catalog_display_refused` with the catalog's own word at its pointer in
//   the display file, and nothing is written (5);
// - a second identical line changes nothing (`unchanged`, the refreshed entry `agrees`) (6).
// A made-up game `neutral` in temporary directories; every package, pool, rule and text is a
// placeholder. No clock is read and nothing sleeps.

use crate::{StandardPackCatalogRequest, StandardPackCatalogResponse, write_standard_pack_catalog};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{LabError, LabResult};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
/// The package whose rule is written after its entry; it produces [`POINTS`].
const COLLECT: &str = "neutral.collect";
/// The package whose rule consumes [`POINTS`] from the start.
const SPEND: &str = "neutral.spend";
const POINTS: &str = "pool.neutral.points";
const STOCK: &str = "pool.neutral.stock";
const CATALOG: &str = "pack-catalog.json";

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("the file")).expect("JSON")
}

/// The bundle index of [`COLLECT`] and [`SPEND`].
fn bundle() -> Value {
    let packs = [COLLECT, SPEND]
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id, "server": "test", "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": packs})
}

/// The rule of [`COLLECT`]: it produces `amount` of [`POINTS`] a run.
fn collect_rule(amount: u64) -> Value {
    json!({"id": "rule.collect", "package_id": COLLECT,
           "produces": [{"pool_id": POINTS, "amount": amount}]})
}

/// The rule of [`SPEND`]: it consumes one of [`POINTS`] a run.
fn spend_rule() -> Value {
    json!({"id": "rule.spend", "package_id": SPEND,
           "consumes": [{"pool_id": POINTS, "amount": 1}]})
}

/// `schedule.json` with `rules`, both pools declared.
fn schedule(rules: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": rules,
        "pools": [{"id": POINTS}, {"id": STOCK}]
    })
}

/// A built game root of a working copy, `<temporary>/repo/neutral/`, with `bundle.json` and a
/// `schedule.json` of `rules`.
fn working_root(rules: Vec<Value>) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("repo").join(GAME);
    write_json(&root.join("bundle.json"), &bundle());
    write_json(&root.join("schedule.json"), &schedule(rules));
    (temp, root)
}

/// A leftover `ui.json` with the group `daily` and a display for [`COLLECT`] in it.
fn ui() -> Value {
    json!({
        "schema_version": "actingcommand.ui.v1",
        "game": GAME,
        "groups": [{"id": "daily", "title": {"en": "Daily"}}],
        "packs": [{"package_id": COLLECT, "title": {"en": "Collect"},
                   "summary": {"en": "Collects points."}, "group": "daily"}]
    })
}

/// One line of the writer on `root`.
fn line(
    root: &Path,
    plan: bool,
    refresh: &[&str],
    displays: Option<&Path>,
) -> LabResult<StandardPackCatalogResponse> {
    write_standard_pack_catalog(StandardPackCatalogRequest {
        root: root.to_owned(),
        plan,
        refresh: refresh
            .iter()
            .map(|package_id| (*package_id).to_owned())
            .collect(),
        displays: displays.map(Path::to_path_buf),
    })
}

fn write(
    root: &Path,
    plan: bool,
    refresh: &[&str],
    displays: Option<&Path>,
) -> StandardPackCatalogResponse {
    line(root, plan, refresh, displays)
        .unwrap_or_else(|error| panic!("the writer: {}: {}", error.code, error.message))
}

/// The values of each `standard_pack_catalog_schedule_differs` of `warnings` naming `package_id`.
fn differs(warnings: &[Outcome], package_id: &str) -> Vec<Value> {
    warnings
        .iter()
        .filter(|warning| warning.is(SharedCode::StandardPackCatalogScheduleDiffers))
        .filter(|warning| warning.value("package_id") == Some(&json!(package_id)))
        .map(|warning| json!(warning.values()))
        .collect()
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

fn refreshed(answer: &StandardPackCatalogResponse) -> Value {
    serde_json::to_value(&answer.refreshed).expect("refreshed")
}

/// A constant quantity of `amount` of `pool`, as the file writes it.
fn constant(pool: &str, amount: u64) -> Value {
    json!({"pool_id": pool, "quantity": {"kind": "constant", "amount": amount}})
}

/// (1) The order agents use: adopt held (no rule yet), try, then write the rule.
#[test]
fn pc2_refresh_writes_the_rule_into_an_entry_written_before_it_and_keeps_its_display() {
    let (_temp, root) = working_root(vec![spend_rule()]);
    write_json(&root.join("ui.json"), &ui());
    let first = write(&root, false, &[], None);
    assert_eq!(first.status, "written");
    let before = read_json(&root.join(CATALOG));
    let empty = json!({
        "package_id": COLLECT, "title": {"en": "Collect"}, "summary": {"en": "Collects points."},
        "group": "daily", "consumes": [], "produces": []
    });
    assert_eq!(
        before["packs"][0], empty,
        "an entry written while its package has no rule is empty"
    );

    write_json(
        &root.join("schedule.json"),
        &schedule(vec![collect_rule(2), spend_rule()]),
    );
    let bytes = fs::read(root.join(CATALOG)).expect("the file");
    let plain = write(&root, false, &[], None);
    assert_eq!(plain.status, "unchanged");
    assert!(!plain.changed);
    assert!(plain.refreshed.is_none() && plain.display_written.is_none());
    assert_eq!(fs::read(root.join(CATALOG)).expect("the file"), bytes);
    assert_eq!(
        differs(&plain.warnings, COLLECT),
        vec![json!({
            "game": GAME, "package_id": COLLECT, "pool_id": POINTS, "catalog_side": "produces",
            "catalog_difference": "catalog_lacks", "task_id": "rule.collect"
        })],
        "a plain line keeps the entry and warns"
    );

    let answer = write(&root, false, &[COLLECT], None);
    assert_eq!(answer.status, "written");
    assert!(answer.changed);
    assert_eq!(answer.kept, vec![COLLECT.to_owned(), SPEND.to_owned()]);
    assert!(answer.added.is_empty());
    assert_eq!(
        refreshed(&answer),
        json!([{
            "package_id": COLLECT, "state": "replaced",
            "before": {"consumes": [], "produces": []},
            "after": {"consumes": [], "produces": [constant(POINTS, 2)]}
        }])
    );
    let after = read_json(&root.join(CATALOG));
    let mut expected = empty;
    expected["produces"] = json!([constant(POINTS, 2)]);
    assert_eq!(
        after["packs"][0], expected,
        "the rule's pools are written, the display is kept, the entry stays first"
    );
    assert_eq!(after["packs"][1], before["packs"][1]);
    assert_eq!(after["groups"], before["groups"]);
    assert_eq!(
        differs(&answer.warnings, COLLECT),
        Vec::<Value>::new(),
        "the warning names the refreshed package no more"
    );
    assert!(
        answer
            .differences
            .iter()
            .all(|difference| difference.package_id != COLLECT)
    );
}

/// (2) An entry no `refresh` names is not overwritten, though it differs from its rules.
#[test]
fn pc2_an_entry_not_named_is_untouched_though_it_differs_from_its_rules() {
    let (_temp, root) = working_root(vec![collect_rule(2), spend_rule()]);
    write(&root, false, &[], None);
    let before = read_json(&root.join(CATALOG));
    let mut spend = spend_rule();
    spend["produces"] = json!([{"pool_id": STOCK, "amount": 1}]);
    write_json(
        &root.join("schedule.json"),
        &schedule(vec![collect_rule(3), spend]),
    );

    let answer = write(&root, false, &[COLLECT], None);

    assert_eq!(answer.status, "written");
    let after = read_json(&root.join(CATALOG));
    assert_eq!(
        after["packs"][1], before["packs"][1],
        "the entry not named is as written"
    );
    assert_eq!(
        after["packs"][0]["produces"],
        json!([constant(POINTS, 3)]),
        "the named entry holds its rule's amount"
    );
    assert_eq!(
        refreshed(&answer)[0]["before"]["produces"],
        json!([constant(POINTS, 2)])
    );
    assert_eq!(
        differs(&answer.warnings, SPEND),
        vec![json!({
            "game": GAME, "package_id": SPEND, "pool_id": STOCK, "catalog_side": "produces",
            "catalog_difference": "catalog_lacks", "task_id": "rule.spend"
        })],
        "its difference is still answered and warned of"
    );
}

/// (3) `--plan` writes nothing and answers `refreshed` with before and after.
#[test]
fn pc2_plan_writes_nothing_and_answers_refreshed_before_and_after() {
    let (_temp, root) = working_root(vec![collect_rule(2), spend_rule()]);
    write_json(
        &root.join(CATALOG),
        &json!({
            "schema_version": "actingcommand.pack-catalog.v1",
            "game": GAME,
            "packs": [{"package_id": COLLECT, "consumes": [], "produces": [{"pool_id": STOCK}]}]
        }),
    );
    let bytes = fs::read(root.join(CATALOG)).expect("the file");

    let planned = write(&root, true, &[COLLECT], None);

    assert_eq!(planned.status, "planned");
    assert!(planned.changed);
    assert_eq!(
        fs::read(root.join(CATALOG)).expect("the file"),
        bytes,
        "a plan writes nothing"
    );
    assert_eq!(
        refreshed(&planned),
        json!([{
            "package_id": COLLECT, "state": "replaced",
            "before": {"consumes": [], "produces": [{"pool_id": STOCK}]},
            "after": {"consumes": [], "produces": [constant(POINTS, 2)]}
        }])
    );
    let document = planned.document.expect("a plan answers its document");
    assert_eq!(
        document["packs"][0],
        json!({"package_id": COLLECT, "consumes": [], "produces": [constant(POINTS, 2)]})
    );
    assert_eq!(planned.added, vec![SPEND.to_owned()]);
}

/// (4) Each refusal of `refresh`, and the named package without an entry, which is none.
#[test]
fn pc2_refresh_refuses_an_unregistered_or_repeated_package_and_writes_nothing() {
    let (_temp, root) = working_root(vec![collect_rule(2), spend_rule()]);
    write(&root, false, &[], None);
    let bytes = fs::read(root.join(CATALOG)).expect("the file");
    let refused = |refresh: &[&str], package_id: &str, word: &str| {
        let error = line(&root, false, refresh, None).expect_err("the line is refused");
        assert_eq!(
            error.code, "standard_pack_catalog_refresh_refused",
            "{word}"
        );
        let outcome = refusal(&error);
        assert_eq!(outcome["values"]["game"], GAME, "{word}");
        assert_eq!(outcome["values"]["package_id"], package_id, "{word}");
        assert_eq!(outcome["values"]["catalog_refresh_problem"], word);
        assert_eq!(
            fs::read(root.join(CATALOG)).expect("the file"),
            bytes,
            "{word}: nothing is written"
        );
    };
    refused(
        &["neutral.absent"],
        "neutral.absent",
        "package_unregistered",
    );
    refused(&[COLLECT, COLLECT], COLLECT, "package_repeated");

    let (_other, fresh) = working_root(vec![collect_rule(2), spend_rule()]);
    let answer = write(&fresh, false, &[SPEND], None);
    assert_eq!(answer.status, "written");
    assert_eq!(answer.added, vec![COLLECT.to_owned(), SPEND.to_owned()]);
    assert_eq!(
        refreshed(&answer),
        json!([{
            "package_id": SPEND, "state": "added",
            "after": {"consumes": [constant(POINTS, 1)], "produces": []}
        }]),
        "a named package without an entry gets its new entry"
    );
}

/// (5) A display file writes the named entries' displays; a defect is refused with the catalog's
/// own word, nothing written.
#[test]
fn pc2_displays_replace_a_named_entry_s_display_and_a_defect_is_refused_with_its_word() {
    let (temp, root) = working_root(vec![collect_rule(2), spend_rule()]);
    write_json(&root.join("ui.json"), &ui());
    write(&root, false, &[], None);
    let before = read_json(&root.join(CATALOG));
    let displays = temp.path().join("displays.json");
    write_json(
        &displays,
        &json!({"packs": [
            {"package_id": COLLECT, "title": {"en": "Collect points"},
             "summary": {"en": "Collects points every hour."}, "risk": ["word_one"]},
            {"package_id": SPEND, "title": {"en": "Spend"}, "summary": {"en": "Spends points."},
             "group": "daily"}
        ]}),
    );

    let answer = write(&root, false, &[], Some(&displays));

    assert_eq!(answer.status, "written");
    assert_eq!(
        answer.display_written,
        Some(vec![COLLECT.to_owned(), SPEND.to_owned()])
    );
    assert!(answer.refreshed.is_none());
    let after = read_json(&root.join(CATALOG));
    assert_eq!(
        after["packs"][0],
        json!({
            "package_id": COLLECT, "title": {"en": "Collect points"},
            "summary": {"en": "Collects points every hour."}, "risk": ["word_one"],
            "consumes": [], "produces": [constant(POINTS, 2)]
        }),
        "the display is replaced whole (its group gone), the business part untouched"
    );
    assert_eq!(
        after["packs"][1],
        json!({
            "package_id": SPEND, "title": {"en": "Spend"}, "summary": {"en": "Spends points."},
            "group": "daily", "consumes": [constant(POINTS, 1)], "produces": []
        })
    );
    for index in 0..2 {
        for side in ["consumes", "produces"] {
            assert_eq!(after["packs"][index][side], before["packs"][index][side]);
        }
    }
    let bytes = fs::read(root.join(CATALOG)).expect("the file");

    for (field, value, pointer, word) in [
        (
            "title",
            json!({"en": "x".repeat(129)}),
            "/packs/0/title/en",
            "catalog_out_of_limits",
        ),
        (
            "group",
            json!("weekly"),
            "/packs/0/group",
            "catalog_group_unknown",
        ),
    ] {
        let mut defective = json!({"packs": [
            {"package_id": COLLECT, "title": {"en": "Collect"}, "summary": {"en": "Collects."}}
        ]});
        defective["packs"][0][field] = value;
        write_json(&displays, &defective);
        let error = line(&root, false, &[], Some(&displays)).expect_err("the line is refused");
        assert_eq!(
            error.code, "standard_pack_catalog_display_refused",
            "{word}"
        );
        let outcome = refusal(&error);
        assert_eq!(outcome["values"]["standard_pack_problem"], word);
        assert_eq!(outcome["values"]["json_pointer"], pointer, "{word}");
        assert_eq!(
            outcome["values"]["path"],
            json!(displays.display().to_string()),
            "{word}"
        );
        assert_eq!(
            fs::read(root.join(CATALOG)).expect("the file"),
            bytes,
            "{word}: nothing is written"
        );
    }
}

/// (6) A second identical line changes nothing.
#[test]
fn pc2_a_second_identical_line_changes_nothing() {
    let (temp, root) = working_root(vec![spend_rule()]);
    write(&root, false, &[], None);
    write_json(
        &root.join("schedule.json"),
        &schedule(vec![collect_rule(2), spend_rule()]),
    );
    let displays = temp.path().join("displays.json");
    write_json(
        &displays,
        &json!({"packs": [{"package_id": COLLECT, "title": {"en": "Collect"},
                           "summary": {"en": "Collects points."}}]}),
    );

    let first = write(&root, false, &[COLLECT], Some(&displays));
    assert_eq!(first.status, "written");
    assert_eq!(refreshed(&first)[0]["state"], "replaced");
    let bytes = fs::read(root.join(CATALOG)).expect("the file");

    let second = write(&root, false, &[COLLECT], Some(&displays));

    assert_eq!(second.status, "unchanged");
    assert!(!second.changed);
    assert_eq!(
        fs::read(root.join(CATALOG)).expect("the file"),
        bytes,
        "nothing is written"
    );
    let after = json!({"consumes": [], "produces": [constant(POINTS, 2)]});
    assert_eq!(
        refreshed(&second),
        json!([{"package_id": COLLECT, "state": "agrees", "before": after, "after": after}])
    );
    assert_eq!(second.display_written, Some(vec![COLLECT.to_owned()]));
}
