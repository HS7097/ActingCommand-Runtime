// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-12 (register §1.445; contracts/standard-pack-authoring.md, "The trial and the
// two warnings" and "The answer"; coordinator work order
// rd-1116/brief-463-s312-adopt-trial-review.md): `actinglab standard-pack adopt` names its offline
// review for what it is, shows the latest trial of the content it writes, warns before enabling
// and names where a value is registered, on P1's made-up resource repository of the game
// `neutral`, server `test` (one package, its business rule, a valued pool).
// - `s312_the_review_names_the_offline_check_and_no_run_through`: the answer's review carries
//   `recording_checked` and no `run_through`; a recording not checked offline, as a business rule,
//   is refused `recording_unreviewed` as before, its details' review the same, its message saying
//   that the recording was not checked offline.
// - `s312_without_a_record_the_trial_is_none_and_not_tried_is_warned`: no trial record: `trial`
//   `none`, no record, no other content, `not_tried` warned with the package's id; its content
//   prefix is the one a trial's plan measures of the directory the adoption wrote.
// - `s312_a_passed_record_of_this_content_answers_passed_and_not_tried_is_gone`: a passed trial of
//   the adopted directory, then the package adopted again: `passed`, the record's authorization,
//   time, request and end; no `not_tried`.
// - `s312_a_failed_record_of_this_content_answers_failed_and_not_tried_stays`: a failed trial:
//   `failed`, the record's end; `not_tried` warned.
// - `s312_a_record_of_other_content_answers_none_and_says_so`: a passed trial of another content
//   of the same package id: `none`, `other_content` true, `not_tried` warned (under `--dry-run`).
// - `s312_a_decision_that_lifts_a_hold_warns_that_enabling_needs_authorization`: a held first
//   adoption warns no `enabling_needs_authorization`; a business decision that lifts that hold
//   warns it once, with the package's id.
// - `s312_next_names_bindings_json_only_when_the_rule_has_no_value`: a business rule with
//   `"pending"` alone: `next` names `ours/model/bindings.json` with the rule's id, a value binding
//   and a hand entry; with its value: `next` names nothing (under `--dry-run`).
// No device, no Runtime, no clock read, no sleep: temporary directories and made-up packages.

use crate::codes::{AdoptProblem, ResourceToolingCode, TrialState};
use crate::{
    AdoptClass, AdoptDecision, AdoptSource, AdoptWorktree, PackTrialRecord,
    StandardPackAdoptRequest, StandardPackAdoptResponse, StandardPackTrialRequest, TrialRunEnd,
    check_trial_run, plan_standard_pack_trial, standard_pack_adopt, write_trial_record,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{TaskOutcome, canonical_json_bytes};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The package the repository holds, with its rule.
const EXISTING: &str = "neutral.existing";
/// The package adopted.
const ADOPTED: &str = "neutral.daily";
const POOL: &str = "pool.neutral.points";
const RECORD: &str = "rec-1";
/// The outcome key the made-up packages declare.
const OUTCOME_KEY: &str = "task_done";
const DECISION_SCHEMA: &str = "actingcommand.pack-decision.v1";
/// The trial's instance and the reference of the owner's authorization it is sent under.
const INSTANCE: &str = "node-a";
const AUTHORIZATION: &str = "board item 463, comment 0000000012";
/// The adoptions' times, which name their check trees and kept copies.
const FIRST: u64 = 1_000;
const SECOND: u64 = 2_000;
/// When a trial's run ended, which names its record.
const TRIED_AT: u64 = 1_500;
const BINDINGS: &str = "ours/model/bindings.json";

/// A repository checkout in a temporary directory, its state directory beside it.
struct Fixture {
    temp: TempDir,
}

impl Fixture {
    /// The repository: `applications.json`, `maintenance.json` (`[]`), the package [`EXISTING`]
    /// and `schedule.json` ([`schedule`]), the root files in the canonical form.
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
            &package_files(EXISTING, "Existing fixture", &json!({})),
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

    /// The adopted package's directory in the repository.
    fn adopted_directory(&self) -> PathBuf {
        self.repo().join("packs").join(ADOPTED)
    }
}

/// The version control of a checkout whose every change is committed.
struct Committed;

impl AdoptWorktree for Committed {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }
}

/// Writes `value` in the canonical form.
fn write(path: &Path, value: &Value) {
    fs::write(path, canonical_json_bytes(value).expect("canonical JSON")).expect("the file");
}

/// The provenance of a recording's entry task whose cross-check is `cross_check`, its first
/// decision evaluated and no step to review.
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

/// The files of the package `package_id` of [`SERVER`]: a route from `home` (a blue first pixel)
/// to `page_b` (a red one), its one tap waiting 200 ms after it, the outcome key
/// [`OUTCOME_KEY`], `goal` its goal and `provenance` its entry task's provenance.
fn package_files(package_id: &str, goal: &str, provenance: &Value) -> Vec<(String, Value)> {
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

/// The package [`ADOPTED`] whose goal is `goal`, its recording checked offline, as a content
/// directory named `name` beside the repository.
fn package_dir(fixture: &Fixture, name: &str, goal: &str, cross_check: &str) -> AdoptSource {
    let directory = fixture.path(name);
    write_directory(
        &directory,
        &package_files(ADOPTED, goal, &provenance(cross_check)),
    );
    AdoptSource::Package(directory)
}

/// The decision document `decision`, written beside the repository.
fn decide(fixture: &Fixture, name: &str, decision: &Value) -> AdoptDecision {
    let path = fixture.path(&format!("{name}.json"));
    fs::write(&path, serde_json::to_vec_pretty(decision).expect("JSON")).expect("the decision");
    AdoptDecision::Document(path)
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

/// The repository's `schedule.json` before any adoption: [`EXISTING`]'s business rule (priority
/// 100, hourly, stopped by its own run's end on its declared key) and a valued pool.
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
        }],
        "pools": [{
            "id": POOL,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.neutral.points"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
        }]
    })
}

fn request(
    fixture: &Fixture,
    source: AdoptSource,
    decision: Option<AdoptDecision>,
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

/// The answer of the adoption `request`, which must be taken.
#[track_caller]
fn adopted(request: StandardPackAdoptRequest) -> StandardPackAdoptResponse {
    standard_pack_adopt(request, &Committed)
        .unwrap_or_else(|error| panic!("the adoption is taken: {error}"))
}

/// The trial of the package directory `package` under [`AUTHORIZATION`], its records kept in the
/// fixture's state directory.
fn trial_request(fixture: &Fixture, package: &Path) -> StandardPackTrialRequest {
    StandardPackTrialRequest {
        package: package.to_path_buf(),
        instance: INSTANCE.to_owned(),
        authorization: Some(AUTHORIZATION.to_owned()),
        variables: Vec::new(),
        expect: None,
        irreversible: false,
        root: None,
        state_dir: fixture.state(),
    }
}

/// The content prefix a trial's plan measures of the package directory `package`.
#[track_caller]
fn measured(fixture: &Fixture, package: &Path) -> String {
    plan_standard_pack_trial(&trial_request(fixture, package))
        .unwrap_or_else(|error| panic!("the trial's plan: {error}"))
        .plan
        .content_hash_prefix
}

/// Records a trial of the package directory `package` whose run `request_id` ended `end` at
/// [`TRIED_AT`], by the trial's own plan, its run's checks and its writer; the content prefix it
/// recorded.
#[track_caller]
fn record_trial(fixture: &Fixture, package: &Path, request_id: &str, end: TrialRunEnd) -> String {
    let tried = trial_request(fixture, package);
    let answer = plan_standard_pack_trial(&tried)
        .unwrap_or_else(|error| panic!("the trial's plan: {error}"));
    let content = answer.plan.content_hash_prefix.clone();
    let plan = check_trial_run(&answer, &tried)
        .unwrap_or_else(|error| panic!("the trial's run may be sent: {error}"));
    let record = PackTrialRecord::of_run(
        plan,
        TRIED_AT,
        request_id.to_owned(),
        Some(format!("run-{request_id}")),
        end,
    );
    write_trial_record(&fixture.state(), &record)
        .unwrap_or_else(|error| panic!("the trial's record: {error}"));
    content
}

/// A run that ended normally on the target page, with the package's key.
fn ended() -> TrialRunEnd {
    TrialRunEnd {
        outcome: TaskOutcome::Success,
        outcome_key: Some(OUTCOME_KEY.to_owned()),
        final_page: Some("page_b".to_owned()),
        failure_code: None,
        failure_values: Map::new(),
    }
}

/// A run that failed on the entry page.
fn failed() -> TrialRunEnd {
    TrialRunEnd {
        outcome: TaskOutcome::Failure,
        outcome_key: None,
        final_page: Some("home".to_owned()),
        failure_code: None,
        failure_values: Map::new(),
    }
}

/// The warnings of `answer` whose code is `code`.
fn warned(answer: &StandardPackAdoptResponse, code: ResourceToolingCode) -> Vec<&Outcome> {
    answer
        .warnings
        .iter()
        .filter(|warning| warning.is(code))
        .collect()
}

/// Asserts that `answer` warns `code` exactly once, naming [`ADOPTED`].
#[track_caller]
fn warns_once(answer: &StandardPackAdoptResponse, code: ResourceToolingCode) {
    let found = warned(answer, code);
    assert_eq!(found.len(), 1, "{:?}", answer.warnings);
    assert_eq!(
        found[0].value("package_id"),
        Some(&json!(ADOPTED)),
        "{:?}",
        found[0]
    );
}

#[test]
fn s312_the_review_names_the_offline_check_and_no_run_through() {
    let fixture = Fixture::new();
    let source = package_dir(&fixture, "package", "Adopted fixture", "passed");
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let answer = adopted(request(&fixture, source, Some(decision), false, FIRST));
    assert!(answer.review.recording_checked, "{:?}", answer.review);
    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(
        printed["review"]["recording_checked"],
        json!(true),
        "{printed}"
    );
    assert!(printed["review"].get("run_through").is_none(), "{printed}");

    // A recording not checked offline, as a business rule: the same refusal and code, its review
    // the same members, its message saying what was not done.
    let unchecked = Fixture::new();
    let source = package_dir(&unchecked, "package", "Unchecked", "partially_evaluated");
    let decision = decide(&unchecked, "decision", &business(&fixed(9_000)));
    let error = standard_pack_adopt(
        request(&unchecked, source, Some(decision), false, FIRST),
        &Committed,
    )
    .expect_err("a recording not checked offline is adopted only as held");
    let details = error.details.clone().expect("the refusal's details");
    assert_eq!(
        details["outcome"]["code"],
        ResourceToolingCode::AdoptRefused.as_str(),
        "{details}"
    );
    assert_eq!(
        details["outcome"]["values"]["adopt_problem"],
        AdoptProblem::RecordingUnreviewed.as_str(),
        "{details}"
    );
    assert_eq!(
        details["review"]["recording_checked"],
        json!(false),
        "{details}"
    );
    assert!(details["review"].get("run_through").is_none(), "{details}");
    assert!(
        error.message.contains("was not checked offline"),
        "{}",
        error.message
    );
}

#[test]
fn s312_without_a_record_the_trial_is_none_and_not_tried_is_warned() {
    let fixture = Fixture::new();
    let source = package_dir(&fixture, "package", "Adopted fixture", "passed");
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let answer = adopted(request(&fixture, source, Some(decision), false, FIRST));

    let trial = answer.review.trial.clone().expect("the review's trial");
    assert_eq!(trial.state, TrialState::NoRecord, "{trial:?}");
    assert_eq!(trial.record, None, "{trial:?}");
    assert!(!trial.other_content, "{trial:?}");
    assert_eq!(answer.review.trial_not_read, None);
    // The content the adoption wrote, as a trial of that directory measures it.
    assert_eq!(
        trial.content_hash_prefix,
        measured(&fixture, &fixture.adopted_directory())
    );
    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(
        printed["review"]["trial"]["state"],
        json!("none"),
        "{printed}"
    );
    assert_eq!(
        printed["review"]["trial"]["other_content"],
        json!(false),
        "{printed}"
    );
    warns_once(&answer, ResourceToolingCode::NotTried);
}

#[test]
fn s312_a_passed_record_of_this_content_answers_passed_and_not_tried_is_gone() {
    let fixture = Fixture::new();
    let source = package_dir(&fixture, "package", "Adopted fixture", "passed");
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let first = adopted(request(
        &fixture,
        source.clone(),
        Some(decision),
        false,
        FIRST,
    ));
    warns_once(&first, ResourceToolingCode::NotTried);

    let content = record_trial(
        &fixture,
        &fixture.adopted_directory(),
        "req-passed",
        ended(),
    );
    // The same content adopted again, without a decision.
    let answer = adopted(request(&fixture, source, None, true, SECOND));
    let trial = answer.review.trial.clone().expect("the review's trial");
    assert_eq!(trial.state, TrialState::Passed, "{trial:?}");
    assert_eq!(trial.content_hash_prefix, content);
    assert!(!trial.other_content, "{trial:?}");
    let record = trial.record.expect("the record's reference");
    assert_eq!(record.authorization, AUTHORIZATION);
    assert_eq!(record.at_unix_ms, TRIED_AT);
    assert_eq!(record.request_id, "req-passed");
    assert_eq!(record.end, ended());
    assert_eq!(record.matched, None);
    assert!(
        warned(&answer, ResourceToolingCode::NotTried).is_empty(),
        "{:?}",
        answer.warnings
    );
    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(
        printed["review"]["trial"]["state"],
        json!("passed"),
        "{printed}"
    );
    assert_eq!(
        printed["review"]["trial"]["record"]["authorization"],
        json!(AUTHORIZATION),
        "{printed}"
    );
}

#[test]
fn s312_a_failed_record_of_this_content_answers_failed_and_not_tried_stays() {
    let fixture = Fixture::new();
    let source = package_dir(&fixture, "package", "Adopted fixture", "passed");
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    adopted(request(
        &fixture,
        source.clone(),
        Some(decision),
        false,
        FIRST,
    ));

    let content = record_trial(
        &fixture,
        &fixture.adopted_directory(),
        "req-failed",
        failed(),
    );
    let answer = adopted(request(&fixture, source, None, true, SECOND));
    let trial = answer.review.trial.clone().expect("the review's trial");
    assert_eq!(trial.state, TrialState::Failed, "{trial:?}");
    assert_eq!(trial.content_hash_prefix, content);
    let record = trial.record.expect("the record's reference");
    assert_eq!(record.request_id, "req-failed");
    assert_eq!(record.end, failed());
    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(
        printed["review"]["trial"]["state"],
        json!("failed"),
        "{printed}"
    );
    warns_once(&answer, ResourceToolingCode::NotTried);
}

#[test]
fn s312_a_record_of_other_content_answers_none_and_says_so() {
    let fixture = Fixture::new();
    // An older content of the same package id, tried and passed.
    let older = fixture.path("older");
    write_directory(
        &older,
        &package_files(ADOPTED, "Older fixture", &provenance("passed")),
    );
    let other = record_trial(&fixture, &older, "req-older", ended());

    let source = package_dir(&fixture, "package", "Adopted fixture", "passed");
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let mut dry_run = request(&fixture, source, Some(decision), false, FIRST);
    dry_run.dry_run = true;
    let answer = adopted(dry_run);
    let trial = answer.review.trial.clone().expect("the review's trial");
    assert_eq!(trial.state, TrialState::NoRecord, "{trial:?}");
    assert_eq!(trial.record, None, "{trial:?}");
    assert!(trial.other_content, "{trial:?}");
    assert_ne!(trial.content_hash_prefix, other, "{trial:?}");
    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(
        printed["review"]["trial"]["other_content"],
        json!(true),
        "{printed}"
    );
    warns_once(&answer, ResourceToolingCode::NotTried);
}

#[test]
fn s312_a_decision_that_lifts_a_hold_warns_that_enabling_needs_authorization() {
    let fixture = Fixture::new();
    let source = package_dir(&fixture, "package", "Adopted fixture", "passed");
    let held = adopted(request(
        &fixture,
        source.clone(),
        Some(AdoptDecision::Held {
            rationale: "Not tried on a real device yet.".to_owned(),
            reference: None,
        }),
        false,
        FIRST,
    ));
    assert_eq!(held.class, AdoptClass::Held);
    assert!(
        warned(&held, ResourceToolingCode::EnablingNeedsAuthorization).is_empty(),
        "{:?}",
        held.warnings
    );

    // A business decision lifts the hold.
    let decision = decide(&fixture, "decision", &business(&fixed(9_000)));
    let answer = adopted(request(&fixture, source, Some(decision), true, SECOND));
    assert_eq!(answer.class, AdoptClass::Business);
    assert_eq!(answer.held, None);
    warns_once(&answer, ResourceToolingCode::EnablingNeedsAuthorization);
}

#[test]
fn s312_next_names_bindings_json_only_when_the_rule_has_no_value() {
    let fixture = Fixture::new();
    let source = package_dir(&fixture, "package", "Adopted fixture", "passed");

    // "pending" alone: the rule has no value yet.
    let pending = decide(&fixture, "pending", &business(&json!("pending")));
    let mut dry_run = request(&fixture, source.clone(), Some(pending), false, FIRST);
    dry_run.dry_run = true;
    let answer = adopted(dry_run);
    assert!(answer.waits_for_value.is_some(), "{:?}", answer.rule);
    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(
        printed["next"],
        json!([{
            "path": BINDINGS,
            "fixed_value": {
                "rule_id": ADOPTED,
                "value_binding": {
                    "value": "",
                    "file": "schedule.json",
                    "rule_id": ADOPTED,
                    "path": "/fixed_value_milli"
                },
                "hand_entry": {"rule_id": ADOPTED, "status": "", "ref": ""}
            }
        }]),
        "{printed}"
    );

    // With its value: nothing to name.
    let valued = decide(&fixture, "valued", &business(&fixed(9_000)));
    let mut dry_run = request(&fixture, source, Some(valued), false, SECOND);
    dry_run.dry_run = true;
    let answer = adopted(dry_run);
    assert!(
        answer.waits_for_value.is_none(),
        "{:?}",
        answer.waits_for_value
    );
    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(printed["next"], json!([]), "{printed}");
}
