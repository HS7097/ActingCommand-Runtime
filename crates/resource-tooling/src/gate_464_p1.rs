// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #464 P1 (register §1.377; contracts/standard-pack-authoring.md; coordinator work orders
// `rd-1114/brief-464-p1-adopt.md`, the design's row "P1 adopt" of its section G,
// `rd-1114/brief-1722-fix1.md`, `rd-1115/brief-1722-fix2.md`, `rd-1115/brief-1722-fix3.md` and
// `rd-1115/brief-1722-fix4.md`):
// `actinglab standard-pack adopt`
// on a made-up resource repository of the game `neutral`, server `test`, which holds one package,
// its business rule and a valued pool.
// - a first adoption of a stopped recording writes exactly the pack directory (its JSON canonical)
//   and `schedule.json` (the rule appended with its derived members, its `feedback_stop` from the
//   package's own key and its `expected_duration_ms` at the neutral floor, `requires` gaining
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
// - a pending value answers, with the evaluator's registered reason and in words, that the task
//   is not dispatched until a value is given;
// - `--dry-run` runs every check and writes nothing;
// - a JSON container and a ZIP of the same files give the same directory and the same rule;
// - fix round 1: `--replace` with the state directory on another volume than the checkout is
//   refused before anything is written, the volumes told apart by the test's own machine; an
//   outcome predicate on the rule itself, given (trigger, `feedback_stop`) or kept, on a key the
//   package does not declare is refused at its pointer; the check tree is one marked directory
//   for each checkout, reused and emptied by the next call, and a directory there without its
//   marker is not touched and the call refused;
// - fix round 2: a given `feedback_stop` on one of the package's two keys is written as given, not
//   as the derivation's; an activity target that measures the rule's outcome on a key its package
//   does not declare, and a trigger that reads another task's outcome on a key that task's package
//   does not declare, are refused at their pointer; a business or a held adoption removes the
//   package's `maintenance.json` entry and answers it; `pending` for a follower of a chain is
//   refused; a root file another writer changed after the call read it is not renamed over; the
//   old pack directory is moved back when the new one cannot take its place; a check tree another
//   call holds is refused `check_tree_busy`;
// - fix round 3: after a call stopped between the two renames of the root files, the steps the
//   refusals advise (move the trusted `.part` out, put the pack directory back as it was, adopt
//   with `--replace` and the decision) are each accepted and end in one role; a rule that
//   declares no maintenance beside its package's `maintenance.json` entry is that package's one
//   maintenance role as the composer reads it (adopted again with or without a decision), and a
//   business rule only without that entry;
// - fix round 4: the two roles met after the interrupted call's `.part` was moved out are told
//   with the pack directory committed or absent, as `--replace` found it; a maintenance decision
//   by its uses alone beside the package's entry leaves the rule, and `schedule.json`, as written;
//   of the directories a crash left beside the check tree, named as a call makes it whole in, the
//   next call clears the one with the marker and leaves the one without it, and a link named so
//   is refused `check_tree_link` before anything is cleared.
// No device, no Runtime, no clock read, no sleep: temporary directories and made-up packages (and
// on Windows `mklink`, to make a junction).

use crate::adopt::CHECK_TREE_MARKER;
use crate::codes::AdoptProblem;
use crate::{
    AdoptClass, AdoptCoverage, AdoptDecision, AdoptSource, AdoptWorktree, DerivedFrom,
    PackageBundleRequest, ResourceToolingCode, RootEntryChange, StandardPackAdoptRequest,
    StandardPackAdoptResponse, StandardPackRootFiles, package_bundle, standard_pack_adopt,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{LabError, LabResult, canonical_json_bytes};
use actingcommand_policy::{BENEFIT_FIXED_VALUE_SUBSYSTEM, BenefitGap, PolicyCode};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
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
/// An outcome key no made-up package declares.
const UNDECLARED_KEY: &str = "never_declared";
/// The second key of a normal end of a package of [`two_keys`], which [`EXISTING`]'s package does
/// not declare.
const SECOND_KEY: &str = "task_skipped";
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

/// A machine whose every path below `state` lies on another volume than the checkout, every
/// change of the checkout committed: two volumes the test tells apart on one physical drive.
struct TwoVolumes {
    state: PathBuf,
}

impl AdoptWorktree for TwoVolumes {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn volume(&self, path: &Path) -> io::Result<String> {
        let volume = if path.starts_with(&self.state) {
            "the state directory's volume"
        } else {
            "the checkout's volume"
        };
        Ok(volume.to_owned())
    }
}

/// A machine on which another writer rewrites `schedule.json` (to `bytes`) as soon as the call has
/// put the adopted pack directory in place: after the call read the root files and before it
/// renames them. Every change of the checkout is committed.
struct OtherWriter {
    schedule: PathBuf,
    bytes: Vec<u8>,
}

impl AdoptWorktree for OtherWriter {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)?;
        if to.ends_with(ADOPTED_DIRECTORY) {
            fs::write(&self.schedule, &self.bytes)?;
        }
        Ok(())
    }
}

/// A machine on which the new pack directory cannot take its place (on Windows a file in use
/// there makes the rename fail); every other rename is made. Every change is committed.
struct NewDirectoryInUse;

impl AdoptWorktree for NewDirectoryInUse {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if from.ends_with(format!("{ADOPTED_DIRECTORY}.part")) {
            return Err(io::Error::other("a file of the new directory is in use"));
        }
        fs::rename(from, to)
    }
}

/// Fix round 3: a machine on which the call stops between the two renames of the root files, as a
/// crash there would leave them: the rename of `maintenance.json.part` (the second, for a decision
/// of another class than maintenance) fails; every other rename is made. Every change is
/// committed.
struct StopsBetweenTheRenames;

impl AdoptWorktree for StopsBetweenTheRenames {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if from.ends_with("maintenance.json.part") {
            return Err(io::Error::other("the call stops here"));
        }
        fs::rename(from, to)
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

/// `files` ([`package_files`]) whose entry task declares a second key of a normal end,
/// [`SECOND_KEY`], on the entry page `home`, which no run of its route ends on.
fn two_keys(files: Vec<(String, Value)>) -> Vec<(String, Value)> {
    files
        .into_iter()
        .map(|(relative, mut value)| {
            if relative.ends_with("task.json")
                && let Some(mappings) = value["scheduling_outcome"]["mappings"].as_array_mut()
            {
                mappings.push(json!({
                    "outcome_key": SECOND_KEY,
                    "effect": "no_designated_effect",
                    "terminal_pages": ["home"]
                }));
            }
            (relative, value)
        })
        .collect()
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

/// The rule of [`EXISTING`]: business, priority 100, hourly, no value, stopped by its own run's
/// end on the key its package declares (a key it does not declare would refuse every dispatch).
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

/// The repository's `schedule.json` before any adoption.
fn schedule() -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": [existing_rule()],
        "pools": [pool()]
    })
}

/// An outcome predicate on the adopted rule itself: its own run ended with `key`.
fn own_outcome(key: &str) -> Value {
    outcome_of(ADOPTED, key)
}

/// An outcome predicate: the run of `task_id` ended with `key`.
fn outcome_of(task_id: &str, key: &str) -> Value {
    json!({
        "kind": "outcome",
        "task_id": task_id,
        "outcome_key": key,
        "comparison": "eq",
        "value": {"type": "boolean", "value": true}
    })
}

/// A chain edge: due once the latest run of `task_id` succeeded and this rule has not served it.
fn edge(task_id: &str) -> Value {
    json!({
        "kind": "dependency_completed",
        "task_id": task_id,
        "terminal_states": ["succeeded"],
        "since_own_run": true
    })
}

/// An activity profile active all day on every weekday, without its scope, whose one goal
/// measures `metric`.
fn activity_profile(metric: &Value) -> Value {
    json!({
        "id": "activity.neutral.daily",
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7_200_000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300_000,
            "expected_duration_ms": 60_000
        },
        "minimum_interval_ms": 60_000,
        "maximum_interval_ms": 300_000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1_000,
        "goals": [{
            "id": "goal.neutral.daily",
            "metric": metric,
            "target": 1,
            "deadline_unix_ms": 4_102_444_800_000_u64,
            "strategic_weight_milli": 1_000,
            "best_effort": true
        }]
    })
}

/// The details and the message of a refused or failed call; empty when it was not refused.
fn refusal(result: &LabResult<StandardPackAdoptResponse>) -> (Value, String) {
    match result {
        Err(error) => (
            error.details.clone().unwrap_or(Value::Null),
            error.message.clone(),
        ),
        Ok(_) => (Value::Null, String::new()),
    }
}

/// The outcome a failed call's error carries.
fn outcome_of_error(error: &LabError) -> Value {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .cloned()
        .unwrap_or_else(|| panic!("the error carries its outcome: {error}"))
}

/// The rule a first adoption of [`ADOPTED`] writes with a fixed value of the pack's own: the
/// decision's trigger and value, every other member derived (its `feedback_stop` from the
/// package's one key, its `expected_duration_ms` the neutral floor over the package's 200 ms).
fn adopted_rule(milli: u64) -> Value {
    json!({
        "id": ADOPTED,
        "package_id": ADOPTED,
        "priority": 100,
        "trigger": every(86_400_000),
        "feedback_stop": own_outcome(OUTCOME_KEY),
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
        decision: decision.map(AdoptDecision::Document),
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
    // Never copied from the file's rule: from the package's own declared key.
    assert_eq!(from("feedback_stop"), Some(DerivedFrom::Package));
    // The package's 200 ms of waits raised to the neutral floor.
    assert_eq!(
        from("expected_duration_ms"),
        Some(DerivedFrom::NeutralFloor)
    );
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
fn p1_a_pending_value_answers_that_the_task_waits_for_its_value() {
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
    // In the evaluator's registered reason and in words: not dispatched until a value is given.
    let wait = answer
        .waits_for_value
        .as_ref()
        .expect("the answer says the task waits for its value");
    assert!(
        wait.outcome.is(PolicyCode::BenefitUnknown),
        "{:?}",
        wait.outcome
    );
    assert_eq!(
        wait.outcome.value("benefit_gap"),
        Some(&json!(BenefitGap::NoEffects.as_str())),
        "{:?}",
        wait.outcome
    );
    assert!(
        wait.text
            .contains("is not dispatched until a value is given"),
        "{}",
        wait.text
    );
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

#[test]
fn p1_replace_across_two_volumes_is_refused_before_anything_is_written() {
    let fixture = Fixture::new();
    let files = package_files(ADOPTED, SERVER, "Adopted fixture", &provenance("passed"));
    let source = recording(&fixture, &stopped(), &files);
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    standard_pack_adopt(
        request(&fixture, source.clone(), Some(decision), false, FIRST),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the first adoption: {error}"));
    let before = tree(&fixture.repo());

    // The state directory, and the replaced directory's kept place in it, on another volume.
    let machine = TwoVolumes {
        state: fixture.state(),
    };
    let result = standard_pack_adopt(request(&fixture, source, None, true, SECOND), &machine);
    let message = match &result {
        Err(error) => error.message.clone(),
        Ok(_) => String::new(),
    };
    assert_refused(
        result,
        AdoptProblem::ReplaceAcrossVolumes,
        &json!({"entry": ADOPTED_DIRECTORY}),
        &fixture,
        &before,
    );
    // The way out: a state directory on the checkout's volume, and the recording's package by
    // its path instead of --record-id.
    assert!(message.contains("--state-dir"), "{message}");
    assert!(
        message.contains("--package") && message.contains(&format!("{ADOPTED}.zip")),
        "{message}"
    );
}

#[test]
fn p1_an_outcome_predicate_on_the_rule_itself_needs_a_key_the_package_declares() {
    let fixture = Fixture::new();
    // Fix round 2: a package with two keys of a normal end, so that a given stop on one of them is
    // not the stop the derivation would write (on both, under `any`).
    let source = package_dir(
        &fixture,
        "package",
        &two_keys(package_files(
            ADOPTED,
            SERVER,
            "Adopted",
            &provenance("passed"),
        )),
    );
    let before = tree(&fixture.repo());

    // Given: a trigger (daily while its own run has not ended on a key), and a `feedback_stop`.
    let cases = [
        (
            "trigger",
            json!({
                "schema_version": DECISION_SCHEMA,
                "class": "business",
                "trigger": {"kind": "all", "predicates": [
                    every(86_400_000),
                    {"kind": "not", "predicate": own_outcome(UNDECLARED_KEY)}
                ]},
                "value": fixed(9_000)
            }),
            "/trigger/predicates/1/predicate/outcome_key",
        ),
        (
            "stop",
            json!({
                "schema_version": DECISION_SCHEMA,
                "class": "business",
                "trigger": every(86_400_000),
                "value": fixed(9_000),
                "overrides": {"feedback_stop": own_outcome(UNDECLARED_KEY)}
            }),
            "/overrides/feedback_stop/outcome_key",
        ),
    ];
    for (name, decision, pointer) in cases {
        let decision = decide(&fixture, name, &decision);
        assert_refused(
            standard_pack_adopt(
                request(&fixture, source.clone(), Some(decision), false, FIRST),
                &Committed,
            ),
            AdoptProblem::DecisionInvalid,
            &json!({"json_pointer": pointer}),
            &fixture,
            &before,
        );
    }

    // A given `feedback_stop` on one of the keys the package declares is written as given, not as
    // the derivation would write it.
    let declared = decide(
        &fixture,
        "declared",
        &json!({
            "schema_version": DECISION_SCHEMA,
            "class": "business",
            "trigger": every(86_400_000),
            "value": fixed(9_000),
            "overrides": {"feedback_stop": own_outcome(OUTCOME_KEY)}
        }),
    );
    let answer = standard_pack_adopt(
        request(&fixture, source.clone(), Some(declared), false, SECOND),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("a declared key is adopted: {error}"));
    assert!(
        answer
            .given
            .iter()
            .any(|given| given.member == "feedback_stop" && given.value == own_outcome(OUTCOME_KEY)),
        "{:?}",
        answer.given
    );
    assert!(
        answer
            .derived
            .iter()
            .all(|derived| derived.member != "feedback_stop"),
        "{:?}",
        answer.derived
    );
    let rule = answer.rule.expect("the rule");
    assert_eq!(rule["feedback_stop"], own_outcome(OUTCOME_KEY));

    // Kept: the rule's stop edited to a key the package does not declare, then adopted again
    // without a decision, which keeps the rule as it is.
    let mut schedule = fixture.schedule();
    schedule["rules"][1]["feedback_stop"]["outcome_key"] = json!(UNDECLARED_KEY);
    write(&fixture.repo().join("schedule.json"), &schedule);
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(request(&fixture, source, None, true, THIRD), &Committed),
        AdoptProblem::DecisionInvalid,
        &json!({
            "entry": "schedule.json",
            "json_pointer": "/rules/1/feedback_stop/outcome_key"
        }),
        &fixture,
        &before,
    );
}

#[test]
fn p1_the_check_tree_is_one_marked_scratch_directory_for_each_checkout() {
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let dry_run = |at_unix_ms: u64| {
        let mut dry_run = request(
            &fixture,
            source.clone(),
            Some(decision.clone()),
            false,
            at_unix_ms,
        );
        dry_run.dry_run = true;
        dry_run
    };
    let first = standard_pack_adopt(dry_run(FIRST), &Committed)
        .unwrap_or_else(|error| panic!("the first dry run: {error}"));
    let check_tree = PathBuf::from(&first.check_tree);
    assert!(
        check_tree.starts_with(fixture.state().join("adopt-check")),
        "{}",
        first.check_tree
    );
    assert!(
        check_tree.join(CHECK_TREE_MARKER).is_file(),
        "the tool marked its tree"
    );

    // The next call reuses the one tree, emptied of what an earlier call left, its marker kept.
    fs::write(check_tree.join("left-by-an-earlier-call"), b"scratch").expect("a file in the tree");
    let second = standard_pack_adopt(dry_run(SECOND), &Committed)
        .unwrap_or_else(|error| panic!("the second dry run: {error}"));
    assert_eq!(second.check_tree, first.check_tree);
    assert!(!check_tree.join("left-by-an-earlier-call").exists());
    assert!(check_tree.join(CHECK_TREE_MARKER).is_file());
    assert!(
        check_tree.join("bundle").join("bundle.json").is_file(),
        "the second call laid its tree there"
    );

    // Without its marker the directory is not the tool's: nothing in it is touched, and the call
    // is refused before it writes.
    fs::rename(
        check_tree.join(CHECK_TREE_MARKER),
        fixture.path("marker-moved-away"),
    )
    .expect("the marker moved away");
    let untouched = tree(&check_tree);
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision), false, THIRD),
            &Committed,
        ),
        AdoptProblem::CheckTreeUnmarked,
        &json!({}),
        &fixture,
        &before,
    );
    assert_eq!(tree(&check_tree), untouched, "nothing in it is touched");
}

#[test]
fn p1_an_activity_target_on_a_key_the_package_does_not_declare_is_refused() {
    let fixture = Fixture::new();
    // An activity profile whose goal measures the adopted rule's outcome on a key its package does
    // not declare: the Runtime compares that measure, so it would refuse every dispatch of the
    // rule.
    let mut schedule = schedule();
    schedule["activity"] = json!([activity_profile(&json!({
        "kind": "outcome",
        "task_id": ADOPTED,
        "outcome_key": UNDECLARED_KEY
    }))]);
    write(&fixture.repo().join("schedule.json"), &schedule);
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let before = tree(&fixture.repo());

    let result = standard_pack_adopt(
        request(&fixture, source, Some(decision), false, FIRST),
        &Committed,
    );
    let (details, message) = refusal(&result);
    assert_refused(
        result,
        AdoptProblem::DecisionInvalid,
        &json!({
            "entry": "schedule.json",
            "json_pointer": "/activity/0/goals/0/metric/outcome_key"
        }),
        &fixture,
        &before,
    );
    assert_eq!(details["declared_outcome_keys"], json!([OUTCOME_KEY]));
    assert!(message.contains("activity.neutral.daily"), "{message}");
}

#[test]
fn p1_a_predicate_on_another_tasks_outcome_needs_a_key_that_tasks_package_declares() {
    let fixture = Fixture::new();
    // The adopted package declares both keys; the existing rule's package only the first.
    let source = package_dir(
        &fixture,
        "package",
        &two_keys(package_files(
            ADOPTED,
            SERVER,
            "Adopted",
            &provenance("passed"),
        )),
    );
    let after_existing = |key: &str| {
        json!({
            "schema_version": DECISION_SCHEMA,
            "class": "business",
            "trigger": {"kind": "all", "predicates": [
                every(86_400_000),
                {"kind": "not", "predicate": outcome_of(EXISTING, key)}
            ]},
            "value": fixed(9_000)
        })
    };
    let before = tree(&fixture.repo());

    // The trigger reads the existing rule's outcome on a key its package does not declare.
    let refused = decide(&fixture, "refused", &after_existing(SECOND_KEY));
    let result = standard_pack_adopt(
        request(&fixture, source.clone(), Some(refused), false, FIRST),
        &Committed,
    );
    let (details, _) = refusal(&result);
    assert_refused(
        result,
        AdoptProblem::DecisionInvalid,
        &json!({"json_pointer": "/trigger/predicates/1/predicate/outcome_key"}),
        &fixture,
        &before,
    );
    assert_eq!(details["declared_outcome_keys"], json!([OUTCOME_KEY]));
    assert_eq!(details["task_id"], EXISTING);
    assert_eq!(details["package_id"], EXISTING);

    // On the key its package declares the rule is adopted as given.
    let declared = decide(&fixture, "declared", &after_existing(OUTCOME_KEY));
    let answer = standard_pack_adopt(
        request(&fixture, source, Some(declared), false, SECOND),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("a declared key of the task read is adopted: {error}"));
    assert_eq!(
        answer.rule.expect("the rule")["trigger"],
        after_existing(OUTCOME_KEY)["trigger"]
    );
}

#[test]
fn p1_a_business_or_held_adoption_ends_the_maintenance_entry_and_answers_it() {
    let entry = json!({"package_id": ADOPTED, "server": SERVER, "uses": ["return_home"]});
    let decisions = [
        ("business", business(&fixed(9_000))),
        (
            "held",
            json!({
                "schema_version": DECISION_SCHEMA,
                "class": "held",
                "held": {"reason": "Fixture: kept from scheduling."}
            }),
        ),
    ];
    for (name, decision) in decisions {
        // The package's maintenance entry is an earlier adoption's, so the call is a --replace.
        let fixture = Fixture::new();
        write(&fixture.repo().join("maintenance.json"), &json!([entry]));
        let source = package_dir(
            &fixture,
            "package",
            &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
        );
        let decision = decide(&fixture, name, &decision);
        let answer = standard_pack_adopt(
            request(&fixture, source, Some(decision), true, FIRST),
            &Committed,
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            answer.removed_entries,
            [RootEntryChange {
                file: "maintenance.json".to_owned(),
                json_pointer: "/0".to_owned(),
                before: entry.clone(),
                after: None,
            }],
            "{name}"
        );
        assert_eq!(
            fs::read(fixture.repo().join("maintenance.json")).expect("maintenance.json"),
            canonical_json_bytes(&json!([])).expect("canonical JSON"),
            "{name}: one role"
        );
        assert_eq!(
            answer.written,
            [ADOPTED_DIRECTORY, "schedule.json", "maintenance.json"],
            "{name}"
        );
    }
}

#[test]
fn p1_pending_for_a_follower_of_a_chain_is_refused() {
    let fixture = Fixture::new();
    // The existing rule valued by a fixed value: the head a chain rides with.
    let mut head = existing_rule();
    head["fixed_value_milli"] = json!(5_000);
    write(
        &fixture.repo().join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": [BENEFIT_FIXED_VALUE_SUBSYSTEM],
            "rules": [head],
            "pools": [pool()]
        }),
    );
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    // A rule whose whole trigger is an edge to that head: a follower, dispatched with its head.
    let decision = decide(
        &fixture,
        "pending",
        &json!({
            "schema_version": DECISION_SCHEMA,
            "class": "business",
            "trigger": edge(EXISTING),
            "value": "pending"
        }),
    );
    let before = tree(&fixture.repo());
    let result = standard_pack_adopt(
        request(&fixture, source, Some(decision), false, FIRST),
        &Committed,
    );
    let (_, message) = refusal(&result);
    assert_refused(
        result,
        AdoptProblem::DecisionInvalid,
        &json!({"json_pointer": "/value"}),
        &fixture,
        &before,
    );
    assert!(message.contains("follows its chain's head"), "{message}");
}

#[test]
fn p1_a_root_file_another_writer_changed_is_not_renamed_over() {
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    // Another writer's schedule.json, written after the call read the file.
    let mut theirs = schedule();
    theirs["rules"][0]["cooldown_ms"] = json!(2_000);
    let theirs = canonical_json_bytes(&theirs).expect("canonical JSON");
    let machine = OtherWriter {
        schedule: fixture.repo().join("schedule.json"),
        bytes: theirs.clone(),
    };
    let maintenance = fs::read(fixture.repo().join("maintenance.json")).expect("maintenance.json");

    let error = standard_pack_adopt(
        request(&fixture, source, Some(decision), false, FIRST),
        &machine,
    )
    .expect_err("the call stops before it renames schedule.json");
    let outcome = outcome_of_error(&error);
    assert_eq!(
        outcome["code"],
        ResourceToolingCode::AdoptWriteFailed.as_str(),
        "{error}"
    );
    assert!(
        outcome["values"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("schedule.json")),
        "{error}"
    );
    // The other writer's file stays: nothing is renamed over it, and the file this call computed
    // waits as its .part.
    assert_eq!(
        fs::read(fixture.repo().join("schedule.json")).expect("schedule.json"),
        theirs
    );
    assert!(fixture.repo().join("schedule.json.part").is_file());
    assert_eq!(
        fs::read(fixture.repo().join("maintenance.json")).expect("maintenance.json"),
        maintenance
    );
}

#[test]
fn p1_the_old_directory_is_moved_back_when_the_new_one_cannot_take_its_place() {
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
    let old = tree(&fixture.repo().join(ADOPTED_DIRECTORY));
    let schedule = fs::read(fixture.repo().join("schedule.json")).expect("schedule.json");

    let second = package_files(ADOPTED, SERVER, "Second recording", &provenance("passed"));
    let error = standard_pack_adopt(
        request(
            &fixture,
            package_dir(&fixture, "second", &second),
            None,
            true,
            SECOND,
        ),
        &NewDirectoryInUse,
    )
    .expect_err("the new directory cannot take its place");
    let outcome = outcome_of_error(&error);
    assert_eq!(
        outcome["code"],
        ResourceToolingCode::AdoptWriteFailed.as_str(),
        "{error}"
    );
    assert!(error.message.contains("moved back"), "{error}");
    // The old directory is in its place again, whole; the new one waits as its .part; no copy is
    // kept; the rule is as it was.
    assert_eq!(tree(&fixture.repo().join(ADOPTED_DIRECTORY)), old);
    assert_eq!(
        tree(&fixture.repo().join("packs").join(format!("{ADOPTED}.part"))),
        canonical_files(&second)
    );
    assert!(
        !fixture
            .state()
            .join("adopt")
            .join(format!("{ADOPTED}.old-{SECOND}"))
            .exists()
    );
    assert_eq!(
        fs::read(fixture.repo().join("schedule.json")).expect("schedule.json"),
        schedule
    );
}

#[test]
fn p1_a_check_tree_another_adoption_holds_is_refused_busy() {
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let mut dry_run = request(
        &fixture,
        source.clone(),
        Some(decision.clone()),
        false,
        FIRST,
    );
    dry_run.dry_run = true;
    let first = standard_pack_adopt(dry_run, &Committed)
        .unwrap_or_else(|error| panic!("the dry run: {error}"));
    let check_tree = PathBuf::from(&first.check_tree);
    let laid = tree(&check_tree);

    // Another adoption of the checkout with this state directory holds the tree's marker.
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .open(check_tree.join(CHECK_TREE_MARKER))
        .expect("the marker opens");
    held.lock().expect("the marker is locked");
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision), false, SECOND),
            &Committed,
        ),
        AdoptProblem::CheckTreeBusy,
        &json!({}),
        &fixture,
        &before,
    );
    drop(held);
    assert_eq!(
        tree(&check_tree),
        laid,
        "the tree another call holds is not emptied"
    );
}

#[test]
fn p1_after_an_interruption_between_the_two_renames_the_advised_steps_are_accepted() {
    // A package with a maintenance.json entry, adopted again as held: the call renames
    // schedule.json (its hold) first and maintenance.json (its entry ended) second, and stops
    // between the two.
    let entry = json!({"package_id": ADOPTED, "server": SERVER, "uses": ["startup"]});
    let fixture = Fixture::new();
    write(&fixture.repo().join("maintenance.json"), &json!([entry]));
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let held = decide(
        &fixture,
        "held",
        &json!({
            "schema_version": DECISION_SCHEMA,
            "class": "held",
            "held": {"reason": "Fixture: kept from scheduling."}
        }),
    );
    let error = standard_pack_adopt(
        request(&fixture, source.clone(), Some(held.clone()), true, FIRST),
        &StopsBetweenTheRenames,
    )
    .expect_err("the call stops between the two renames");
    assert_eq!(
        outcome_of_error(&error)["code"],
        ResourceToolingCode::AdoptWriteFailed.as_str(),
        "{error}"
    );
    // What it left: its directory in place, schedule.json renamed, maintenance.json as it was
    // beside its .part.
    assert!(
        fixture
            .repo()
            .join(ADOPTED_DIRECTORY)
            .join("control.json")
            .is_file()
    );
    assert_eq!(fixture.schedule()["held"][0]["package_id"], ADOPTED);
    assert!(fixture.repo().join("maintenance.json.part").is_file());

    // The next call names the leftover, trusts it as maintenance.json whole, and says to move it
    // out and run adopt again with --replace.
    let before = tree(&fixture.repo());
    let result = standard_pack_adopt(
        request(&fixture, source.clone(), Some(held.clone()), true, SECOND),
        &Committed,
    );
    let (_, message) = refusal(&result);
    assert_refused(
        result,
        AdoptProblem::RepositoryInvalid,
        &json!({"entry": "maintenance.json.part"}),
        &fixture,
        &before,
    );
    assert!(
        message.contains("maintenance.json.part is trusted"),
        "{message}"
    );
    assert!(message.contains("--replace"), "{message}");

    // The .part moved out: a call without the decision meets the two roles. Only --replace meets
    // that check, after pack_dirty passed (fix round 4), so the message says the pack directory is
    // committed or absent and to run adopt with --replace and the decision.
    fs::rename(
        fixture.repo().join("maintenance.json.part"),
        fixture.path("maintenance.json.part"),
    )
    .expect("the .part moved out of the checkout");
    let before = tree(&fixture.repo());
    let result = standard_pack_adopt(
        request(&fixture, source.clone(), None, true, THIRD),
        &Committed,
    );
    let (_, message) = refusal(&result);
    assert_refused(
        result,
        AdoptProblem::RepositoryInvalid,
        &json!({"entry": "maintenance.json", "json_pointer": "/0", "package_id": ADOPTED}),
        &fixture,
        &before,
    );
    assert!(
        message.contains(&format!("{ADOPTED_DIRECTORY} is committed or absent")),
        "{message}"
    );
    assert!(
        message.contains("run adopt again with --replace"),
        "{message}"
    );
    assert!(!message.contains("not committed"), "{message}");

    // The pack directory put back as it was (absent before that call), adopt with --replace and
    // the decision is accepted and writes the package's one role.
    fs::rename(
        fixture.repo().join(ADOPTED_DIRECTORY),
        fixture.path("adopted-directory-moved-out"),
    )
    .expect("the directory moved out of the checkout");
    let answer = standard_pack_adopt(
        request(&fixture, source, Some(held), true, FOURTH),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the advised call is accepted: {error}"));
    assert_eq!(answer.class, AdoptClass::Held);
    assert_eq!(
        answer.removed_entries,
        [RootEntryChange {
            file: "maintenance.json".to_owned(),
            json_pointer: "/0".to_owned(),
            before: entry,
            after: None,
        }]
    );
    assert_eq!(
        fs::read(fixture.repo().join("maintenance.json")).expect("maintenance.json"),
        canonical_json_bytes(&json!([])).expect("canonical JSON"),
        "one role"
    );
    assert_eq!(fixture.schedule()["held"][0]["package_id"], ADOPTED);
    assert!(!fixture.repo().join("maintenance.json.part").exists());
    assert!(!fixture.repo().join("schedule.json.part").exists());
}

#[test]
fn p1_a_rule_beside_its_packages_maintenance_entry_is_its_one_maintenance_role() {
    // A hand-written root: the package's rule declares no maintenance and writes no value.
    let mut rule = adopted_rule(1);
    let value = rule
        .as_object_mut()
        .expect("the rule")
        .shift_remove("fixed_value_milli");
    assert_eq!(value, Some(json!(1)));
    let entry = json!({"package_id": ADOPTED, "server": SERVER, "uses": ["startup"]});
    let hand_written = |fixture: &Fixture, maintenance: &Value| {
        write(
            &fixture.repo().join("schedule.json"),
            &json!({
                "schema_version": "actingcommand.schedule.v1",
                "rules": [existing_rule(), rule.clone()],
                "pools": [pool()]
            }),
        );
        write(&fixture.repo().join("maintenance.json"), maintenance);
        write_directory(
            &fixture.repo().join(ADOPTED_DIRECTORY),
            &package_files(ADOPTED, SERVER, "Adopted before", &provenance("passed")),
        );
    };
    let by_uses = json!({
        "schema_version": DECISION_SCHEMA,
        "class": "maintenance",
        "uses": "startup"
    });

    // Without the entry the composer judges the rule by its value: a business rule, beside which
    // a maintenance role by its uses alone is refused.
    let fixture = Fixture::new();
    hand_written(&fixture, &json!([]));
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let decision = decide(&fixture, "uses", &by_uses);
    let before = tree(&fixture.repo());
    assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision), true, FIRST),
            &Committed,
        ),
        AdoptProblem::BusinessRuleInTheWay,
        &json!({"entry": "schedule.json", "json_pointer": "/rules/1", "package_id": ADOPTED}),
        &fixture,
        &before,
    );

    // Beside the package's maintenance.json entry the composer does not judge it: it is part of
    // the package's one maintenance role. Adopted again without a decision, the class is
    // maintenance and both root files are kept byte for byte.
    let fixture = Fixture::new();
    hand_written(&fixture, &json!([entry]));
    let schedule = fs::read(fixture.repo().join("schedule.json")).expect("schedule.json");
    let maintenance = fs::read(fixture.repo().join("maintenance.json")).expect("maintenance.json");
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let answer = standard_pack_adopt(
        request(&fixture, source.clone(), None, true, FIRST),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("one role, adopted again without a decision: {error}"));
    assert_eq!(answer.class, AdoptClass::Maintenance);
    assert_eq!(answer.coverage, None);
    assert_eq!(answer.written, [ADOPTED_DIRECTORY]);
    assert_eq!(
        fs::read(fixture.repo().join("schedule.json")).expect("schedule.json"),
        schedule
    );
    assert_eq!(
        fs::read(fixture.repo().join("maintenance.json")).expect("maintenance.json"),
        maintenance
    );

    // A maintenance role by its uses alone is taken there, its entry kept as it was. Fix round 4:
    // the rule gains no maintenance declaration, which the composer does not need (its package's
    // entry leaves it unjudged already): schedule.json read after the call is byte for byte as it
    // was, and only the pack directory is written.
    let decision = decide(&fixture, "uses", &by_uses);
    let answer = standard_pack_adopt(
        request(&fixture, source, Some(decision), true, SECOND),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("maintenance by its uses alone beside the entry: {error}"));
    assert_eq!(answer.class, AdoptClass::Maintenance);
    assert_eq!(answer.maintenance, Some(entry));
    assert_eq!(
        fs::read(fixture.repo().join("maintenance.json")).expect("maintenance.json"),
        maintenance
    );
    assert_eq!(
        fs::read(fixture.repo().join("schedule.json")).expect("schedule.json"),
        schedule,
        "the rule beside the entry is kept as it was written"
    );
    assert_eq!(fixture.schedule()["rules"][1], rule);
    assert_eq!(fixture.schedule()["rules"][1].get("maintenance"), None);
    assert!(
        answer
            .given
            .iter()
            .all(|given| given.member != "maintenance"),
        "{:?}",
        answer.given
    );
    assert!(
        answer.replaced_entries.is_empty(),
        "{:?}",
        answer.replaced_entries
    );
    assert_eq!(answer.written, [ADOPTED_DIRECTORY]);
}

/// Makes `link` a link to the directory `target`: a symbolic link, or on Windows a junction, which
/// needs no privilege (made by `mklink`, the one program the gate runs).
fn link_directory(link: &Path, target: &Path) {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).expect("the symbolic link");
    }
    #[cfg(windows)]
    {
        // mklink takes plain paths, not the canonical `\\?\` form.
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("run mklink");
        assert!(
            made.status.success(),
            "make the junction: {}",
            String::from_utf8_lossy(&made.stderr)
        );
    }
}

#[test]
fn p1_leftover_temporary_check_trees_cleared_with_the_marker_only_never_through_a_link() {
    let fixture = Fixture::new();
    let source = package_dir(
        &fixture,
        "package",
        &package_files(ADOPTED, SERVER, "Adopted", &provenance("passed")),
    );
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let dry_run = |at_unix_ms: u64| {
        let mut dry_run = request(
            &fixture,
            source.clone(),
            Some(decision.clone()),
            false,
            at_unix_ms,
        );
        dry_run.dry_run = true;
        dry_run
    };
    let first = standard_pack_adopt(dry_run(FIRST), &Committed)
        .unwrap_or_else(|error| panic!("the first dry run: {error}"));
    let check_tree = PathBuf::from(&first.check_tree);
    let parent = check_tree
        .parent()
        .expect("the directory of the check trees")
        .to_path_buf();
    let name = check_tree
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the check tree's name")
        .to_owned();
    // Named as a call makes the tree whole in, `<checkout>.new-<process>-<ms>-<count>`, beside it.
    let staged = |numbers: &str| parent.join(format!("{name}.new-{numbers}"));

    // What crashes left there: one directory with the tool's marker, one without it.
    let marked = staged("1-2-3");
    fs::create_dir(&marked).expect("a marked leftover");
    fs::write(marked.join(CHECK_TREE_MARKER), b"marker").expect("its marker");
    fs::write(marked.join("half-made"), b"scratch").expect("a file in it");
    let unmarked = staged("4-5-6");
    fs::create_dir(&unmarked).expect("an unmarked leftover");
    fs::write(unmarked.join("not-the-tools"), b"data").expect("a file in it");
    let unmarked_before = tree(&unmarked);

    // The next call, which holds the tree, clears the one with the marker and leaves the other.
    let second = standard_pack_adopt(dry_run(SECOND), &Committed)
        .unwrap_or_else(|error| panic!("the second dry run: {error}"));
    assert_eq!(second.check_tree, first.check_tree);
    assert!(!marked.exists(), "the leftover with the marker is cleared");
    assert!(unmarked.is_dir(), "the leftover without the marker is left");
    assert_eq!(tree(&unmarked), unmarked_before, "nothing in it is touched");

    // A link named so is refused before anything is cleared: nothing where it points, not the
    // marked leftover beside it, and not the tree.
    let elsewhere = fixture.path("elsewhere");
    fs::create_dir(&elsewhere).expect("a directory elsewhere");
    fs::write(elsewhere.join(CHECK_TREE_MARKER), b"marker").expect("a marker there");
    fs::write(elsewhere.join("someone-elses"), b"data").expect("a file there");
    let pointed_at = tree(&elsewhere);
    let link_name = format!("{name}.new-7-8-9");
    link_directory(&parent.join(&link_name), &elsewhere);
    let marked = staged("10-11-12");
    fs::create_dir(&marked).expect("another marked leftover");
    fs::write(marked.join(CHECK_TREE_MARKER), b"marker").expect("its marker");
    fs::write(check_tree.join("left-by-an-earlier-call"), b"scratch").expect("a file in the tree");
    let before = tree(&fixture.repo());
    let outcome = assert_refused(
        standard_pack_adopt(
            request(&fixture, source, Some(decision), false, THIRD),
            &Committed,
        ),
        AdoptProblem::CheckTreeLink,
        &json!({}),
        &fixture,
        &before,
    );
    assert!(
        outcome["values"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with(link_name.as_str())),
        "{outcome}"
    );
    assert_eq!(
        tree(&elsewhere),
        pointed_at,
        "nothing where the link points is touched"
    );
    assert!(
        marked.join(CHECK_TREE_MARKER).is_file(),
        "nothing is cleared before the refusal"
    );
    assert!(
        check_tree.join("left-by-an-earlier-call").is_file(),
        "the tree is not emptied"
    );
    assert_eq!(tree(&unmarked), unmarked_before);
}

// Workflow #463 S3-4 (register §1.411; contracts/standard-pack-authoring.md, "A held decision by
// two flags"; coordinator work order `rd-1115/brief-463-s34-adopt-held.md`, the design
// `rd-1115/pack-stage3-design.md` §4.2 "S3-4"): `--held-reason` and `--held-reference` are the
// decision `{schema_version, class: "held", held: {reason, reference?}}` without a document
// (`AdoptDecision::Held`), read by the document's reader.
// - a first adoption by the two texts writes the held entry with both and no rule, byte for byte
//   what the same document writes on a repository of its own, and answers as it; a blank reason is
//   refused as the document's blank reason is (`decision_invalid` at `/held/reason`), nothing
//   written;
// - on a package its business rule already schedules, `--replace` with the reason keeps that rule
//   as written and writes its hold beside it (the one-role table's row `held`), as the document
//   does.

/// The reason and the reference of the held decisions of S3-4.
const HELD_REASON: &str = "Not run through on a real device yet.";
const HELD_REFERENCE: &str = "neutral reference 1";

/// The decision of `--held-reason <reason> [--held-reference <reference>]`.
fn held_by_flags(reason: &str, reference: Option<&str>) -> AdoptDecision {
    AdoptDecision::Held {
        reason: reason.to_owned(),
        reference: reference.map(str::to_owned),
    }
}

/// The decision document the two flags are.
fn held_document(reason: &str, reference: Option<&str>) -> Value {
    let mut held = json!({"reason": reason});
    if let Some(reference) = reference {
        held["reference"] = json!(reference);
    }
    json!({"schema_version": DECISION_SCHEMA, "class": "held", "held": held})
}

/// [`request`] with the decision `decision`.
fn request_with(
    fixture: &Fixture,
    source: AdoptSource,
    decision: AdoptDecision,
    replace: bool,
    at_unix_ms: u64,
) -> StandardPackAdoptRequest {
    StandardPackAdoptRequest {
        decision: Some(decision),
        ..request(fixture, source, None, replace, at_unix_ms)
    }
}

#[test]
fn s34_a_first_adoption_by_the_held_reason_writes_what_the_held_document_writes() {
    // A recording not run through, which only the class held takes.
    let files = package_files(
        ADOPTED,
        SERVER,
        "Not run through",
        &provenance("partially_evaluated"),
    );
    let fixture = Fixture::new();
    let source = package_dir(&fixture, "package", &files);

    // A blank reason, by the flag and by the document: the same refusal, nothing written.
    let before = tree(&fixture.repo());
    for (index, blank) in ["", "   "].into_iter().enumerate() {
        assert_refused(
            standard_pack_adopt(
                request_with(
                    &fixture,
                    source.clone(),
                    held_by_flags(blank, None),
                    false,
                    FIRST,
                ),
                &Committed,
            ),
            AdoptProblem::DecisionInvalid,
            &json!({"json_pointer": "/held/reason"}),
            &fixture,
            &before,
        );
        let document = decide(
            &fixture,
            &format!("blank-{index}"),
            &held_document(blank, None),
        );
        assert_refused(
            standard_pack_adopt(
                request(&fixture, source.clone(), Some(document), false, FIRST),
                &Committed,
            ),
            AdoptProblem::DecisionInvalid,
            &json!({"json_pointer": "/held/reason"}),
            &fixture,
            &before,
        );
    }

    // The two texts on this repository, the document they are on a repository of its own.
    let by_flags = standard_pack_adopt(
        request_with(
            &fixture,
            source,
            held_by_flags(HELD_REASON, Some(HELD_REFERENCE)),
            false,
            SECOND,
        ),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the held adoption by its two texts: {error}"));
    let twin = Fixture::new();
    let document = decide(
        &twin,
        "held",
        &held_document(HELD_REASON, Some(HELD_REFERENCE)),
    );
    let by_document = standard_pack_adopt(
        request(
            &twin,
            package_dir(&twin, "package", &files),
            Some(document),
            false,
            SECOND,
        ),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the held adoption by its document: {error}"));

    // The held entry with both texts, and no rule: byte for byte what the document wrote.
    let entry = json!({
        "package_id": ADOPTED,
        "reason": HELD_REASON,
        "reference": HELD_REFERENCE
    });
    let schedule = fixture.schedule();
    assert_eq!(schedule["held"], Value::Array(vec![entry.clone()]));
    assert_eq!(schedule["rules"], json!([existing_rule()]), "no rule");
    assert_eq!(
        tree(&fixture.repo().join(ADOPTED_DIRECTORY)),
        canonical_files(&files)
    );
    assert_eq!(tree(&fixture.repo()), tree(&twin.repo()));

    // And the answer the document's is.
    for answer in [&by_flags, &by_document] {
        assert_eq!(answer.status, "adopted");
        assert_eq!(answer.class, AdoptClass::Held);
        assert!(!answer.review.run_through, "{:?}", answer.review);
        assert_eq!(answer.rule, None);
        assert_eq!(answer.held.as_ref(), Some(&entry));
        assert_eq!(answer.coverage, None);
        assert_eq!(answer.written, [ADOPTED_DIRECTORY, "schedule.json"]);
    }
    assert_eq!(by_flags.review, by_document.review);
    assert_eq!(by_flags.removed_entries, by_document.removed_entries);
    assert_eq!(by_flags.replaced_entries, by_document.replaced_entries);
    assert_eq!(by_flags.requires_added, by_document.requires_added);
    assert_eq!(by_flags.given, by_document.given);
    assert_eq!(by_flags.derived, by_document.derived);
}

#[test]
fn s34_the_held_reason_on_a_scheduled_package_keeps_its_rule_and_writes_its_hold() {
    let files = package_files(ADOPTED, SERVER, "Adopted fixture", &provenance("passed"));
    let entry = json!({"package_id": ADOPTED, "reason": HELD_REASON});
    let mut written = Vec::new();
    for by_flags in [true, false] {
        // The package scheduled by its business rule.
        let fixture = Fixture::new();
        let source = package_dir(&fixture, "package", &files);
        let scheduled = decide(&fixture, "business", &business(&fixed(9_000)));
        standard_pack_adopt(
            request(&fixture, source.clone(), Some(scheduled), false, FIRST),
            &Committed,
        )
        .unwrap_or_else(|error| panic!("by flags {by_flags}: the business adoption: {error}"));
        let rules = fixture.schedule()["rules"].clone();
        assert_eq!(rules[1]["package_id"], ADOPTED, "by flags {by_flags}");

        // Adopted again as held: by the reason, or by the document it is.
        let held = if by_flags {
            held_by_flags(HELD_REASON, None)
        } else {
            AdoptDecision::Document(decide(&fixture, "held", &held_document(HELD_REASON, None)))
        };
        let answer = standard_pack_adopt(
            request_with(&fixture, source, held, true, SECOND),
            &Committed,
        )
        .unwrap_or_else(|error| panic!("by flags {by_flags}: the held adoption: {error}"));

        // The one-role table's row `held`: its held entry; its rule kept and not composed while
        // the hold lasts; no maintenance.json entry to end.
        let schedule = fixture.schedule();
        assert_eq!(
            schedule["rules"], rules,
            "by flags {by_flags}: the rule is kept"
        );
        assert_eq!(
            schedule["held"],
            Value::Array(vec![entry.clone()]),
            "by flags {by_flags}"
        );
        assert_eq!(answer.class, AdoptClass::Held, "by flags {by_flags}");
        assert_eq!(answer.rule.as_ref(), Some(&rules[1]), "by flags {by_flags}");
        assert_eq!(answer.held.as_ref(), Some(&entry), "by flags {by_flags}");
        assert!(answer.replaced, "by flags {by_flags}");
        assert!(
            answer.removed_entries.is_empty() && answer.replaced_entries.is_empty(),
            "by flags {by_flags}: {:?} {:?}",
            answer.removed_entries,
            answer.replaced_entries
        );
        assert_eq!(
            answer.written,
            [ADOPTED_DIRECTORY, "schedule.json"],
            "by flags {by_flags}"
        );
        assert_eq!(
            fs::read(fixture.repo().join("maintenance.json")).expect("maintenance.json"),
            canonical_json_bytes(&json!([])).expect("canonical JSON"),
            "by flags {by_flags}"
        );
        written.push(tree(&fixture.repo()));
    }
    assert_eq!(
        written[0], written[1],
        "the reason writes what the document writes"
    );
}
