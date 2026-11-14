// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #464 P1 (register §1.365; contracts/standard-pack-authoring.md; coordinator work order
// `rd-1114/brief-464-p1-adopt.md`, the design's row "P1 adopt" of its section G): `actinglab
// standard-pack adopt` on a made-up resource repository of the game `neutral`, server `test`,
// which holds one package, its business rule and a valued pool.
// - a first adoption of a stopped recording writes exactly the pack directory (its JSON canonical)
//   and `schedule.json` (the rule appended with its derived members, `requires` gaining
//   `benefit_fixed_value`), every other byte of the repository as it was; the repository as
//   written bundles and composes;
// - a second identical adoption is refused `pack_exists`;
// - `--replace` without a decision keeps `schedule.json` byte for byte and moves the old pack
//   directory to `<state>/adopt/<package_id>.old-<ms>/`;
// - the coordinator's ruling on the value's origin: a decision that says `model` is refused
//   `value_origin_model` and nothing is written; a value a caller gives is written without an
//   origin and answered as given with the origin `pack`; a rule that already carries
//   `fixed_value_origin: "model"` keeps it, whatever value a re-adoption's decision gives or leaves
//   out, and answers it in `model_kept`;
// - each check refuses naming its member or pointer and writes nothing in the repository;
// - the classes `held` (a held entry, no rule, for a package not run through) and `maintenance`
//   (a rule declaring itself maintenance, whose priority the file's rules cannot give);
// - a pending value answers the coverage `undeclared`;
// - `--dry-run` runs every check and writes nothing;
// - a JSON container and a ZIP of the same files give the same directory and the same rule.
// No device, no Runtime, no clock read, no sleep: temporary directories and made-up packages.

use crate::codes::AdoptProblem;
use crate::{
    AdoptClass, AdoptCoverage, AdoptSource, AdoptWorktree, DerivedFrom, PackageBundleRequest,
    ResourceToolingCode, StandardPackAdoptRequest, StandardPackAdoptResponse,
    StandardPackRootFiles, package_bundle, standard_pack_adopt,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{LabResult, canonical_json_bytes};
use actingcommand_policy::BENEFIT_FIXED_VALUE_SUBSYSTEM;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package the repository holds, with its rule.
const EXISTING: &str = "neutral.existing";
/// The package adopted.
const ADOPTED: &str = "neutral.daily";
const ADOPTED_DIRECTORY: &str = "packs/neutral.daily";
const POOL: &str = "pool.neutral.points";
const RECORD: &str = "rec-1";
/// The outcome key the made-up packages declare.
const OUTCOME_KEY: &str = "task_done";
const DECISION_SCHEMA: &str = "actingcommand.pack-decision.v1";
/// The operations' times, which name their check trees and kept copies.
const FIRST: u64 = 1_000;
const SECOND: u64 = 2_000;
const THIRD: u64 = 3_000;
const FOURTH: u64 = 4_000;

/// A repository checkout in a temporary directory, its state directory beside it.
struct Fixture {
    temp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::with_maintenance(true)
    }

    /// The repository: `applications.json`, `maintenance.json` (`[]`, unless `maintenance` is
    /// false), the package [`EXISTING`] and `schedule.json` ([`schedule`]), the root files in the
    /// canonical form.
    fn with_maintenance(maintenance: bool) -> Self {
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
        if maintenance {
            write(&repo.join("maintenance.json"), &json!([]));
        }
        write_directory(
            &repo.join("packs").join(EXISTING),
            &package_files(EXISTING, SERVER, "Existing fixture", &json!({})),
        );
        write(&repo.join("schedule.json"), &schedule());
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

    fn schedule(&self) -> Value {
        serde_json::from_slice(&fs::read(self.repo().join("schedule.json")).expect("schedule.json"))
            .expect("schedule.json reads")
    }
}

/// The version control of a checkout whose every change is committed.
struct Committed;

impl AdoptWorktree for Committed {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }
}

/// The version control of a checkout with a change not committed below every directory.
struct Changed;

impl AdoptWorktree for Changed {
    fn changes_under(&self, _repo: &Path, relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(vec![format!("{relative}/control.json")])
    }
}

/// Writes `value` in the canonical form.
fn write(path: &Path, value: &Value) {
    fs::write(path, canonical_json_bytes(value).expect("canonical JSON")).expect("the file");
}

/// The provenance of a recording's entry task (`contracts/lab-recording.md`, "Pages and
/// documents") whose cross-check is `cross_check`, its first decision evaluated and no step to
/// review.
fn provenance(cross_check: &str) -> Value {
    json!({
        "source": "lab_record",
        "record_id": RECORD,
        "generator": "actinglab record stop",
        "steps": [{"step": 1, "click": {"needs_review": false}}],
        "warnings": [],
        "cross_check": {"status": cross_check}
    })
}

/// The files of the package `package_id` of `server`: a route from `home` (a blue first pixel)
/// to `page_b` (a red one), its one tap waiting 200 ms after it, the outcome key
/// [`OUTCOME_KEY`], `goal` its goal and `provenance` its entry task's provenance.
fn package_files(
    package_id: &str,
    server: &str,
    goal: &str,
    provenance: &Value,
) -> Vec<(String, Value)> {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": server,
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
        "server_scope": [server],
        "locale": "en-US",
        "goal": goal,
        "provenance": provenance,
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

/// Writes `files` as a ZIP container at `path`.
fn write_zip(path: &Path, files: &[(String, Value)]) {
    let mut zip = ZipWriter::new(File::create(path).expect("the ZIP file"));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (relative, value) in files {
        zip.start_file(relative.as_str(), options)
            .expect("a ZIP entry");
        zip.write_all(&serde_json::to_vec_pretty(value).expect("JSON"))
            .expect("the entry's bytes");
    }
    zip.finish().expect("the ZIP finishes");
}

/// Writes `files` as an `actingcommand.package.content-json.v1` container at `path`.
fn write_json_container(path: &Path, files: &[(String, Value)]) {
    let texts = files
        .iter()
        .map(|(relative, value)| {
            (
                relative.clone(),
                Value::String(serde_json::to_string_pretty(value).expect("JSON")),
            )
        })
        .collect::<Map<_, _>>();
    let container = json!({
        "schema_version": "actingcommand.package.content-json.v1",
        "files": texts
    });
    fs::write(path, serde_json::to_vec(&container).expect("JSON")).expect("the container");
}

/// The package `files` as a content directory named `name` beside the repository.
fn package_dir(fixture: &Fixture, name: &str, files: &[(String, Value)]) -> AdoptSource {
    let directory = fixture.path(name);
    write_directory(&directory, files);
    AdoptSource::Package(directory)
}

/// The recording [`RECORD`] of the state directory: `document` its `recording.json`, `files` its
/// last stop's package, a ZIP.
fn recording(fixture: &Fixture, document: &Value, files: &[(String, Value)]) -> AdoptSource {
    let lab_dir = fixture
        .state()
        .join("record-artifacts")
        .join(RECORD)
        .join("lab");
    fs::create_dir_all(lab_dir.join("out")).expect("the recording's directory");
    write(&lab_dir.join("recording.json"), document);
    write_zip(&lab_dir.join("out").join(format!("{ADOPTED}.zip")), files);
    AdoptSource::Recording {
        record_id: RECORD.to_owned(),
        lab_dir,
    }
}

/// A recording whose last stop generated the package [`ADOPTED`] as a ZIP.
fn stopped() -> Value {
    json!({
        "schema_version": "actingcommand.lab-recording.v1",
        "record_id": RECORD,
        "status": "stopped",
        "artifact": {"container": "zip", "path": "out", "package_id": ADOPTED},
        "steps": []
    })
}

/// Writes the decision document `decision` beside the repository.
fn decide(fixture: &Fixture, name: &str, decision: &Value) -> PathBuf {
    let path = fixture.path(&format!("{name}.json"));
    fs::write(&path, serde_json::to_vec_pretty(decision).expect("JSON")).expect("the decision");
    path
}

/// A business decision on the daily clock with `value`.
fn business(value: &Value) -> Value {
    json!({
        "schema_version": DECISION_SCHEMA,
        "class": "business",
        "trigger": every(86_400_000),
        "value": value
    })
}

/// A fixed value of the pack's own.
fn fixed(milli: u64) -> Value {
    json!({"fixed_value_milli": milli, "origin": "pack"})
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

/// The rule of [`EXISTING`]: business, priority 100, hourly, no value.
fn existing_rule() -> Value {
    json!({
        "id": EXISTING,
        "package_id": EXISTING,
        "priority": 100,
        "trigger": every(3_600_000),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": EXISTING,
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

/// The repository's `schedule.json` before any adoption.
fn schedule() -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": [existing_rule()],
        "pools": [pool()]
    })
}

/// The rule a first adoption of [`ADOPTED`] writes with a fixed value of the pack's own: the
/// decision's trigger and value, every other member derived.
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
        "expected_duration_ms": 200,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    })
}

fn request(
    fixture: &Fixture,
    source: AdoptSource,
    decision: Option<PathBuf>,
    replace: bool,
    at_unix_ms: u64,
) -> StandardPackAdoptRequest {
    StandardPackAdoptRequest {
        repo: fixture.repo(),
        source,
        decision,
        replace,
        dry_run: false,
        state_dir: fixture.state(),
        at_unix_ms,
    }
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

/// Asserts that `result` is the refusal `problem` holding each of `values`, and that nothing was
/// written in the repository of `fixture` (its files `before`) and nothing kept.
fn assert_refused(
    result: LabResult<StandardPackAdoptResponse>,
    problem: AdoptProblem,
    values: &Value,
    fixture: &Fixture,
    before: &BTreeMap<String, Vec<u8>>,
) -> Value {
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
    for (key, value) in values.as_object().expect("the values") {
        assert_eq!(&outcome["values"][key.as_str()], value, "{key}: {error}");
    }
    assert_eq!(&tree(&fixture.repo()), before, "nothing written: {error}");
    assert!(
        !fixture.state().join("adopt").exists(),
        "nothing kept: {error}"
    );
    outcome
}

#[test]
fn p1_a_first_adoption_writes_the_pack_directory_the_rule_and_the_requires_words() {
    let fixture = Fixture::new();
    let files = package_files(ADOPTED, SERVER, "Adopted fixture", &provenance("passed"));
    let source = recording(&fixture, &stopped(), &files);
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let before = tree(&fixture.repo());

    let answer = standard_pack_adopt(
        request(&fixture, source, Some(decision), false, FIRST),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the recording is adopted: {error}"));

    // Exactly the pack directory and `schedule.json`: the rule appended, `requires` after
    // `schema_version`; every other byte as it was.
    let mut expected = before.clone();
    for (relative, bytes) in canonical_files(&files) {
        expected.insert(format!("{ADOPTED_DIRECTORY}/{relative}"), bytes);
    }
    let written = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [BENEFIT_FIXED_VALUE_SUBSYSTEM],
        "rules": [existing_rule(), adopted_rule(9_000)],
        "pools": [pool()]
    });
    expected.insert(
        "schedule.json".to_owned(),
        canonical_json_bytes(&written).expect("canonical JSON"),
    );
    assert_eq!(tree(&fixture.repo()), expected);
    assert!(
        !fixture.state().join("adopt").exists(),
        "a first adoption keeps no copy"
    );

    // The answer.
    assert_eq!(answer.status, "adopted");
    assert_eq!(answer.class, AdoptClass::Business);
    assert_eq!(answer.package_id, ADOPTED);
    assert!(answer.review.run_through, "{:?}", answer.review);
    assert_eq!(answer.review.record_id.as_deref(), Some(RECORD));
    assert_eq!(answer.rule, Some(adopted_rule(9_000)));
    assert_eq!(answer.requires_added, [BENEFIT_FIXED_VALUE_SUBSYSTEM]);
    assert_eq!(answer.coverage, Some(AdoptCoverage::Complete));
    assert_eq!(answer.written, [ADOPTED_DIRECTORY, "schedule.json"]);
    assert!(answer.would_write.is_empty());
    assert_eq!(answer.kept, None);
    let from = |member: &str| {
        answer
            .derived
            .iter()
            .find(|derived| derived.member == member)
            .map(|derived| derived.from)
    };
    assert_eq!(from("id"), Some(DerivedFrom::Package));
    assert_eq!(from("priority"), Some(DerivedFrom::File));
    assert_eq!(from("feedback_stop"), Some(DerivedFrom::File));
    assert_eq!(from("expected_duration_ms"), Some(DerivedFrom::Package));
    assert_eq!(from("on_failure"), Some(DerivedFrom::Neutral));
    assert_eq!(from("cooldown_ms"), Some(DerivedFrom::File));
    assert_eq!(from("trigger"), None, "the decision gave the trigger");
    // Given: the trigger and the value, a number of the pack's own written without an origin.
    assert_eq!(
        answer
            .given
            .iter()
            .map(|given| (given.member.as_str(), given.origin.as_deref()))
            .collect::<Vec<_>>(),
        [("trigger", None), ("fixed_value_milli", Some("pack"))]
    );
    assert_eq!(answer.given[1].value, 9_000);
    assert!(answer.model_kept.is_empty());
    assert!(
        Path::new(&answer.check_tree)
            .join("installed")
            .join(GAME)
            .join(ADOPTED)
            .join("control.json")
            .is_file(),
        "the check read the installed layout"
    );

    // The post-check: the repository as written bundles and composes.
    let bundled = package_bundle(PackageBundleRequest {
        applications: fixture.repo().join("applications.json"),
        packs_root: fixture.repo().join("packs"),
        out: fixture.path("post-check"),
        source: None,
        maintenance: Some(Vec::new()),
        root_files: StandardPackRootFiles {
            schedule: Some(fixture.repo().join("schedule.json")),
            ..StandardPackRootFiles::default()
        },
    })
    .unwrap_or_else(|error| panic!("the repository as written bundles: {error}"));
    assert!(
        bundled
            .index
            .packs
            .iter()
            .any(|pack| pack.package_id == ADOPTED)
    );
}

#[test]
fn p1_a_second_identical_adoption_is_refused_pack_exists() {
    let fixture = Fixture::new();
    let files = package_files(ADOPTED, SERVER, "Adopted fixture", &provenance("passed"));
    let source = package_dir(&fixture, "package", &files);
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    standard_pack_adopt(
        request(
            &fixture,
            source.clone(),
            Some(decision.clone()),
            false,
            FIRST,
        ),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the first adoption: {error}"));
    let before = tree(&fixture.repo());

    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision), false, SECOND),
            &Committed,
        ),
        AdoptProblem::PackExists,
        &json!({"entry": ADOPTED_DIRECTORY}),
        &fixture,
        &before,
    );
}

#[test]
fn p1_replace_keeps_the_rule_byte_for_byte_and_moves_the_old_directory_to_the_state_directory() {
    let fixture = Fixture::new();
    let first = package_files(ADOPTED, SERVER, "First recording", &provenance("passed"));
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    standard_pack_adopt(
        request(
            &fixture,
            package_dir(&fixture, "first", &first),
            Some(decision),
            false,
            FIRST,
        ),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the first adoption: {error}"));
    let schedule = fs::read(fixture.repo().join("schedule.json")).expect("schedule.json");
    let old = tree(&fixture.repo().join(ADOPTED_DIRECTORY));

    // The repaired recording, adopted again without a decision.
    let second = package_files(ADOPTED, SERVER, "Second recording", &provenance("passed"));
    let answer = standard_pack_adopt(
        request(
            &fixture,
            package_dir(&fixture, "second", &second),
            None,
            true,
            SECOND,
        ),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the adoption again: {error}"));

    assert_eq!(
        fs::read(fixture.repo().join("schedule.json")).expect("schedule.json"),
        schedule,
        "the rule is kept byte for byte"
    );
    assert_eq!(
        tree(&fixture.repo().join(ADOPTED_DIRECTORY)),
        canonical_files(&second)
    );
    let kept = fixture
        .state()
        .join("adopt")
        .join(format!("{ADOPTED}.old-{SECOND}"));
    assert_eq!(tree(&kept), old, "the old directory is kept whole");
    assert_eq!(answer.kept.as_deref().map(Path::new), Some(kept.as_path()));
    assert!(answer.replaced);
    assert_eq!(answer.written, [ADOPTED_DIRECTORY]);
    assert!(
        answer.derived_drift.is_empty(),
        "{:?}",
        answer.derived_drift
    );
}

#[test]
fn p1_a_value_the_model_wrote_is_kept_whatever_a_replace_decides() {
    let fixture = Fixture::new();
    let files = package_files(ADOPTED, SERVER, "Adopted fixture", &provenance("passed"));
    let source = package_dir(&fixture, "package", &files);
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    standard_pack_adopt(
        request(&fixture, source.clone(), Some(decision), false, FIRST),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the first adoption: {error}"));
    // The rule's value as the model's export would write it (the writer the contract names is not
    // built; the file is edited here as it would be).
    let mut schedule = fixture.schedule();
    schedule["rules"][1]["fixed_value_milli"] = json!(5_000);
    schedule["rules"][1]["fixed_value_origin"] = json!("model");
    write(&fixture.repo().join("schedule.json"), &schedule);
    let model_kept = ["fixed_value_milli", "fixed_value_origin"];

    // A decision that names the trigger and not the value; one that names a value of the pack's
    // own; one that names the value pending: the model's value is kept each time, and answered.
    let decisions = [
        (
            "retrigger",
            json!({
                "schema_version": DECISION_SCHEMA,
                "class": "business",
                "trigger": every(3_600_000)
            }),
            SECOND,
        ),
        ("by-pack", business(&fixed(7_000)), THIRD),
        ("pending", business(&json!("pending")), FOURTH),
    ];
    for (name, decision, at_unix_ms) in decisions {
        let decision = decide(&fixture, name, &decision);
        let answer = standard_pack_adopt(
            request(&fixture, source.clone(), Some(decision), true, at_unix_ms),
            &Committed,
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(answer.model_kept, model_kept, "{name}");
        assert!(
            answer
                .given
                .iter()
                .all(|given| given.member != "fixed_value_milli" && given.member != "value"),
            "{name}: {:?}",
            answer.given
        );
        let written = fixture.schedule();
        let rule = &written["rules"][1];
        assert_eq!(rule["fixed_value_milli"], 5_000, "{name}");
        assert_eq!(rule["fixed_value_origin"], "model", "{name}");
        assert_eq!(answer.rule.as_ref(), Some(rule), "{name}");
        // Never copied to another rule.
        assert!(
            written["rules"][0].get("fixed_value_origin").is_none(),
            "{name}"
        );
    }
    assert_eq!(fixture.schedule()["rules"][1]["trigger"], every(86_400_000));
}

#[test]
fn p1_the_package_checks_refuse_by_name_and_write_nothing() {
    let passed = provenance("passed");

    // `--repo` without `maintenance.json`.
    let fixture = Fixture::with_maintenance(false);
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &passed),
    );
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision), false, FIRST),
            &Committed,
        ),
        AdoptProblem::RepositoryInvalid,
        &json!({"entry": "maintenance.json"}),
        &fixture,
        &before,
    );

    // A recording marked again after its last stop.
    let fixture = Fixture::new();
    let mut amended = stopped();
    amended["amended_at_unix_ms"] = json!(5);
    let source = recording(
        &fixture,
        &amended,
        &package_files(ADOPTED, SERVER, "Adopted", &passed),
    );
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision.clone()), false, FIRST),
            &Committed,
        ),
        AdoptProblem::RecordingNotCurrent,
        &json!({"member": "amended_at_unix_ms"}),
        &fixture,
        &before,
    );

    // A package the loader does not admit.
    let broken = fixture.path("broken");
    fs::create_dir_all(&broken).expect("the package directory");
    write(
        &broken.join("control.json"),
        &json!({"schema_version": "Lab-1y.control.v2"}),
    );
    let outcome = assert_refused(
        standard_pack_adopt(
            request(
                &fixture,
                AdoptSource::Package(broken),
                Some(decision.clone()),
                false,
                FIRST,
            ),
            &Committed,
        ),
        AdoptProblem::PackageInvalid,
        &json!({}),
        &fixture,
        &before,
    );
    assert!(
        outcome["causes"]
            .as_array()
            .is_some_and(|causes| !causes.is_empty()),
        "the loader's refusal is the cause: {outcome}"
    );

    // A package of a server `applications.json` does not declare.
    let source = package_dir(
        &fixture,
        "other",
        &package_files(ADOPTED, "other", "Adopted", &passed),
    );
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision.clone()), false, FIRST),
            &Committed,
        ),
        AdoptProblem::PackageMismatch,
        &json!({"member": "server"}),
        &fixture,
        &before,
    );

    // A recording whose cross-check was only partly evaluated, as a business rule.
    let source = package_dir(
        &fixture,
        "partial",
        &package_files(
            ADOPTED,
            SERVER,
            "Adopted",
            &provenance("partially_evaluated"),
        ),
    );
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision.clone()), false, FIRST),
            &Committed,
        ),
        AdoptProblem::RecordingUnreviewed,
        &json!({"package_id": ADOPTED}),
        &fixture,
        &before,
    );

    // A package whose text holds an absolute local path.
    let source = package_dir(
        &fixture,
        "local",
        &package_files(ADOPTED, SERVER, "C:\\ActingCommand\\frame.png", &passed),
    );
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision.clone()), false, FIRST),
            &Committed,
        ),
        AdoptProblem::LocalPath,
        &json!({"entry": "resources/operations/task/task.json", "json_pointer": "/goal"}),
        &fixture,
        &before,
    );

    // `--replace` over a pack directory with a change not committed.
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &passed),
    );
    standard_pack_adopt(
        request(&fixture, source.clone(), Some(decision), false, SECOND),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the first adoption: {error}"));
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(request(&fixture, source, None, true, THIRD), &Changed),
        AdoptProblem::PackDirty,
        &json!({"entry": ADOPTED_DIRECTORY, "count": 1}),
        &fixture,
        &before,
    );
}

#[test]
fn p1_the_decision_checks_refuse_at_their_pointer_and_write_nothing() {
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let before = tree(&fixture.repo());
    let cases = [
        // A first adoption without a decision.
        (
            AdoptProblem::DecisionInvalid,
            None,
            json!({"json_pointer": ""}),
        ),
        // A business rule without its value.
        (
            AdoptProblem::DecisionInvalid,
            Some(json!({
                "schema_version": DECISION_SCHEMA,
                "class": "business",
                "trigger": every(86_400_000)
            })),
            json!({"json_pointer": "/value"}),
        ),
        // A pool the file does not declare.
        (
            AdoptProblem::DecisionInvalid,
            Some(json!({
                "schema_version": DECISION_SCHEMA,
                "class": "business",
                "trigger": every(86_400_000),
                "value": "pending",
                "consumes": [{"pool_id": "pool.neutral.missing", "amount": 1}]
            })),
            json!({"json_pointer": "/consumes/0/pool_id"}),
        ),
        // A member no decision has.
        (
            AdoptProblem::DecisionInvalid,
            Some(json!({
                "schema_version": DECISION_SCHEMA,
                "class": "business",
                "colour": "blue"
            })),
            json!({"json_pointer": "/colour"}),
        ),
        // The coordinator's ruling: a value whose origin the caller says is the model's.
        (
            AdoptProblem::ValueOriginModel,
            Some(business(
                &json!({"fixed_value_milli": 9_000, "origin": "model"}),
            )),
            json!({"json_pointer": "/value/origin"}),
        ),
    ];
    for (index, (problem, decision, values)) in cases.into_iter().enumerate() {
        let decision =
            decision.map(|decision| decide(&fixture, &format!("case-{index}"), &decision));
        assert_refused(
            standard_pack_adopt(
                request(&fixture, source.clone(), decision, false, FIRST),
                &Committed,
            ),
            problem,
            &values,
            &fixture,
            &before,
        );
    }
}

#[test]
fn p1_the_written_form_checks_refuse_by_name_and_write_nothing() {
    // A trigger the composer refuses: `rule_refused` at the composer's pointer into the rule
    // appended, the second of the file.
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let refused = decide(
        &fixture,
        "refused",
        &json!({
            "schema_version": DECISION_SCHEMA,
            "class": "business",
            "trigger": {"kind": "no_such_predicate"},
            "value": fixed(9_000)
        }),
    );
    let before = tree(&fixture.repo());
    let outcome = assert_refused(
        standard_pack_adopt(
            request(&fixture, source.clone(), Some(refused), false, FIRST),
            &Committed,
        ),
        AdoptProblem::RuleRefused,
        &json!({"entry": "schedule.json"}),
        &fixture,
        &before,
    );
    assert!(
        outcome["values"]["json_pointer"]
            .as_str()
            .is_some_and(|pointer| pointer.starts_with("/rules/1")),
        "the composer's pointer into the rule appended: {outcome}"
    );

    // A `schedule.json` adopt must change that is not in the canonical form.
    fs::write(
        fixture.repo().join("schedule.json"),
        serde_json::to_vec(&schedule()).expect("JSON"),
    )
    .expect("schedule.json");
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision), false, SECOND),
            &Committed,
        ),
        AdoptProblem::RootFileNotCanonical,
        &json!({"entry": "schedule.json"}),
        &fixture,
        &before,
    );
}

#[test]
fn p1_the_held_and_the_maintenance_classes() {
    // Held: a recording not run through, kept from scheduling with its reason.
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "partial",
        &package_files(
            ADOPTED,
            SERVER,
            "Partly checked",
            &provenance("partially_evaluated"),
        ),
    );
    let held = decide(
        &fixture,
        "held",
        &json!({
            "schema_version": DECISION_SCHEMA,
            "class": "held",
            "held": {"reason": "Fixture: its cross-check is partly evaluated."}
        }),
    );
    let answer = standard_pack_adopt(
        request(&fixture, source, Some(held), false, FIRST),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the held adoption: {error}"));
    assert_eq!(answer.class, AdoptClass::Held);
    assert!(!answer.review.run_through);
    assert_eq!(answer.rule, None);
    assert_eq!(answer.coverage, None);
    let schedule = fixture.schedule();
    assert_eq!(
        schedule["held"],
        json!([{
            "package_id": ADOPTED,
            "reason": "Fixture: its cross-check is partly evaluated."
        }])
    );
    assert_eq!(schedule["rules"], json!([existing_rule()]));
    assert!(
        fixture
            .repo()
            .join(ADOPTED_DIRECTORY)
            .join("control.json")
            .is_file()
    );

    // Maintenance as a rule: no maintenance rule of the file gives its priority, so the decision
    // names it.
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let unnamed = decide(
        &fixture,
        "unnamed",
        &json!({
            "schema_version": DECISION_SCHEMA,
            "class": "maintenance",
            "trigger": every(3_600_000)
        }),
    );
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source.clone(), Some(unnamed), false, FIRST),
            &Committed,
        ),
        AdoptProblem::DecisionInvalid,
        &json!({"json_pointer": "/overrides/priority"}),
        &fixture,
        &before,
    );
    let named = decide(
        &fixture,
        "named",
        &json!({
            "schema_version": DECISION_SCHEMA,
            "class": "maintenance",
            "trigger": every(3_600_000),
            "overrides": {"priority": 200}
        }),
    );
    let answer = standard_pack_adopt(
        request(&fixture, source, Some(named), false, SECOND),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the maintenance adoption: {error}"));
    assert_eq!(answer.class, AdoptClass::Maintenance);
    assert_eq!(answer.coverage, None);
    let rule = answer.rule.expect("the maintenance rule");
    assert_eq!(rule["maintenance"], true);
    assert_eq!(rule["priority"], 200);
    // No maintenance rule of the file: the outcome predicate on the rule itself with the key
    // the package declares, and the contract's defaults.
    assert_eq!(rule["feedback_stop"]["task_id"], ADOPTED);
    assert_eq!(rule["feedback_stop"]["outcome_key"], OUTCOME_KEY);
    assert_eq!(rule["cooldown_ms"], 60_000);
    assert_eq!(rule["next_run_clamp_ms"], 3_600_000);
    let from = |member: &str| {
        answer
            .derived
            .iter()
            .find(|derived| derived.member == member)
            .map(|derived| derived.from)
    };
    assert_eq!(from("feedback_stop"), Some(DerivedFrom::Package));
    assert_eq!(from("cooldown_ms"), Some(DerivedFrom::Neutral));
    assert_eq!(from("priority"), None, "the decision named it");
    assert_eq!(fixture.schedule()["rules"][1], rule);
}

#[test]
fn p1_a_pending_value_answers_the_coverage_undeclared() {
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let decision = decide(&fixture, "pending", &business(&json!("pending")));
    let answer = standard_pack_adopt(
        request(&fixture, source, Some(decision), false, FIRST),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the adoption: {error}"));
    assert_eq!(answer.coverage, Some(AdoptCoverage::Undeclared));
    assert!(
        answer
            .given
            .iter()
            .any(|given| given.member == "value" && given.value == "pending"),
        "{:?}",
        answer.given
    );
    let rule = answer.rule.expect("the rule");
    assert!(rule.get("fixed_value_milli").is_none(), "{rule}");
    assert!(answer.requires_added.is_empty());
    assert!(
        answer
            .warnings
            .iter()
            .any(|warning| warning.is(SharedCode::StandardPackBenefitUndeclared)),
        "{:?}",
        answer.warnings
    );
}

#[test]
fn p1_a_dry_run_runs_every_check_and_writes_nothing() {
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let before = tree(&fixture.repo());
    let mut dry_run = request(&fixture, source, Some(decision), false, FIRST);
    dry_run.dry_run = true;
    let answer = standard_pack_adopt(dry_run, &Committed)
        .unwrap_or_else(|error| panic!("the dry run: {error}"));
    assert_eq!(answer.status, "validated");
    assert!(answer.dry_run);
    assert!(answer.written.is_empty());
    assert_eq!(answer.would_write, [ADOPTED_DIRECTORY, "schedule.json"]);
    assert_eq!(answer.rule, Some(adopted_rule(9_000)));
    assert_eq!(tree(&fixture.repo()), before, "nothing written");
    assert!(!fixture.state().join("adopt").exists(), "nothing kept");
    assert!(
        Path::new(&answer.check_tree)
            .join("bundle")
            .join("bundle.json")
            .is_file(),
        "the check ran"
    );
}

#[test]
fn p1_a_json_container_and_a_zip_give_the_same_directory() {
    let files = package_files(ADOPTED, SERVER, "Adopted fixture", &provenance("passed"));
    let mut adopted = Vec::new();
    for container in ["zip", "json"] {
        let fixture = Fixture::new();
        let path = fixture.path(&format!("package.{container}"));
        if container == "zip" {
            write_zip(&path, &files);
        } else {
            write_json_container(&path, &files);
        }
        let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
        standard_pack_adopt(
            request(
                &fixture,
                AdoptSource::Package(path),
                Some(decision),
                false,
                FIRST,
            ),
            &Committed,
        )
        .unwrap_or_else(|error| panic!("the {container} container is adopted: {error}"));
        adopted.push((
            tree(&fixture.repo().join(ADOPTED_DIRECTORY)),
            fs::read(fixture.repo().join("schedule.json")).expect("schedule.json"),
        ));
    }
    assert_eq!(adopted[0], adopted[1]);
    assert_eq!(adopted[0].0, canonical_files(&files));
}
