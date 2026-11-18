// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S2b-1 (register §1.477; contracts/standard-pack-authoring.md, "The stages and their
// times"; coordinator work order `rd-1118/brief-463-s2b-adopt-timing.md`): `actinglab
// standard-pack adopt` answers the wall-clock time of each of its stages, a closed list in the
// order a call runs them, and checks the check tree's game root once, its readiness taking that
// check's warnings. On a made-up resource repository of the game `neutral`, server `test`, which
// holds one package and its business rule, as P1's tests make it:
// - `s2b1_a_dry_run_and_a_real_run_answer_every_stage_in_order`: a recording with one regression
//   case, adopted under `--dry-run`, then for real: each answer's `timings` is `{stages,
//   total_ms}`, its stages each `{stage, ms}`, named exactly the closed list in its order, and the
//   worktree was told the same stages in the same order;
// - `s2b1_a_refusal_in_the_root_check_names_it_and_no_later_stage`: another writer spoils the
//   installed layout's `bundle.json` as the root check starts: refused by that check
//   (`standard_pack_root_invalid`, its outcome as before), `details.timings` stopped in
//   `root_check`, its stages the list up to it;
// - `s2b1_a_refusal_in_the_cases_names_it_and_no_later_stage`: the recording's case fails:
//   `case_failed` as before, `details.timings` stopped in `cases`, its stages the list up to it,
//   nothing written;
// - `s2b1_a_root_check_that_warns_answers_as_the_readiness_function_with_its_own_check`: both
//   packages declare one variable differently, which the root check warns of at each declaration:
//   the answer's readiness is the row the readiness function answers with its own root check of the
//   same game root, the answer's warnings hold the adopted package's whole and count the other's,
//   and the written set is the pack directory and `schedule.json`;
// - `s2b1_a_readiness_not_judged_answers_as_the_readiness_function_refuses`: another writer spoils
//   `bundle.json` as the readiness starts: `readiness_not_judged` carries the refusal the readiness
//   function answers for that root and the text of it; the same adoption with the readiness judged
//   answers the same warnings and the same `would_write`, and the readiness function's row;
// - `s2b1_an_adoption_checks_the_root_once`: a dry run and a real run each run one root check (the
//   test seam's count), and the readiness function called alone runs its own.
// No duration is compared: only the member's shape and its stage names. No device, no Runtime, no
// clock read, no sleep: temporary directories and made-up packages.

use crate::standard_pack_check::root_checks_run;
use crate::standard_pack_trial::cause_of;
use crate::{
    AdoptCaseAt, AdoptCaseRun, AdoptCaseVerdict, AdoptCasesRun, AdoptDecision, AdoptLab,
    AdoptSource, AdoptStage, AdoptWorktree, ReadyRefused, RecordingReady, StandardPackAdoptRequest,
    StandardPackAdoptResponse, StandardPackReadyRequest, judge_standard_pack_readiness,
    standard_pack_adopt,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{LabError, LabResult, canonical_json_bytes};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package the repository holds, with its rule.
const EXISTING: &str = "neutral.existing";
/// The package adopted.
const ADOPTED: &str = "neutral.daily";
const ADOPTED_DIRECTORY: &str = "packs/neutral.daily";
const RECORD: &str = "rec-1";
/// The outcome key the made-up packages declare.
const OUTCOME_KEY: &str = "task_done";
const DECISION_SCHEMA: &str = "actingcommand.pack-decision.v1";
/// The recording's one regression case, its file and where an adoption writes it.
const CASE: &str = "first";
const CASE_FILE: &str = "case.json";
const CASE_DIRECTORY: &str = "regression/neutral/neutral.daily.first";
const ROOT_INVALID: &str = "standard_pack_root_invalid";
/// The adoptions' times, which name their check trees and kept copies.
const FIRST: u64 = 1_000;
const SECOND: u64 = 2_000;
/// The closed list of the contract, in its order.
const STAGES: [&str; 14] = [
    "repository",
    "recording",
    "package",
    "checks",
    "check_tree",
    "copy_packages",
    "bundle",
    "installed_tree",
    "root_check",
    "rule_checks",
    "cases",
    "warnings",
    "write",
    "readiness",
];

/// A repository checkout in a temporary directory, its state directory beside it.
struct Fixture {
    temp: TempDir,
}

impl Fixture {
    /// The repository: `applications.json`, `maintenance.json` (`[]`), the package [`EXISTING`]
    /// (declaring `variables` when given) and `schedule.json` with its rule, in the canonical form.
    fn new(variables: Option<&Value>) -> Self {
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
            &package_files(EXISTING, &json!({}), variables),
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
}

/// The Lab of these tests. It cannot read the made-up recordings whole, so the check of a
/// recording's own members judges them, as P1's tests have it; its runner answers each case with
/// `verdict`, the case's `case.json` as the file it read.
struct Runner {
    verdict: AdoptCaseVerdict,
}

impl AdoptLab for Runner {
    fn recording_ready(
        &self,
        _state_dir: &Path,
        _record_id: &str,
    ) -> Result<RecordingReady, ReadyRefused> {
        Err(ReadyRefused::Unreadable(LabError::usage(
            "the recordings of these tests are made up: no reader of the Lab reads them",
        )))
    }

    fn run_cases(&self, _package_dir: &Path, cases: &[AdoptCaseAt]) -> LabResult<AdoptCasesRun> {
        let failed = self.verdict == AdoptCaseVerdict::Failed;
        let runs = cases
            .iter()
            .map(|case| AdoptCaseRun {
                verdict: self.verdict.clone(),
                files: vec![(
                    CASE_FILE.to_owned(),
                    fs::read(case.directory.join(CASE_FILE)).expect("the case's file"),
                )],
            })
            .collect();
        let listed = if failed {
            cases
                .iter()
                .map(|case| json!({"path": case.path, "verdict": "failed"}))
                .collect()
        } else {
            Vec::new()
        };
        Ok(AdoptCasesRun {
            runs,
            not_passed: if failed { cases.len() } else { 0 },
            listed,
        })
    }
}

/// A machine whose checkout has every change committed, with the Lab [`Runner`]. It keeps the
/// stages it is told, in order; given `spoil`, as that stage starts it makes the `bundle.json` of
/// the check tree's game root under that state directory no JSON, as another writer would.
struct Machine {
    runner: Runner,
    seen: RefCell<Vec<AdoptStage>>,
    spoil: Option<(AdoptStage, PathBuf)>,
}

impl Machine {
    fn new(verdict: AdoptCaseVerdict, spoil: Option<(AdoptStage, PathBuf)>) -> Self {
        Self {
            runner: Runner { verdict },
            seen: RefCell::new(Vec::new()),
            spoil,
        }
    }
}

impl AdoptWorktree for Machine {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn lab(&self) -> &dyn AdoptLab {
        &self.runner
    }

    fn stage_started(&self, stage: AdoptStage) {
        self.seen.borrow_mut().push(stage);
        if let Some((at, state)) = &self.spoil
            && *at == stage
        {
            let roots = installed_roots(state);
            assert!(!roots.is_empty(), "a check tree is laid out at {stage:?}");
            for root in roots {
                fs::write(root.join("bundle.json"), b"no JSON")
                    .expect("another writer rewrites bundle.json");
            }
        }
    }
}

/// The game roots the check trees under the state directory `state` laid out.
fn installed_roots(state: &Path) -> Vec<PathBuf> {
    fs::read_dir(state.join("adopt-check"))
        .expect("the check trees")
        .map(|entry| {
            entry
                .expect("a check tree")
                .path()
                .join("installed")
                .join(GAME)
        })
        .filter(|root| root.is_dir())
        .collect()
}

/// Writes `value` in the canonical form.
fn write(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
    fs::write(path, canonical_json_bytes(value).expect("canonical JSON")).expect("the file");
}

/// The provenance of a recording's entry task checked offline: its cross-check passed, its first
/// decision evaluated and no step to review.
fn provenance() -> Value {
    json!({
        "source": "lab_record",
        "record_id": RECORD,
        "generator": "actinglab record stop",
        "steps": [{"step": 1, "click": {"needs_review": false}}],
        "warnings": [],
        "cross_check": {"status": "passed"}
    })
}

/// The switch `extra` with its `default`, a variable a package's control declaration declares.
fn switch(default: bool) -> Value {
    json!({"name": "extra", "type": "switch", "title": {"en": "Switch"}, "default": default})
}

/// The files of the package `package_id` of [`SERVER`]: a route from `home` (a blue first pixel)
/// to `page_b` (a red one), its one tap waiting 200 ms after it, the outcome key [`OUTCOME_KEY`],
/// `provenance` its entry task's provenance and `variables`, when given, what its control declares.
fn package_files(
    package_id: &str,
    provenance: &Value,
    variables: Option<&Value>,
) -> Vec<(String, Value)> {
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
    if let Some(variables) = variables {
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
        "goal": "S2b-1 fixture",
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

/// Writes `files` as a content directory at `directory`, pretty-printed without a final newline.
fn write_directory(directory: &Path, files: &[(String, Value)]) {
    for (relative, value) in files {
        let path = directory.join(relative);
        fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
        fs::write(&path, serde_json::to_vec_pretty(value).expect("JSON")).expect("the file");
    }
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
    write(
        path,
        &json!({
            "schema_version": "actingcommand.package.content-json.v1",
            "files": texts
        }),
    );
}

/// The package [`ADOPTED`] of `files` as a content directory beside the repository.
fn package_dir(fixture: &Fixture, files: &[(String, Value)]) -> AdoptSource {
    let directory = fixture.path("package");
    write_directory(&directory, files);
    AdoptSource::Package(directory)
}

/// The stopped recording [`RECORD`] of the state directory: its last stop's package `files` as a
/// JSON container and, beside it, its one regression case [`CASE`].
fn recording(fixture: &Fixture, files: &[(String, Value)]) -> AdoptSource {
    let lab_dir = fixture
        .state()
        .join("record-artifacts")
        .join(RECORD)
        .join("lab");
    let out = lab_dir.join("out");
    write(
        &lab_dir.join("recording.json"),
        &json!({
            "schema_version": "actingcommand.lab-recording.v1",
            "record_id": RECORD,
            "status": "stopped",
            "artifact": {"container": "json", "path": "out", "package_id": ADOPTED},
            "steps": []
        }),
    );
    write_json_container(&out.join(format!("{ADOPTED}.json")), files);
    write(
        &out.join(format!("{ADOPTED}.cases"))
            .join(CASE)
            .join(CASE_FILE),
        &json!({"package_id": ADOPTED, "source": {"record_id": RECORD, "final": true}}),
    );
    AdoptSource::Recording {
        record_id: RECORD.to_owned(),
        lab_dir,
        session_state_dir: fixture.state(),
    }
}

/// A business decision on the daily clock with a fixed value of the pack's own, written beside the
/// repository.
fn decide(fixture: &Fixture) -> AdoptDecision {
    let path = fixture.path("decision.json");
    let decision = json!({
        "schema_version": DECISION_SCHEMA,
        "class": "business",
        "trigger": every(86_400_000),
        "value": {"fixed_value_milli": 9_000, "origin": "pack"}
    });
    fs::write(&path, serde_json::to_vec_pretty(&decision).expect("JSON")).expect("the decision");
    AdoptDecision::Document(path)
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

/// The repository's `schedule.json` before any adoption: [`EXISTING`]'s business rule (priority
/// 100, hourly, stopped by its own run's end on its declared key).
fn schedule() -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": [{
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
        }]
    })
}

fn request(
    fixture: &Fixture,
    source: AdoptSource,
    decision: AdoptDecision,
    dry_run: bool,
    at_unix_ms: u64,
) -> StandardPackAdoptRequest {
    StandardPackAdoptRequest {
        repo: fixture.repo(),
        source,
        decision: Some(decision),
        replace: false,
        dry_run,
        state_dir: fixture.state(),
        at_unix_ms,
    }
}

/// The answer of the adoption `request` on a machine that spoils nothing, which must be taken.
#[track_caller]
fn adopted(request: StandardPackAdoptRequest) -> StandardPackAdoptResponse {
    standard_pack_adopt(request, &Machine::new(AdoptCaseVerdict::Passed, None))
        .unwrap_or_else(|error| panic!("the adoption is taken: {error}"))
}

/// The game root the check tree of `answer` laid out, `<check_tree>/installed/<game>/`.
fn installed_of(answer: &StandardPackAdoptResponse) -> PathBuf {
    Path::new(&answer.check_tree).join("installed").join(GAME)
}

/// The readiness of the game root `installed` as `standard-pack ready --root` asks it.
fn ready_request(installed: &Path) -> StandardPackReadyRequest {
    StandardPackReadyRequest {
        root: installed.to_path_buf(),
        declared_package: None,
        installation: None,
    }
}

/// The names of the members of `value`, in the order of their names.
fn members(value: &Value) -> Vec<String> {
    let mut names = value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// The stage names of `timings` (the answer's member, or a failed call's `details.timings`), each
/// stage asserted to be `{stage, ms}` with `ms` a whole number, and `total_ms` one too.
#[track_caller]
fn stage_names(timings: &Value) -> Vec<String> {
    assert!(timings["total_ms"].is_u64(), "{timings}");
    timings["stages"]
        .as_array()
        .unwrap_or_else(|| panic!("the stages: {timings}"))
        .iter()
        .map(|stage| {
            assert_eq!(members(stage), ["ms", "stage"], "{stage}");
            assert!(stage["ms"].is_u64(), "{stage}");
            stage["stage"]
                .as_str()
                .unwrap_or_else(|| panic!("a stage's name: {stage}"))
                .to_owned()
        })
        .collect()
}

/// The `standard_pack_root_invalid` warnings of `warnings`, each as it serializes.
fn root_invalid(warnings: &[Outcome]) -> Vec<Value> {
    warnings
        .iter()
        .filter(|warning| warning.code().as_str() == ROOT_INVALID)
        .map(|warning| serde_json::to_value(warning).expect("the warning serializes"))
        .collect()
}

#[test]
fn s2b1_a_dry_run_and_a_real_run_answer_every_stage_in_order() {
    let fixture = Fixture::new(None);
    let source = recording(&fixture, &package_files(ADOPTED, &provenance(), None));
    let decision = decide(&fixture);
    for (dry_run, at_unix_ms, status) in [(true, FIRST, "validated"), (false, SECOND, "adopted")] {
        let machine = Machine::new(AdoptCaseVerdict::Passed, None);
        let answer = standard_pack_adopt(
            request(
                &fixture,
                source.clone(),
                decision.clone(),
                dry_run,
                at_unix_ms,
            ),
            &machine,
        )
        .unwrap_or_else(|error| panic!("the adoption is taken (dry run {dry_run}): {error}"));
        let printed = serde_json::to_value(&answer).expect("the answer serializes");
        assert_eq!(printed["status"], status, "{printed}");
        assert_eq!(printed["cases"]["state"], "taken", "{printed}");
        let timings = &printed["timings"];
        assert_eq!(members(timings), ["stages", "total_ms"], "{timings}");
        assert_eq!(stage_names(timings), STAGES, "dry run {dry_run}");
        // The worktree was told the same stages, each as it started, in the same order.
        let answered = answer
            .timings
            .stages
            .iter()
            .map(|time| time.stage)
            .collect::<Vec<_>>();
        assert_eq!(*machine.seen.borrow(), answered, "dry run {dry_run}");
    }
    assert!(
        fixture
            .repo()
            .join(CASE_DIRECTORY)
            .join(CASE_FILE)
            .is_file(),
        "the real run wrote the case"
    );
}

#[test]
fn s2b1_a_refusal_in_the_root_check_names_it_and_no_later_stage() {
    let fixture = Fixture::new(None);
    let source = package_dir(&fixture, &package_files(ADOPTED, &provenance(), None));
    let machine = Machine::new(
        AdoptCaseVerdict::Passed,
        Some((AdoptStage::RootCheck, fixture.state())),
    );
    let error = standard_pack_adopt(
        request(&fixture, source, decide(&fixture), true, FIRST),
        &machine,
    )
    .expect_err("the root check refuses the spoiled bundle.json");
    // The root check's own refusal, as before.
    assert_eq!(error.code, ROOT_INVALID, "{error}");
    let details = error.details.clone().unwrap_or(Value::Null);
    assert_eq!(details["outcome"]["code"], ROOT_INVALID, "{details}");
    let timings = &details["timings"];
    assert_eq!(
        members(timings),
        ["stages", "stopped_in", "total_ms"],
        "{details}"
    );
    assert_eq!(timings["stopped_in"], "root_check", "{details}");
    assert_eq!(stage_names(timings), &STAGES[..9], "{details}");
}

#[test]
fn s2b1_a_refusal_in_the_cases_names_it_and_no_later_stage() {
    let fixture = Fixture::new(None);
    let source = recording(&fixture, &package_files(ADOPTED, &provenance(), None));
    let error = standard_pack_adopt(
        request(&fixture, source, decide(&fixture), false, FIRST),
        &Machine::new(AdoptCaseVerdict::Failed, None),
    )
    .expect_err("the failing case refuses the adoption");
    let details = error.details.clone().unwrap_or(Value::Null);
    // The refusal of the cases, as before.
    assert_eq!(
        details["outcome"]["values"]["adopt_problem"], "case_failed",
        "{details}"
    );
    assert_eq!(details["failed_total"], 1, "{details}");
    let timings = &details["timings"];
    assert_eq!(timings["stopped_in"], "cases", "{details}");
    assert_eq!(stage_names(timings), &STAGES[..11], "{details}");
    assert!(
        !fixture.repo().join(ADOPTED_DIRECTORY).exists(),
        "nothing written: {details}"
    );
}

#[test]
fn s2b1_a_root_check_that_warns_answers_as_the_readiness_function_with_its_own_check() {
    let fixture = Fixture::new(Some(&json!([switch(false)])));
    let source = package_dir(
        &fixture,
        &package_files(ADOPTED, &provenance(), Some(&json!([switch(true)]))),
    );
    let answer = adopted(request(&fixture, source, decide(&fixture), false, FIRST));

    // The reference: the readiness function called alone on the same game root, which runs its own
    // root check of it.
    let installed = installed_of(&answer);
    let reference = judge_standard_pack_readiness(&ready_request(&installed))
        .unwrap_or_else(|error| panic!("the reference readiness: {error}"));
    let row = reference
        .packs
        .iter()
        .find(|pack| pack.package_id == ADOPTED)
        .expect("the reference's row of the adopted pack");
    assert_eq!(
        serde_json::to_value(answer.readiness.as_ref().expect("the readiness is judged"))
            .expect("the readiness serializes"),
        serde_json::to_value(row).expect("the reference's row serializes")
    );

    // The root check warns of the variable at each declaration: the adopted package's warning is in
    // the answer whole, the other package's counted, as the reference's check gives them.
    let own = root_invalid(&row.warnings);
    assert_eq!(own.len(), 1, "{:?}", row.warnings);
    assert_eq!(root_invalid(&answer.warnings), own, "{:?}", answer.warnings);
    let others = root_invalid(&reference.warnings).len() - own.len();
    assert_eq!(others, 1, "{:?}", reference.warnings);
    assert_eq!(
        answer
            .other_packages_warnings
            .as_ref()
            .and_then(|counted| counted.counts.get(ROOT_INVALID)),
        Some(&others),
        "{:?}",
        answer.other_packages_warnings
    );

    // The written set.
    assert_eq!(answer.written, [ADOPTED_DIRECTORY, "schedule.json"]);
    assert!(
        fixture
            .repo()
            .join(ADOPTED_DIRECTORY)
            .join("control.json")
            .is_file()
    );
}

#[test]
fn s2b1_a_readiness_not_judged_answers_as_the_readiness_function_refuses() {
    let fixture = Fixture::new(None);
    let source = package_dir(&fixture, &package_files(ADOPTED, &provenance(), None));
    let decision = decide(&fixture);

    // Another writer spoils the check tree's bundle.json as the readiness starts.
    let spoiled = standard_pack_adopt(
        request(&fixture, source.clone(), decision.clone(), true, FIRST),
        &Machine::new(
            AdoptCaseVerdict::Passed,
            Some((AdoptStage::Readiness, fixture.state())),
        ),
    )
    .unwrap_or_else(|error| panic!("the adoption is taken: {error}"));
    let installed = installed_of(&spoiled);
    let refusal = judge_standard_pack_readiness(&ready_request(&installed))
        .expect_err("the readiness function refuses the spoiled root");
    assert!(spoiled.readiness.is_none(), "{:?}", spoiled.readiness);
    let not_judged = spoiled
        .readiness_not_judged
        .as_ref()
        .expect("the readiness is not judged");
    assert_eq!(
        serde_json::to_value(&not_judged.outcome).expect("the outcome serializes"),
        serde_json::to_value(cause_of(&refusal)).expect("the outcome serializes")
    );
    assert_eq!(
        not_judged.text,
        format!(
            "the readiness of {ADOPTED} was not judged on the check tree's game root {}: \
             {refusal}; the adoption is answered without it",
            installed.display()
        )
    );

    // The same adoption with the readiness judged: everything else is the same, and the readiness
    // is the reference's row.
    let judged = adopted(request(&fixture, source, decision, true, SECOND));
    let reference = judge_standard_pack_readiness(&ready_request(&installed_of(&judged)))
        .unwrap_or_else(|error| panic!("the reference readiness: {error}"));
    let row = reference
        .packs
        .iter()
        .find(|pack| pack.package_id == ADOPTED)
        .expect("the reference's row of the adopted pack");
    assert_eq!(
        serde_json::to_value(judged.readiness.as_ref().expect("the readiness is judged"))
            .expect("the readiness serializes"),
        serde_json::to_value(row).expect("the reference's row serializes")
    );
    assert_eq!(
        serde_json::to_value(&spoiled.warnings).expect("the warnings serialize"),
        serde_json::to_value(&judged.warnings).expect("the warnings serialize")
    );
    assert_eq!(
        spoiled.other_packages_warnings,
        judged.other_packages_warnings
    );
    assert_eq!(spoiled.would_write, judged.would_write);
    assert_eq!(spoiled.would_write, [ADOPTED_DIRECTORY, "schedule.json"]);
}

#[test]
fn s2b1_an_adoption_checks_the_root_once() {
    let fixture = Fixture::new(None);
    let source = package_dir(&fixture, &package_files(ADOPTED, &provenance(), None));
    let decision = decide(&fixture);
    for (dry_run, at_unix_ms) in [(true, FIRST), (false, SECOND)] {
        let before = root_checks_run();
        let answer = adopted(request(
            &fixture,
            source.clone(),
            decision.clone(),
            dry_run,
            at_unix_ms,
        ));
        assert_eq!(root_checks_run() - before, 1, "dry run {dry_run}");
        assert!(answer.readiness.is_some(), "dry run {dry_run}");
        // The seam counts every root check: the readiness function called alone runs its own.
        let before = root_checks_run();
        judge_standard_pack_readiness(&ready_request(&installed_of(&answer)))
            .unwrap_or_else(|error| panic!("the readiness alone: {error}"));
        assert_eq!(root_checks_run() - before, 1, "dry run {dry_run}");
    }
}
