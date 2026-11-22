// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 CV-2b (register §1.515; contracts/standard-pack-authoring.md, "On a switched root";
// coordinator work order `rd-1122/brief-477-cv2b.md`, its rulings 1 to 7 and its tests):
// `standard-pack adopt` on a checkout whose `schedule.json` names `catalog_business` writes the
// adopted rule's business part into `pack-catalog.json` instead of into the rule.
// - ruling 2, the proof: adopting into a root that is not switched, bringing the package's entry up
//   to its rules (`standard-pack catalog --refresh`, the cut-over's first step) and cutting the
//   root over (`--cut`) gives, byte for byte, the `schedule.json`, the `pack-catalog.json`, every
//   other file and the composition that adopting into the same root cut over first gives: a first
//   adoption (1), a re-adoption that changes the value (2), and a re-adoption of the first of a
//   package's two rules whose entry held one own part (3; adopt never adds a second rule of a
//   package, it takes the package's first rule, so this is the case of ruling 1's second rule);
// - ruling 3: an adoption whose catalog would not read writes neither file, and the catalog is
//   renamed before `schedule.json`, whose `.part` an interrupted call leaves (4);
// - ruling 4: a root that is not switched, holding a catalog of version 2, is adopted byte for
//   byte as before: the golden of the first adoption of #464 P1's fixture (5);
// - ruling 5: a re-adoption that names no business member leaves the catalog as it is (6);
// - a switched root whose catalog is missing or of version 1 is refused with its word (7).
// A made-up game `neutral`, server `test`; every package, pool and rule is a placeholder. No clock
// is read and nothing sleeps.

use crate::codes::AdoptProblem;
use crate::gate_464_p1::NoLab;
use crate::{
    AdoptDecision, AdoptLab, AdoptSource, AdoptWorktree, PackageBundleRequest, ResourceToolingCode,
    StandardPackAdoptRequest, StandardPackAdoptResponse, StandardPackCatalogCutRequest,
    StandardPackCatalogRequest, StandardPackComposeRequest, StandardPackRootFiles,
    compose_standard_packs, cut_standard_pack_catalog, package_bundle, standard_pack_adopt,
    write_standard_pack_catalog,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{LabResult, canonical_json_bytes};
use actingcommand_policy::{BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package the checkout holds, with its rule.
const EXISTING: &str = "neutral.existing";
/// The package adopted, its rule of the same id.
const ADOPTED: &str = "neutral.daily";
const ADOPTED_DIRECTORY: &str = "packs/neutral.daily";
/// A second rule of the adopted package, written by hand.
const SECOND_RULE: &str = "neutral.daily.second";
const POOL: &str = "pool.neutral.points";
/// The outcome key the made-up packages declare.
const OUTCOME_KEY: &str = "task_done";
const DECISION_SCHEMA: &str = "actingcommand.pack-decision.v1";
const SCHEDULE_FILE: &str = "schedule.json";
const CATALOG_FILE: &str = "pack-catalog.json";
/// The business members a rule of a switched root writes none of.
const BUSINESS: [&str; 6] = [
    "consumes",
    "produces",
    "limited",
    "realization",
    "fixed_value_origin",
    "fixed_value_milli",
];
/// The operations' times, which name their check trees and kept copies.
const FIRST: u64 = 1_000;
const SECOND: u64 = 2_000;
const THIRD: u64 = 3_000;

/// A checkout in a temporary directory, its state directory beside it.
struct Fixture {
    temp: TempDir,
}

impl Fixture {
    /// #464 P1's checkout: `applications.json`, `maintenance.json` (`[]`), the package
    /// [`EXISTING`] and `schedule.json` ([`schedule`]), the root files in the canonical form.
    fn new() -> Self {
        let fixture = Self {
            temp: TempDir::new().expect("a temporary directory"),
        };
        let repo = fixture.repo();
        fs::create_dir_all(repo.join("packs")).expect("the packs directory");
        write(
            &repo.join("applications.json"),
            &json!({
                "schema_version": "actingcommand.applications.v1",
                "game": GAME,
                "servers": {SERVER: {"application_id": "com.example.neutral", "label": "Test"}}
            }),
        );
        write(&repo.join("maintenance.json"), &json!([]));
        write_directory(
            &repo.join("packs").join(EXISTING),
            &package_files(EXISTING, "Existing fixture"),
        );
        write(&repo.join(SCHEDULE_FILE), &schedule());
        fixture
    }

    fn repo(&self) -> PathBuf {
        self.temp.path().join("repo")
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    /// The checkout's root file `name`, read as JSON.
    fn json(&self, name: &str) -> Value {
        serde_json::from_slice(&self.bytes(name)).expect("the file reads as JSON")
    }

    /// The bytes of the checkout's root file `name`.
    fn bytes(&self, name: &str) -> Vec<u8> {
        fs::read(self.repo().join(name)).expect("the root file")
    }
}

/// The version control of a checkout whose every change is committed.
struct Committed;

impl AdoptWorktree for Committed {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn lab(&self) -> &dyn AdoptLab {
        &NoLab
    }
}

/// A machine on which the call stops right before it renames `schedule.json.part`, as a crash
/// there would leave the checkout; every other rename is made. Every change is committed.
struct StopsBeforeTheSchedule;

impl AdoptWorktree for StopsBeforeTheSchedule {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if from.ends_with("schedule.json.part") {
            return Err(io::Error::other("the call stops here"));
        }
        fs::rename(from, to)
    }

    fn lab(&self) -> &dyn AdoptLab {
        &NoLab
    }
}

/// Writes `value` in the canonical form.
fn write(path: &Path, value: &Value) {
    fs::write(path, canonical_json_bytes(value).expect("canonical JSON")).expect("the file");
}

/// The files of the package `package_id` (#464 P1's made-up package): a route from `home` to
/// `page_b`, one tap waiting 200 ms after it, the outcome key [`OUTCOME_KEY`], `goal` its goal, its
/// entry task's provenance a recording checked offline.
fn package_files(package_id: &str, goal: &str) -> Vec<(String, Value)> {
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
        "goal": goal,
        "provenance": {
            "source": "lab_record",
            "record_id": "rec-1",
            "generator": "actinglab record stop",
            "steps": [{"step": 1, "click": {"needs_review": false}}],
            "warnings": [],
            "cross_check": {"status": "passed"}
        },
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
    });
    vec![
        ("control.json".to_owned(), control),
        (
            "resources/operations/resources.json".to_owned(),
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json".to_owned(), task),
    ]
}

/// The files `files` as their canonical bytes, by path.
fn canonical_files(files: &[(String, Value)]) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .map(|(relative, value)| {
            (
                relative.clone(),
                canonical_json_bytes(value).expect("canonical JSON"),
            )
        })
        .collect()
}

/// Writes `files` as a content directory at `directory`, pretty-printed without a final newline.
fn write_directory(directory: &Path, files: &[(String, Value)]) {
    for (relative, value) in files {
        let path = directory.join(relative);
        fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
        fs::write(&path, serde_json::to_vec_pretty(value).expect("JSON")).expect("the file");
    }
}

/// A local clock every `every_ms`.
fn every(every_ms: u64) -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": every_ms,
            "anchor_ms": 0
        }
    })
}

/// A fixed value of the pack's own.
fn fixed(milli: u64) -> Value {
    json!({"fixed_value_milli": milli, "origin": "pack"})
}

/// A business decision on the daily clock with the value `milli`, consuming 3 of [`POOL`] a run
/// and limited: a decision that gives each business member a first adoption writes but the
/// realization mark.
fn business(milli: u64) -> Value {
    json!({
        "schema_version": DECISION_SCHEMA,
        "class": "business",
        "trigger": every(86_400_000),
        "value": fixed(milli),
        "consumes": [{"pool_id": POOL, "amount": 3}],
        "limited": true
    })
}

/// A business decision on the daily clock that gives the value `milli` alone.
fn value_only(milli: u64) -> Value {
    json!({
        "schema_version": DECISION_SCHEMA,
        "class": "business",
        "trigger": every(86_400_000),
        "value": fixed(milli)
    })
}

/// The rule of [`EXISTING`]: business, priority 100, hourly, no value, stopped by its own run's
/// end on the key its package declares.
fn existing_rule() -> Value {
    json!({
        "id": EXISTING,
        "package_id": EXISTING,
        "priority": 100,
        "trigger": every(3_600_000),
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
    })
}

/// A pool of constant capacity 1000 read from `resource.neutral.points`, valued 1000 milli a unit.
fn pool() -> Value {
    json!({
        "id": POOL,
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": "resource.neutral.points"},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
    })
}

/// The checkout's `schedule.json` before any adoption.
fn schedule() -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": [existing_rule()],
        "pools": [pool()]
    })
}

/// The rule #464 P1's first adoption of [`ADOPTED`] writes with a fixed value of the pack's own:
/// the decision's trigger and value, every other member derived.
fn adopted_rule(milli: u64) -> Value {
    json!({
        "id": ADOPTED,
        "package_id": ADOPTED,
        "priority": 100,
        "trigger": every(86_400_000),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": ADOPTED,
            "outcome_key": OUTCOME_KEY,
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [],
        "fixed_value_milli": milli,
        "on_failure": {
            "action": "pause",
            "retry_limit": 1,
            "retry_backoff_ms": 0,
            "escalation_threshold": 2
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

/// Every file below `root`, by its `/`-separated path relative to it.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    collect(root, root, &mut files);
    files
}

fn collect(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("below the root")
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.insert(relative, fs::read(&path).expect("the file"));
        }
    }
}

/// A line of `standard-pack catalog` on the checkout, writing version 2, `refresh` the packages
/// whose entries it brings up to their rules.
fn catalog_line(fixture: &Fixture, refresh: &[&str]) {
    write_standard_pack_catalog(StandardPackCatalogRequest {
        root: fixture.repo(),
        plan: false,
        refresh: refresh
            .iter()
            .map(|package_id| (*package_id).to_owned())
            .collect(),
        displays: None,
        values: None,
        version_2: true,
    })
    .unwrap_or_else(|error| panic!("the catalog line: {}: {}", error.code, error.message));
}

/// `standard-pack catalog --cut` of the checkout, which switches it.
fn cut_over(fixture: &Fixture) {
    let answer = cut_standard_pack_catalog(StandardPackCatalogCutRequest {
        root: fixture.repo(),
        plan: false,
    })
    .unwrap_or_else(|error| {
        panic!(
            "the cut: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });
    assert_eq!(answer.status, "written");
}

/// An adoption of [`ADOPTED`] into the checkout, its package a content directory named `name`
/// beside it, with the decision `decision` when given.
fn adoption(
    fixture: &Fixture,
    name: &str,
    decision: Option<&Value>,
    replace: bool,
    at_unix_ms: u64,
    worktree: &dyn AdoptWorktree,
) -> LabResult<StandardPackAdoptResponse> {
    let directory = fixture.path(name);
    write_directory(&directory, &package_files(ADOPTED, "Adopted fixture"));
    let document = decision.map(|decided| {
        let path = fixture.path(&format!("{name}.decision.json"));
        fs::write(&path, serde_json::to_vec_pretty(decided).expect("JSON")).expect("the decision");
        path
    });
    standard_pack_adopt(
        StandardPackAdoptRequest {
            repo: fixture.repo(),
            source: AdoptSource::Package(directory),
            decision: document.map(AdoptDecision::Document),
            replace,
            dry_run: false,
            state_dir: fixture.state(),
            at_unix_ms,
        },
        worktree,
    )
}

/// [`adoption`] that is accepted.
fn adopted_by(
    fixture: &Fixture,
    name: &str,
    decision: Option<&Value>,
    replace: bool,
    at_unix_ms: u64,
) -> StandardPackAdoptResponse {
    adoption(fixture, name, decision, replace, at_unix_ms, &Committed).unwrap_or_else(|error| {
        panic!(
            "{name}: {}: {} {:?}",
            error.code, error.message, error.details
        )
    })
}

/// The root of `prepare` once a fixture: #464 P1's checkout, prepared, and its catalog of version
/// 2 written by the writer from its rules, so that the root can be cut over.
fn prepared_root(prepare: fn(&Fixture)) -> Fixture {
    let fixture = Fixture::new();
    prepare(&fixture);
    catalog_line(&fixture, &[]);
    fixture
}

/// Nothing more.
fn as_is(_fixture: &Fixture) {}

/// [`ADOPTED`] adopted first into the root not switched yet ([`business`], 9 000 milli).
fn adopted_first(fixture: &Fixture) {
    adopted_by(fixture, "first", Some(&business(9_000)), false, FIRST);
}

/// [`adopted_first`], then a second rule of [`ADOPTED`], [`SECOND_RULE`], written by hand as a
/// copy of its first: the two rules of one package write the same business part.
fn two_rules(fixture: &Fixture) {
    adopted_first(fixture);
    let mut written = fixture.json(SCHEDULE_FILE);
    let rules = written["rules"].as_array_mut().expect("the rules");
    let mut second = rules
        .iter()
        .find(|rule| rule["id"] == ADOPTED)
        .cloned()
        .expect("the adopted rule");
    second["id"] = json!(SECOND_RULE);
    rules.push(second);
    write(&fixture.repo().join(SCHEDULE_FILE), &written);
}

/// The composition of the checkout of `fixture`: its packages and its `schedule.json` bundled by
/// the build's own `package_bundle`, laid out as an installed game root with the checkout's
/// `pack-catalog.json` beside it, and composed by `standard-pack compose` for the instance the
/// tools' check binds: the rules digest, the catalog hash and every composed document's bytes.
fn composition(fixture: &Fixture) -> (String, String, BTreeMap<String, Vec<u8>>) {
    let repo = fixture.repo();
    let bundled = fixture.path("composition-bundle");
    package_bundle(PackageBundleRequest {
        applications: repo.join("applications.json"),
        packs_root: repo.join("packs"),
        out: bundled.clone(),
        source: None,
        maintenance: Some(Vec::new()),
        root_files: StandardPackRootFiles {
            schedule: Some(repo.join(SCHEDULE_FILE)),
            ..StandardPackRootFiles::default()
        },
    })
    .unwrap_or_else(|error| panic!("the checkout bundles: {}: {}", error.code, error.message));
    let installed = fixture.path("composition-root").join(GAME);
    for (relative, bytes) in tree(&bundled) {
        let laid = relative.strip_prefix("packs/").unwrap_or(&relative);
        let target = installed.join(laid);
        fs::create_dir_all(target.parent().expect("a parent")).expect("the directory");
        fs::write(&target, bytes).expect("the file");
    }
    fs::write(installed.join(CATALOG_FILE), fixture.bytes(CATALOG_FILE)).expect("the catalog");
    let out = fixture.path("composition-out");
    let answer = compose_standard_packs(StandardPackComposeRequest {
        root: fixture.path("composition-root"),
        instances: vec![(
            format!("standard-pack-check.{SERVER}"),
            GAME.to_owned(),
            SERVER.to_owned(),
        )],
        out: out.clone(),
    })
    .unwrap_or_else(|error| panic!("the root composes: {}: {}", error.code, error.message));
    (answer.rules_digest, answer.catalog_hash, tree(&out))
}

/// The text of the root file `name` of the checkout of `fixture`, for a readable difference.
fn text(fixture: &Fixture, name: &str) -> String {
    String::from_utf8(fixture.bytes(name)).expect("UTF-8")
}

/// Ruling 2: the checkout adopted as not switched and then cut over (`plain`) and the one cut
/// over first and then adopted (`cut_first`) hold the same `schedule.json` and `pack-catalog.json`
/// byte for byte, the same files everywhere else, and compose the same: digest, hash, documents.
fn assert_same_roots(plain: &Fixture, cut_first: &Fixture) {
    for name in [SCHEDULE_FILE, CATALOG_FILE] {
        assert_eq!(text(plain, name), text(cut_first, name), "{name}");
    }
    assert_eq!(tree(&plain.repo()), tree(&cut_first.repo()));
    let left = composition(plain);
    let right = composition(cut_first);
    assert_eq!(left, right);
}

/// The root of `fixture` is switched and its rules write no business member.
fn assert_switched(fixture: &Fixture) {
    let written = fixture.json(SCHEDULE_FILE);
    assert_eq!(
        written["requires"]
            .as_array()
            .and_then(|names| names.last())
            .and_then(Value::as_str),
        Some(CATALOG_BUSINESS_SUBSYSTEM),
        "{written}"
    );
    for rule in written["rules"].as_array().expect("the rules") {
        for member in BUSINESS {
            assert!(rule.get(member).is_none(), "{member}: {rule}");
        }
    }
}

/// The entry of `package_id` in the checkout's catalog, and its place among the entries.
fn entry_in_catalog(fixture: &Fixture, package_id: &str) -> (usize, Value) {
    let written = fixture.json(CATALOG_FILE);
    written["packs"]
        .as_array()
        .expect("the entries")
        .iter()
        .enumerate()
        .find(|(_, entry)| entry["package_id"] == package_id)
        .map(|(position, entry)| (position, entry.clone()))
        .expect("the package's entry")
}

/// An effect of [`POOL`] as the catalog writes it, consuming `amount` a run.
fn consumed(amount: u64) -> Value {
    json!({
        "pool_id": POOL,
        "direction": "consume",
        "amount": amount,
        "observation_source": "self_reported",
        "confidence_milli": 1000
    })
}

/// Ruling 2 (1): a first adoption. Adopting [`ADOPTED`] into the root not switched, bringing its
/// entry up to its rule (`--refresh`) and cutting the root over gives what adopting it into the
/// root cut over first gives: `requires` naming `benefit_fixed_value` before `catalog_business`,
/// the rule without its business part, the entry's own part `{consumes, produces, value,
/// limited}` after the other entries. The switched adoption writes the pack directory, the
/// catalog and `schedule.json`, in that order, and answers the rule as written and the entry.
#[test]
fn cv2b_r2_a_first_adoption_into_a_switched_root_equals_adopting_then_cutting_over() {
    let plain = prepared_root(as_is);
    let cut_first = prepared_root(as_is);
    cut_over(&cut_first);

    adopted_by(&plain, "package", Some(&business(9_000)), false, FIRST);
    catalog_line(&plain, &[ADOPTED]);
    cut_over(&plain);
    let answer = adopted_by(&cut_first, "package", Some(&business(9_000)), false, FIRST);

    assert_same_roots(&plain, &cut_first);
    assert_switched(&cut_first);
    let written = cut_first.json(SCHEDULE_FILE);
    assert_eq!(
        written["requires"],
        json!([BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM])
    );
    let entry = json!({
        "package_id": ADOPTED,
        "consumes": [consumed(3)],
        "produces": [],
        "value": {"milli": 9000},
        "limited": true
    });
    assert_eq!(entry_in_catalog(&cut_first, ADOPTED), (1, entry.clone()));
    assert_eq!(
        answer.written,
        [ADOPTED_DIRECTORY, CATALOG_FILE, SCHEDULE_FILE]
    );
    assert_eq!(answer.catalog_entry, Some(entry));
    assert_eq!(answer.rule.as_ref(), Some(&written["rules"][1]));
}

/// Ruling 2 (2): a re-adoption that changes the value (9 000 to 7 000 milli, the decision naming
/// no other business member). The own part's value is the new one, its effects and mark kept, and
/// the two roots are the same; the entry as it was and as written is answered among the entries
/// replaced.
#[test]
fn cv2b_r2_a_readoption_that_changes_the_value_equals_adopting_then_cutting_over() {
    let plain = prepared_root(adopted_first);
    let cut_first = prepared_root(adopted_first);
    cut_over(&cut_first);

    adopted_by(&plain, "again", Some(&value_only(7_000)), true, SECOND);
    catalog_line(&plain, &[ADOPTED]);
    cut_over(&plain);
    let answer = adopted_by(&cut_first, "again", Some(&value_only(7_000)), true, SECOND);

    assert_same_roots(&plain, &cut_first);
    assert_switched(&cut_first);
    let entry = json!({
        "package_id": ADOPTED,
        "consumes": [consumed(3)],
        "produces": [],
        "value": {"milli": 7000},
        "limited": true
    });
    let (position, written) = entry_in_catalog(&cut_first, ADOPTED);
    assert_eq!(written, entry);
    assert_eq!(answer.written, [ADOPTED_DIRECTORY, CATALOG_FILE]);
    let replaced = answer
        .replaced_entries
        .iter()
        .find(|change| change.file == CATALOG_FILE)
        .expect("the entry replaced");
    assert_eq!(replaced.json_pointer, format!("/packs/{position}"));
    assert_eq!(replaced.before["value"], json!({"milli": 9000}));
    assert_eq!(replaced.after.as_ref(), Some(&entry));
}

/// Ruling 2 (3), ruling 1's second rule: the package is run by two rules whose entry holds one own
/// part, and the re-adoption gives its first rule another value. The adopted rule gets a per-rule
/// part, the other rule's part, which the own part was, is moved to a per-rule part of its own, in
/// the file's order, and the entry keeps no own part: what `--refresh` makes of rules that differ.
#[test]
fn cv2b_r2_a_readoption_of_one_of_two_rules_equals_adopting_then_cutting_over() {
    let plain = prepared_root(two_rules);
    let cut_first = prepared_root(two_rules);
    let (_, own) = entry_in_catalog(&cut_first, ADOPTED);
    assert!(
        own.get("rules").is_none() && own.get("value").is_some(),
        "one own part before: {own}"
    );
    cut_over(&cut_first);

    adopted_by(&plain, "again", Some(&value_only(7_000)), true, SECOND);
    catalog_line(&plain, &[ADOPTED]);
    cut_over(&plain);
    adopted_by(&cut_first, "again", Some(&value_only(7_000)), true, SECOND);

    assert_same_roots(&plain, &cut_first);
    assert_switched(&cut_first);
    assert_eq!(
        entry_in_catalog(&cut_first, ADOPTED).1,
        json!({
            "package_id": ADOPTED,
            "rules": [per_rule_part(ADOPTED, 7_000), per_rule_part(SECOND_RULE, 9_000)]
        })
    );
}

/// The per-rule part of the rule `rule_id` as the catalog writes it: consuming 3 of [`POOL`] a
/// run, the value `milli`, limited.
fn per_rule_part(rule_id: &str, milli: u64) -> Value {
    json!({
        "rule_id": rule_id,
        "consumes": [consumed(3)],
        "produces": [],
        "value": {"milli": milli},
        "limited": true
    })
}

/// The outcome a refused or failed call's error carries, after asserting that it is
/// `adopt_refused` with `problem`.
fn refusal(result: LabResult<StandardPackAdoptResponse>, problem: AdoptProblem) -> Value {
    let error = result.expect_err("the adoption is refused");
    let outcome = error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .cloned()
        .unwrap_or_else(|| panic!("the refusal carries its outcome: {error}"));
    assert_eq!(
        outcome["code"],
        ResourceToolingCode::AdoptRefused.as_str(),
        "{error}"
    );
    assert_eq!(
        outcome["values"]["adopt_problem"],
        problem.as_str(),
        "{error}"
    );
    outcome
}

/// Ruling 3: nothing is written when the root as it would be written does not compose. A first
/// adoption into a switched root whose decision consumes 0 a run gives the catalog an effect its
/// reader refuses: the check of the tree, which reads both files as they would be written, refuses
/// it (`rule_refused` at the catalog's member, `catalog_out_of_limits` its cause), and the checkout
/// is as it was, both files included. The catalog is renamed before `schedule.json`: a call that
/// stops right before it renames `schedule.json.part` has written the catalog, left
/// `schedule.json` as it was and its `.part` beside it, which the next call refuses
/// (`repository_invalid` at it).
#[test]
fn cv2b_r3_an_adoption_that_would_not_compose_writes_neither_file() {
    let fixture = prepared_root(as_is);
    cut_over(&fixture);
    let before = tree(&fixture.repo());
    let mut decision = business(9_000);
    decision["consumes"][0]["amount"] = json!(0);
    let outcome = refusal(
        adoption(&fixture, "zero", Some(&decision), false, FIRST, &Committed),
        AdoptProblem::RuleRefused,
    );
    assert_eq!(outcome["values"]["entry"], CATALOG_FILE, "{outcome}");
    assert_eq!(
        outcome["causes"][0]["values"]["standard_pack_problem"], "catalog_out_of_limits",
        "{outcome}"
    );
    assert_eq!(tree(&fixture.repo()), before, "nothing written");

    let schedule_before = fixture.bytes(SCHEDULE_FILE);
    let error = adoption(
        &fixture,
        "stops",
        Some(&business(9_000)),
        false,
        SECOND,
        &StopsBeforeTheSchedule,
    )
    .expect_err("the call stops before it renames schedule.json");
    assert_eq!(
        error.code,
        ResourceToolingCode::AdoptWriteFailed.as_str(),
        "{}",
        error.message
    );
    assert!(
        fixture.json(CATALOG_FILE)["packs"]
            .as_array()
            .expect("the entries")
            .iter()
            .any(|entry| entry["package_id"] == ADOPTED),
        "the catalog was renamed first"
    );
    assert_eq!(fixture.bytes(SCHEDULE_FILE), schedule_before);
    assert!(fixture.repo().join("schedule.json.part").is_file());
    assert!(!fixture.repo().join("pack-catalog.json.part").exists());
    let error = adoption(
        &fixture,
        "after",
        Some(&business(9_000)),
        true,
        THIRD,
        &Committed,
    )
    .expect_err("the leftover refuses the next call");
    // Fix round 1: the refusal names the catalog that call replaced already, and its undo
    // restores the catalog too.
    assert!(
        error
            .message
            .contains("renames pack-catalog.json, schedule.json and maintenance.json")
            && error
                .message
                .contains("restore it from the repository's history too"),
        "{}",
        error.message
    );
    let outcome = refusal(Err(error), AdoptProblem::RepositoryInvalid);
    assert_eq!(
        outcome["values"]["entry"], "schedule.json.part",
        "{outcome}"
    );
}

/// Ruling 4, the golden of #464 P1's first adoption: a root that does not name
/// `catalog_business`, holding a catalog of version 2, is adopted byte for byte as before. Exactly
/// the pack directory and `schedule.json` are written (the rule appended with its business
/// members, `requires` after `schema_version`), the catalog's bytes are kept, and the answer is
/// P1's: the rule as written, no `catalog_entry`.
#[test]
fn cv2b_r4_a_root_that_is_not_switched_is_adopted_byte_for_byte_as_before() {
    let fixture = prepared_root(as_is);
    let before = tree(&fixture.repo());
    assert!(before.contains_key(CATALOG_FILE));

    let answer = adopted_by(&fixture, "package", Some(&value_only(9_000)), false, FIRST);

    let mut expected = before;
    for (relative, bytes) in canonical_files(&package_files(ADOPTED, "Adopted fixture")) {
        expected.insert(format!("{ADOPTED_DIRECTORY}/{relative}"), bytes);
    }
    let golden = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [BENEFIT_FIXED_VALUE_SUBSYSTEM],
        "rules": [existing_rule(), adopted_rule(9_000)],
        "pools": [pool()]
    });
    expected.insert(
        SCHEDULE_FILE.to_owned(),
        canonical_json_bytes(&golden).expect("canonical JSON"),
    );
    assert_eq!(tree(&fixture.repo()), expected);
    assert_eq!(answer.written, [ADOPTED_DIRECTORY, SCHEDULE_FILE]);
    assert_eq!(answer.rule, Some(adopted_rule(9_000)));
    assert_eq!(answer.requires_added, [BENEFIT_FIXED_VALUE_SUBSYSTEM]);
    assert!(answer.replaced_entries.is_empty());
    let serialized = serde_json::to_value(&answer).expect("the answer serializes");
    assert!(serialized.get("catalog_entry").is_none(), "{serialized}");
}

/// Ruling 5: a re-adoption that carries no business member leaves the catalog's part alone. On a
/// switched root whose package is run by two rules sharing the entry's own part, a decision that
/// names only the trigger changes the first rule's trigger in `schedule.json` and not one byte of
/// the catalog (no part is moved for the rule); and an adoption again without a decision writes
/// neither file.
#[test]
fn cv2b_r5_a_readoption_without_a_business_member_leaves_the_catalog_alone() {
    let fixture = prepared_root(two_rules);
    cut_over(&fixture);
    let catalog_before = fixture.bytes(CATALOG_FILE);
    let (_, entry) = entry_in_catalog(&fixture, ADOPTED);

    let retrigger = json!({
        "schema_version": DECISION_SCHEMA,
        "class": "business",
        "trigger": every(3_600_000)
    });
    let answer = adopted_by(&fixture, "retrigger", Some(&retrigger), true, SECOND);
    assert_eq!(fixture.bytes(CATALOG_FILE), catalog_before);
    assert_eq!(answer.written, [ADOPTED_DIRECTORY, SCHEDULE_FILE]);
    assert_eq!(answer.catalog_entry, Some(entry));
    assert_eq!(
        fixture.json(SCHEDULE_FILE)["rules"][1]["trigger"],
        every(3_600_000)
    );
    assert_switched(&fixture);

    let schedule_before = fixture.bytes(SCHEDULE_FILE);
    let answer = adopted_by(&fixture, "plain", None, true, THIRD);
    assert_eq!(fixture.bytes(CATALOG_FILE), catalog_before);
    assert_eq!(fixture.bytes(SCHEDULE_FILE), schedule_before);
    assert_eq!(answer.written, [ADOPTED_DIRECTORY]);
}

/// A switched root whose catalog is missing, or of version 1, is refused before anything is
/// written: `repository_invalid` at `pack-catalog.json`, its cause `standard_pack_root_invalid`
/// with the word the composer and the writer use for it, `catalog_missing` (the whole document)
/// and `catalog_version_1` (at `/schema_version`).
#[test]
fn cv2b_a_switched_root_without_a_catalog_of_version_2_is_refused_with_its_word() {
    let fixture = prepared_root(as_is);
    cut_over(&fixture);
    fs::remove_file(fixture.repo().join(CATALOG_FILE)).expect("the catalog removed");
    let before = tree(&fixture.repo());
    let outcome = refusal(
        adoption(
            &fixture,
            "missing",
            Some(&business(9_000)),
            false,
            FIRST,
            &Committed,
        ),
        AdoptProblem::RepositoryInvalid,
    );
    assert_eq!(outcome["values"]["entry"], CATALOG_FILE, "{outcome}");
    let cause = &outcome["causes"][0];
    assert_eq!(cause["code"], "standard_pack_root_invalid", "{outcome}");
    assert_eq!(
        cause["values"]["standard_pack_problem"], "catalog_missing",
        "{outcome}"
    );
    assert_eq!(tree(&fixture.repo()), before, "nothing written");

    write(
        &fixture.repo().join(CATALOG_FILE),
        &json!({
            "schema_version": "actingcommand.pack-catalog.v1",
            "game": GAME,
            "packs": [{"package_id": EXISTING, "consumes": [], "produces": []}]
        }),
    );
    let before = tree(&fixture.repo());
    let outcome = refusal(
        adoption(
            &fixture,
            "version-1",
            Some(&business(9_000)),
            false,
            SECOND,
            &Committed,
        ),
        AdoptProblem::RepositoryInvalid,
    );
    let cause = &outcome["causes"][0];
    assert_eq!(
        cause["values"]["standard_pack_problem"], "catalog_version_1",
        "{outcome}"
    );
    assert_eq!(
        outcome["values"]["json_pointer"], "/schema_version",
        "{outcome}"
    );
    assert_eq!(tree(&fixture.repo()), before, "nothing written");
}

/// The checkout's catalog with the entry of `package_id` replaced by what `edit` makes of it,
/// written in the canonical form, as the writer writes the file.
fn edit_entry(fixture: &Fixture, package_id: &str, edit: impl FnOnce(&Value) -> Value) {
    let mut written = fixture.json(CATALOG_FILE);
    let (position, entry) = entry_in_catalog(fixture, package_id);
    written["packs"][position] = edit(&entry);
    write(&fixture.repo().join(CATALOG_FILE), &written);
}

/// Fix round 1, ruling 6: an entry that carries its display (`title`, `summary`, written before its
/// business part) keeps it, member for member and in its place, when an adoption writes its
/// business part (the value 9 000 to 7 000 milli).
#[test]
fn cv2b_fr1_an_entrys_display_is_written_as_it_stands() {
    let fixture = prepared_root(adopted_first);
    cut_over(&fixture);
    edit_entry(&fixture, ADOPTED, |entry| {
        let mut displayed = Map::new();
        displayed.insert("package_id".to_owned(), json!(ADOPTED));
        displayed.insert("title".to_owned(), json!({"en-US": "Daily"}));
        displayed.insert(
            "summary".to_owned(),
            json!({"en-US": "Collects the daily points."}),
        );
        for (name, value) in entry.as_object().expect("an entry") {
            if name != "package_id" {
                displayed.insert(name.clone(), value.clone());
            }
        }
        Value::Object(displayed)
    });

    adopted_by(&fixture, "again", Some(&value_only(7_000)), true, SECOND);

    let (_, entry) = entry_in_catalog(&fixture, ADOPTED);
    assert_eq!(
        entry
            .as_object()
            .expect("an entry")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "package_id",
            "title",
            "summary",
            "consumes",
            "produces",
            "value",
            "limited"
        ]
    );
    assert_eq!(entry["title"], json!({"en-US": "Daily"}));
    assert_eq!(
        entry["summary"],
        json!({"en-US": "Collects the daily points."})
    );
    assert_eq!(entry["value"], json!({"milli": 7000}));
}

/// Fix round 1, ruling 1: a package run by one rule whose entry holds a per-rule part for it (as
/// `--values` with a `rule_id` makes one, its own part beside it): the per-rule part, which stands
/// for the rule, is the one written, and the own part stays as it stood.
#[test]
fn cv2b_fr1_a_package_of_one_rule_whose_entry_holds_its_per_rule_part() {
    let fixture = prepared_root(adopted_first);
    cut_over(&fixture);
    edit_entry(&fixture, ADOPTED, |entry| {
        let mut held = entry.clone();
        held["rules"] = json!([per_rule_part(ADOPTED, 8_000)]);
        held
    });

    adopted_by(&fixture, "again", Some(&value_only(7_000)), true, SECOND);

    let (_, entry) = entry_in_catalog(&fixture, ADOPTED);
    assert_eq!(
        entry["value"],
        json!({"milli": 9000}),
        "the own part as it stood: {entry}"
    );
    assert_eq!(entry["rules"], json!([per_rule_part(ADOPTED, 7_000)]));
}

/// Fix round 1, ruling 1: an entry holding a per-rule part for another rule of the package (the
/// second rule's, beside the own part the first rule stands on): the adopted first rule gets a
/// per-rule part of its own, after the one there, which is kept, and the entry keeps no own part,
/// which no rule stands on any more.
#[test]
fn cv2b_fr1_an_entry_with_a_per_rule_part_of_another_rule_keeps_it() {
    let fixture = prepared_root(two_rules);
    cut_over(&fixture);
    edit_entry(&fixture, ADOPTED, |entry| {
        let mut held = entry.clone();
        held["rules"] = json!([per_rule_part(SECOND_RULE, 8_000)]);
        held
    });

    adopted_by(&fixture, "again", Some(&value_only(7_000)), true, SECOND);

    assert_eq!(
        entry_in_catalog(&fixture, ADOPTED).1,
        json!({
            "package_id": ADOPTED,
            "rules": [per_rule_part(SECOND_RULE, 8_000), per_rule_part(ADOPTED, 7_000)]
        })
    );
}

/// Fix round 1 (the reading without a registry): a catalog of more than 64 entries, the last of
/// them, the adopted package's, holding an effect with a member an effect does not know. The
/// adoption reads the file without judging it against a registry, so no `package_not_listed`
/// fills the reader's bound of 64 errors, and the broken effect, which the reader leaves out,
/// refuses the adoption before anything is written: `repository_invalid` at `pack-catalog.json`,
/// `malformed` at the effect.
#[test]
fn cv2b_fr1_a_broken_element_past_64_entries_refuses_the_adoption() {
    let fixture = prepared_root(as_is);
    cut_over(&fixture);
    let mut written = fixture.json(CATALOG_FILE);
    let packs = written["packs"].as_array_mut().expect("the entries");
    for filler in 0..64 {
        packs.push(json!({"package_id": format!("neutral.filler.{filler}")}));
    }
    let mut broken = consumed(3);
    broken["colour"] = json!("blue");
    packs.push(json!({"package_id": ADOPTED, "consumes": [broken], "produces": []}));
    let broken_at = format!("/packs/{}/consumes/0", packs.len() - 1);
    write(&fixture.repo().join(CATALOG_FILE), &written);
    let before = tree(&fixture.repo());

    let outcome = refusal(
        adoption(
            &fixture,
            "broken",
            Some(&business(9_000)),
            false,
            FIRST,
            &Committed,
        ),
        AdoptProblem::RepositoryInvalid,
    );

    assert_eq!(outcome["values"]["entry"], CATALOG_FILE, "{outcome}");
    assert_eq!(outcome["values"]["json_pointer"], broken_at, "{outcome}");
    assert_eq!(
        outcome["causes"][0]["values"]["standard_pack_problem"], "malformed",
        "{outcome}"
    );
    assert_eq!(tree(&fixture.repo()), before, "nothing written");
}
