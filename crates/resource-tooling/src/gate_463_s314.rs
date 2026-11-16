// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-14 (register §1.446; contracts/standard-pack-schedule.md, "Where a pack stands:
// `standard-pack status`"; the design rd-1115/pack-stage3-design.md §2.3 and §4.2 "S3-14";
// coordinator work order rd-1116/brief-463-s314-pack-status.md): one read that says where each
// pack of a game root stands. One installed game root of five packs, inside a made-up installation
// directory, and a state directory beside it holding trial records written by the trial's own
// writer:
// - `s314_a_held_pack_can_be_tried_and_its_hold_is_missing`: the held pack is admitted, stands
//   `can_be_tried`, answers `enabled.state` `held`, and its only missing item is S2, failing
//   `held`, with the hold's reason and reference as the schedule's entry writes them;
// - `s314_a_pack_ready_to_place_answers_the_passed_trial_of_its_content`: a pack the readiness
//   answers `ready_to_place` answers its content hash prefix, the trial's own, and the passed
//   trial of that content, which expected the end it reached, with its request, its business end
//   and its time; the train: a later trial of that content that ended normally on another end than
//   the one it expected makes it `failed`, the earlier pass on record;
// - `s314_an_enabled_pack_answers_not_recorded_with_journals_not_read` (the train renamed it; its
//   placement journal, which nothing read, is gone): a pack a rule enables is `enabled` with
//   `not_recorded` and `journals_not_read`;
// - `s314_every_pack_answers_not_recorded_with_journals_not_read` (the train renamed it and its
//   unread journal is gone): every pack, enabled, held or supporting, answers the same two words;
// - `s314_the_counts_add_up`: the root's counts of each `stage_state` and each `enabled.state` are
//   the numbers of rows with each word and add up to the five packs;
// - `s314_package_id_answers_one_pack`: with `package_id` the one row, as the whole answer gives
//   it, and the root's counts; a package id the root does not register is `validation_failed`
//   naming `--package-id` `not_recognised`;
// - `s314_a_trial_record_of_other_content_does_not_count`: a later failed record of another
//   content leaves a pack's passed trial standing, and a pack whose only record names another
//   content answers `none`;
// - `s314_the_read_writes_nothing`: every file and directory of the installation, the game root
//   and the state directory is byte for byte what it was;
// - `s314_conditions_are_what_standard_pack_ready_answers`: each row's `conditions` are the items
//   `standard-pack ready` answers for the same root, and its `missing` the ones that fail.
// The train of S3-12 and S3-14 (register §1.446, "Train"; the coordinator's rulings R1 to R6):
// - `s314_train_a_refused_schedule_is_not_judged`: a `schedule.json` the composer refuses: every
//   pack `not_judged` in `stage_state` and `enabled.state`, no hold shown;
// - `s314_train_a_pack_no_rule_names_is_not_placed`: `not_placed`, `can_be_tried`;
// - `s314_train_a_pack_the_loader_refuses_is_not_admitted`: `not_admitted`;
// - `s314_train_a_failed_run_answers_failed`: the latest trial of a content failed: `failed`, no
//   pass on record;
// - `s314_train_a_supporting_pack_stands_supporting_and_shows_its_hold`: the return-home pack
//   stands `supporting`; held in its supporting role it shows its hold on its own row;
// - `s314_train_repo_measures_the_checkout_pack`: `--repo` measures the checkout's pack as the
//   root's is, answers `null` where the checkout has none, and a `--repo` that is no directory is
//   `validation_failed`;
// - `s314_train_an_unreadable_record_leaves_only_its_pack_not_read`: a file of a record's name
//   that is no record: that pack answers `trial_not_read`, the others are answered.
// Made-up packs only; no device, no Runtime, no clock read, no sleep.

use crate::codes::{
    ReadinessItem, ReadinessProblem, ReadinessState, ResourceToolingCode, TrialRecordProblem,
};
use crate::{
    AuthorizationUnrecorded, EnabledState, EnabledStateCounts, ItemState, PackAuthorization,
    PackStatus, PackTrialRecord, ReadinessHold, StageState, StageStateCounts,
    StandardPackReadiness, StandardPackReadyRequest, StandardPackStatus, StandardPackStatusRequest,
    StandardPackTrialRequest, TrialRecordPlan, TrialRunEnd, TrialState,
    judge_standard_pack_readiness, plan_standard_pack_trial, standard_pack_status, trial_records,
    write_trial_record,
};
use actingcommand_contract::TaskOutcome;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The return-home package of the server, a supporting part no rule dispatches.
const HOME: &str = "neutral.test.home";
/// A pack `schedule.json` holds.
const HELD: &str = "neutral.test.held";
/// A pack a rule enables, with a passed trial of its content.
const TRIED: &str = "neutral.test.tried";
/// A pack a rule enables (a placement may have enabled it; the journals are not read).
const PLACED: &str = "neutral.test.placed";
/// A pack a rule enables, whose only trial record names another content.
const RUNS: &str = "neutral.test.runs";
/// A package the root does not hold.
const ABSENT: &str = "neutral.test.absent";
/// The pool the rules produce into, with a valuation.
const VALUED: &str = "pool.valued";
/// The hold's reason and reference.
const REASON: &str = "waits for its first trial on a real application";
const REFERENCE: &str = "fixture reference 1";
/// The trial's instance and authorization, as the trial records them.
const INSTANCE: &str = "node-a";
const AUTHORIZATION: &str = "board item 463, comment 0000000001";
/// The passed trial of [`TRIED`]'s content: its request, its business end, which it expected, and
/// its time.
const PASSED_REQUEST: &str = "req-1000";
const BUSINESS_END: &str = "completed";
const PASSED_AT: u64 = 1_000;
/// An end a run reached normally that is not the one its trial expected.
const OTHER_END: &str = "abandoned";
/// A content no pack of the root holds.
const OTHER_CONTENT: &str = "0123456789ab";
const VALIDATION_FAILED: &str = "validation_failed";

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("file");
}

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

/// The pack `package_id` in `directory`: a 64x24 page graph whose page `home` is told by a blue
/// first pixel and `page_b` by a red one, one trusted click from `home` to `page_b` (as
/// `gate_463_s36` writes it).
fn write_pack(directory: &Path, package_id: &str) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 64, "height": 24},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "S3-14 fixture",
        "coordinate_space": {"width": 64, "height": 24},
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
    write_json(&directory.join("control.json"), &control);
    write_json(
        &directory.join("resources/operations/resources.json"),
        &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
    );
    write_json(
        &directory.join("resources/operations/task/task.json"),
        &task,
    );
}

/// The rule of `package_id` on an hourly clock that produces one unit into the valued pool (as
/// `gate_463_s36` writes it).
fn rule(package_id: &str) -> Value {
    let id = format!("{package_id}.rule");
    json!({
        "id": id,
        "package_id": package_id,
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
        "consumes": [],
        "produces": [{
            "pool_id": VALUED,
            "direction": "produce",
            "amount": 1,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }],
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

/// The hold of `package_id`, with [`REASON`] and [`REFERENCE`].
fn hold(package_id: &str) -> Value {
    json!({"package_id": package_id, "reason": REASON, "reference": REFERENCE})
}

/// The root's `schedule.json`: `rules`, the valued pool and the holds `held`.
fn schedule(rules: Vec<Value>, held: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": rules,
        "pools": [{
            "id": VALUED,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.valued"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": {
                "name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000
            }
        }],
        "activity": [],
        "timeline": [],
        "held": held
    })
}

/// How a trial's run ended: `outcome`, on `outcome_key` when it has one.
fn end(outcome: TaskOutcome, outcome_key: Option<&str>) -> TrialRunEnd {
    TrialRunEnd {
        outcome,
        outcome_key: outcome_key.map(str::to_owned),
        final_page: None,
        failure_code: None,
        failure_values: Map::new(),
    }
}

/// What the tests read: the made-up installation, its game root, the state directory beside it
/// and the content prefix the trial gives [`TRIED`].
struct Fixture {
    temp: TempDir,
    root: PathBuf,
    state: PathBuf,
    tried_content: String,
}

impl Fixture {
    /// The installation `<temp>/installation`;
    /// its game root `packages/neutral` with the five packs, the three rules, the hold of [`HELD`]
    /// and the server's return-home package; and the state directory `<temp>/state` with three
    /// trial records the trial's writer writes: [`TRIED`]'s content passed at 1000 on the end it
    /// expected, another content of [`TRIED`] failed at 2000, and another content of [`RUNS`]
    /// passed at 1500.
    fn write() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let installation = temp.path().join("installation");
        let root = installation.join("packages").join(GAME);
        let mut listed = Vec::new();
        for package_id in [HOME, HELD, TRIED, PLACED, RUNS] {
            write_pack(&root.join(package_id), package_id);
            listed.push(json!({
                "package_id": package_id, "server": SERVER, "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            }));
        }
        write_json(
            &root.join("bundle.json"),
            &json!({
                "schema_version": "actingcommand.bundle.v4",
                "game": GAME,
                "packs": listed,
                "maintenance": [{"package_id": HOME, "server": SERVER, "uses": ["return_home"]}]
            }),
        );
        write_json(
            &root.join("schedule.json"),
            &schedule(
                vec![rule(TRIED), rule(PLACED), rule(RUNS)],
                vec![hold(HELD)],
            ),
        );
        let state = temp.path().join("state");
        let plan = plan_standard_pack_trial(&StandardPackTrialRequest {
            package: root.join(TRIED),
            instance: INSTANCE.to_owned(),
            authorization: Some(AUTHORIZATION.to_owned()),
            variables: Vec::new(),
            expect: None,
            irreversible: false,
            root: None,
            state_dir: state.clone(),
        })
        .unwrap_or_else(|error| panic!("the trial's plan: {}: {}", error.code, error.message));
        let mut kept = TrialRecordPlan::try_from(plan.plan).expect("a plan with an authorization");
        let tried_content = kept.content_hash_prefix.clone();
        assert_ne!(tried_content, OTHER_CONTENT);
        let mut other = kept.clone();
        other.content_hash_prefix = OTHER_CONTENT.to_owned();
        let mut other_of_runs = other.clone();
        other_of_runs.package_id = RUNS.to_owned();
        other_of_runs.package = root.join(RUNS).display().to_string();
        // The train (R1): the passed trial expected the end it reached, as a trial given
        // `--expect` keeps it in its record's plan.
        kept.expect = Some(BUSINESS_END.to_owned());
        for record in [
            PackTrialRecord::of_run(
                kept,
                PASSED_AT,
                PASSED_REQUEST.to_owned(),
                None,
                end(TaskOutcome::Success, Some(BUSINESS_END)),
            ),
            PackTrialRecord::of_run(
                other,
                2_000,
                "req-2000".to_owned(),
                None,
                end(TaskOutcome::Failure, None),
            ),
            PackTrialRecord::of_run(
                other_of_runs,
                1_500,
                "req-1500".to_owned(),
                None,
                end(TaskOutcome::Success, Some(BUSINESS_END)),
            ),
        ] {
            write_trial_record(&state, &record).unwrap_or_else(|error| {
                panic!("the trial record: {}: {}", error.code, error.message)
            });
        }
        Self {
            temp,
            root,
            state,
            tried_content,
        }
    }

    /// Writes the root's `schedule.json` again with `rules` and the holds `held`.
    fn write_schedule(&self, rules: Vec<Value>, held: Vec<Value>) {
        write_json(&self.root.join("schedule.json"), &schedule(rules, held));
    }

    /// Records a trial of the root's pack `package_id`, by the trial's own plan of its directory
    /// and its writer: under [`AUTHORIZATION`], expecting `expect` (kept in the record's plan as a
    /// trial given `--expect` keeps it), its run `request_id` ended `end` at `at_unix_ms`; the
    /// content prefix it recorded.
    fn record(
        &self,
        package_id: &str,
        expect: Option<&str>,
        at_unix_ms: u64,
        request_id: &str,
        end: TrialRunEnd,
    ) -> String {
        let plan = plan_standard_pack_trial(&StandardPackTrialRequest {
            package: self.root.join(package_id),
            instance: INSTANCE.to_owned(),
            authorization: Some(AUTHORIZATION.to_owned()),
            variables: Vec::new(),
            expect: None,
            irreversible: false,
            root: None,
            state_dir: self.state.clone(),
        })
        .unwrap_or_else(|error| panic!("the trial's plan: {}: {}", error.code, error.message));
        let mut kept = TrialRecordPlan::try_from(plan.plan).expect("a plan with an authorization");
        kept.expect = expect.map(str::to_owned);
        let content = kept.content_hash_prefix.clone();
        let record = PackTrialRecord::of_run(kept, at_unix_ms, request_id.to_owned(), None, end);
        write_trial_record(&self.state, &record)
            .unwrap_or_else(|error| panic!("the trial record: {}: {}", error.code, error.message));
        content
    }

    /// The request of `standard-pack status` for the game root with this state directory.
    fn request(&self, package_id: Option<&str>) -> StandardPackStatusRequest {
        StandardPackStatusRequest {
            root: self.root.clone(),
            repo: None,
            package_id: package_id.map(str::to_owned),
            state_dir: self.state.clone(),
        }
    }

    /// The answer of `standard-pack status`, which must pass.
    fn status(&self, package_id: Option<&str>) -> StandardPackStatus {
        standard_pack_status(&self.request(package_id))
            .unwrap_or_else(|error| panic!("the status: {}: {}", error.code, error.message))
    }

    /// The readiness of the game root as `standard-pack ready --root` asks it.
    fn readiness(&self) -> StandardPackReadiness {
        judge_standard_pack_readiness(&StandardPackReadyRequest {
            root: self.root.clone(),
            declared_package: None,
            installation: None,
        })
        .unwrap_or_else(|error| panic!("the readiness: {}: {}", error.code, error.message))
    }
}

/// The row of the pack `package_id`.
fn row<'a>(answer: &'a StandardPackStatus, package_id: &str) -> &'a PackStatus {
    answer
        .packs
        .iter()
        .find(|row| row.package_id == package_id)
        .unwrap_or_else(|| panic!("{package_id} is answered: {answer:?}"))
}

/// Every directory and file below `directory`, each file with its bytes.
fn tree(directory: &Path, into: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
    for entry in fs::read_dir(directory).expect("a directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            tree(&path, into);
            into.insert(path, None);
        } else {
            let bytes = fs::read(&path).expect("a file");
            into.insert(path, Some(bytes));
        }
    }
}

#[test]
fn s314_a_held_pack_can_be_tried_and_its_hold_is_missing() {
    let fixture = Fixture::write();

    let answer = fixture.status(None);

    let held = row(&answer, HELD);
    assert!(held.formed.admitted, "{held:?}");
    assert_eq!(held.stage_state, StageState::CanBeTried);
    assert_eq!(held.enabled.state, EnabledState::Held);
    assert_eq!(held.missing.len(), 1, "{held:?}");
    let missing = &held.missing[0];
    assert_eq!(missing.answer.item, ReadinessItem::Classified);
    assert_eq!(missing.answer.state, ItemState::Fails);
    assert_eq!(
        missing
            .answer
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.value("readiness_problem")),
        Some(&json!(ReadinessProblem::Held.as_str())),
        "{missing:?}"
    );
    assert_eq!(
        missing.held,
        Some(ReadinessHold {
            rationale: REASON.to_owned(),
            reference: Some(REFERENCE.to_owned()),
        })
    );
    let serialized = serde_json::to_value(held).expect("the row");
    assert_eq!(serialized["stage_state"], "can_be_tried");
    assert_eq!(serialized["enabled"]["state"], "held");
    assert_eq!(serialized["missing"][0]["id"], "S2");
    assert_eq!(serialized["missing"][0]["item"], "classified");
    assert_eq!(
        serialized["missing"][0]["held"],
        json!({"reason": REASON, "reference": REFERENCE})
    );
}

#[test]
fn s314_a_pack_ready_to_place_answers_the_passed_trial_of_its_content() {
    let fixture = Fixture::write();
    let readiness = fixture.readiness();
    let ready = readiness
        .packs
        .iter()
        .find(|pack| pack.package_id == TRIED)
        .unwrap_or_else(|| panic!("{TRIED} is judged: {readiness:?}"));
    assert_eq!(ready.readiness, ReadinessState::ReadyToPlace, "{ready:?}");

    let answer = fixture.status(None);

    let tried = row(&answer, TRIED);
    assert!(tried.formed.admitted, "{tried:?}");
    assert_eq!(
        tried.formed.content_hash_prefix.as_deref(),
        Some(fixture.tried_content.as_str())
    );
    assert!(tried.missing.is_empty(), "{tried:?}");
    let trial = tried.trial.as_ref().expect("the trial of its content");
    assert_eq!(trial.state, TrialState::Passed);
    assert_eq!(trial.content_hash_prefix, fixture.tried_content);
    assert!(trial.passed_on_record, "{trial:?}");
    let record = trial.record.as_ref().expect("its latest record");
    assert_eq!(record.request_id, PASSED_REQUEST);
    assert_eq!(record.at_unix_ms, PASSED_AT);
    assert_eq!(record.end.outcome_key.as_deref(), Some(BUSINESS_END));
    // The train (R1): the trial expected the end it reached.
    assert_eq!(record.matched, Some(true));
    let serialized = serde_json::to_value(tried).expect("the row");
    assert_eq!(serialized["trial"]["state"], "passed");
    assert_eq!(serialized["trial"]["record"]["end"]["outcome"], "success");
    assert_eq!(
        serialized["trial"]["record"]["end"]["outcome_key"],
        BUSINESS_END
    );
    assert_eq!(serialized["trial"]["record"]["matched"], json!(true));

    // The train (R1): a later trial of the same content that ended normally, but on another end
    // than the one it expected, did not pass; the earlier pass stays on record.
    let content = fixture.record(
        TRIED,
        Some(BUSINESS_END),
        3_000,
        "req-3000",
        end(TaskOutcome::Success, Some(OTHER_END)),
    );
    assert_eq!(content, fixture.tried_content);

    let answer = fixture.status(Some(TRIED));

    let trial = row(&answer, TRIED)
        .trial
        .as_ref()
        .expect("the trial of its content");
    assert_eq!(trial.state, TrialState::Failed, "{trial:?}");
    assert!(trial.passed_on_record, "{trial:?}");
    let record = trial.record.as_ref().expect("its latest record");
    assert_eq!(record.request_id, "req-3000");
    assert_eq!(record.end.outcome, TaskOutcome::Success);
    assert_eq!(record.matched, Some(false));
}

#[test]
fn s314_an_enabled_pack_answers_not_recorded_with_journals_not_read() {
    let fixture = Fixture::write();

    let answer = fixture.status(None);

    let placed = row(&answer, PLACED);
    assert_eq!(placed.stage_state, StageState::Enabled);
    assert_eq!(placed.enabled.state, EnabledState::Enabled);
    assert_eq!(placed.enabled.authorization, PackAuthorization::NotRecorded);
    assert_eq!(
        placed.enabled.authorization_unrecorded,
        AuthorizationUnrecorded::JournalsNotRead
    );
    let serialized = serde_json::to_value(placed).expect("the row");
    assert_eq!(
        serialized["enabled"],
        json!({
            "state": "enabled",
            "authorization": "not_recorded",
            "authorization_unrecorded": "journals_not_read"
        })
    );
}

#[test]
fn s314_every_pack_answers_not_recorded_with_journals_not_read() {
    let fixture = Fixture::write();

    let answer = fixture.status(None);

    assert_eq!(answer.packs.len(), 5, "{answer:?}");
    for pack in &answer.packs {
        assert_eq!(
            pack.enabled.authorization,
            PackAuthorization::NotRecorded,
            "{}",
            pack.package_id
        );
        assert_eq!(
            pack.enabled.authorization_unrecorded,
            AuthorizationUnrecorded::JournalsNotRead,
            "{}",
            pack.package_id
        );
    }
    let runs = row(&answer, RUNS);
    assert_eq!(runs.stage_state, StageState::Enabled);
    assert_eq!(runs.enabled.state, EnabledState::Enabled);
}

#[test]
fn s314_the_counts_add_up() {
    let fixture = Fixture::write();

    let answer = fixture.status(None);

    assert_eq!(answer.packs.len(), 5, "{answer:?}");
    // The train (R5): the return-home pack stands `supporting`, no longer `can_be_tried`.
    assert_eq!(
        answer.stage_states,
        StageStateCounts {
            not_admitted: 0,
            not_judged: 0,
            can_be_tried: 1,
            supporting: 1,
            enabled: 3,
        }
    );
    assert_eq!(
        answer.enabled_states,
        EnabledStateCounts {
            enabled: 3,
            held: 1,
            not_placed: 0,
            not_scheduled: 1,
            not_judged: 0,
        }
    );
    assert_eq!(row(&answer, HOME).enabled.state, EnabledState::NotScheduled);
    let serialized = serde_json::to_value(&answer).expect("the answer");
    for state in StageState::ALL {
        let rows = answer
            .packs
            .iter()
            .filter(|row| row.stage_state == *state)
            .count();
        assert_eq!(
            serialized["stage_states"][state.as_str()],
            json!(rows),
            "{serialized}"
        );
    }
    for state in EnabledState::ALL {
        let rows = answer
            .packs
            .iter()
            .filter(|row| row.enabled.state == *state)
            .count();
        assert_eq!(
            serialized["enabled_states"][state.as_str()],
            json!(rows),
            "{serialized}"
        );
    }
}

#[test]
fn s314_package_id_answers_one_pack() {
    let fixture = Fixture::write();
    let whole = fixture.status(None);

    let one = fixture.status(Some(HELD));

    assert_eq!(one.packs.len(), 1, "{one:?}");
    assert_eq!(
        serde_json::to_value(&one.packs[0]).expect("the row"),
        serde_json::to_value(row(&whole, HELD)).expect("the row")
    );
    assert_eq!(one.stage_states, whole.stage_states);
    assert_eq!(one.enabled_states, whole.enabled_states);

    let error = standard_pack_status(&fixture.request(Some("neutral.test.absent")))
        .expect_err("a package id the root does not register");
    assert_eq!(error.code, VALIDATION_FAILED, "{}", error.message);
    let details = error.details.clone().expect("the refusal's details");
    assert_eq!(details["argument"], "--package-id", "{details}");
    assert_eq!(details["argument_problem"], "not_recognised", "{details}");
}

#[test]
fn s314_a_trial_record_of_other_content_does_not_count() {
    let fixture = Fixture::write();
    assert_eq!(
        trial_records(&fixture.state, TRIED)
            .expect("the records of the tried pack")
            .len(),
        2
    );
    assert_eq!(
        trial_records(&fixture.state, RUNS)
            .expect("the records of the pack that runs")
            .len(),
        1
    );

    let answer = fixture.status(None);

    // The later record of the tried pack failed, but of another content.
    let tried = row(&answer, TRIED).trial.as_ref().expect("a trial");
    assert_eq!(tried.state, TrialState::Passed);
    assert_eq!(
        tried.record.as_ref().map(|record| record.at_unix_ms),
        Some(PASSED_AT)
    );
    assert!(tried.other_content, "{tried:?}");
    // The only record of the pack that runs names another content.
    let runs_row = row(&answer, RUNS);
    let runs = runs_row.trial.as_ref().expect("a trial");
    assert_eq!(runs.state, TrialState::NoRecord);
    assert_eq!(runs.record, None);
    let serialized = serde_json::to_value(runs_row).expect("the row");
    assert_eq!(
        serialized["trial"],
        json!({
            "state": "none",
            "content_hash_prefix": runs_row.formed.content_hash_prefix,
            "passed_on_record": false,
            "other_content": true
        })
    );
}

#[test]
fn s314_the_read_writes_nothing() {
    let fixture = Fixture::write();
    let mut before = BTreeMap::new();
    tree(fixture.temp.path(), &mut before);

    fixture.status(None);
    fixture.status(Some(TRIED));

    let mut after = BTreeMap::new();
    tree(fixture.temp.path(), &mut after);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    assert!(before == after, "a file changed");
}

#[test]
fn s314_conditions_are_what_standard_pack_ready_answers() {
    let fixture = Fixture::write();
    let readiness = fixture.readiness();

    let answer = fixture.status(None);

    assert_eq!(
        answer
            .packs
            .iter()
            .map(|row| row.package_id.as_str())
            .collect::<Vec<_>>(),
        readiness
            .packs
            .iter()
            .map(|pack| pack.package_id.as_str())
            .collect::<Vec<_>>()
    );
    for pack in &readiness.packs {
        let status = row(&answer, &pack.package_id);
        assert_eq!(status.server, pack.server);
        assert_eq!(
            serde_json::to_value(&status.conditions).expect("the conditions"),
            serde_json::to_value(&pack.items).expect("the items"),
            "{}",
            pack.package_id
        );
        let failing = pack
            .items
            .iter()
            .filter(|item| item.state == ItemState::Fails)
            .map(|item| serde_json::to_value(item).expect("an item"))
            .collect::<Vec<_>>();
        let missing = status
            .missing
            .iter()
            .map(|missing| serde_json::to_value(&missing.answer).expect("an item"))
            .collect::<Vec<_>>();
        assert_eq!(missing, failing, "{}", pack.package_id);
    }
}

#[test]
fn s314_train_a_refused_schedule_is_not_judged() {
    let fixture = Fixture::write();
    // A rule of a package the root does not hold: the composer refuses the file.
    fixture.write_schedule(
        vec![rule(TRIED), rule(PLACED), rule(RUNS), rule(ABSENT)],
        vec![hold(HELD)],
    );

    let answer = fixture.status(None);

    assert_eq!(answer.packs.len(), 5, "{answer:?}");
    for pack in &answer.packs {
        assert!(pack.formed.admitted, "{}", pack.package_id);
        assert_eq!(
            pack.enabled.state,
            EnabledState::NotJudged,
            "{}",
            pack.package_id
        );
        assert_eq!(
            pack.stage_state,
            StageState::NotJudged,
            "{}",
            pack.package_id
        );
        assert_eq!(pack.held, None, "{}", pack.package_id);
    }
    assert_eq!(
        answer.stage_states,
        StageStateCounts {
            not_judged: 5,
            ..StageStateCounts::default()
        }
    );
    assert_eq!(
        answer.enabled_states,
        EnabledStateCounts {
            not_judged: 5,
            ..EnabledStateCounts::default()
        }
    );
    let serialized = serde_json::to_value(row(&answer, TRIED)).expect("the row");
    assert_eq!(serialized["stage_state"], "not_judged");
    assert_eq!(serialized["enabled"]["state"], "not_judged");
}

#[test]
fn s314_train_a_pack_no_rule_names_is_not_placed() {
    let fixture = Fixture::write();
    // No rule names the pack that ran, and nothing holds it.
    fixture.write_schedule(vec![rule(TRIED), rule(PLACED)], vec![hold(HELD)]);

    let answer = fixture.status(None);

    let runs = row(&answer, RUNS);
    assert!(runs.formed.admitted, "{runs:?}");
    assert_eq!(runs.enabled.state, EnabledState::NotPlaced);
    assert_eq!(runs.stage_state, StageState::CanBeTried);
    assert!(
        runs.missing
            .iter()
            .any(|missing| missing.answer.item == ReadinessItem::Classified),
        "{runs:?}"
    );
    assert_eq!(answer.enabled_states.not_placed, 1, "{answer:?}");
    let serialized = serde_json::to_value(runs).expect("the row");
    assert_eq!(serialized["enabled"]["state"], "not_placed");
    assert_eq!(serialized["stage_state"], "can_be_tried");
}

#[test]
fn s314_train_a_pack_the_loader_refuses_is_not_admitted() {
    let fixture = Fixture::write();
    // The pack's task is no task: the Runtime's loader refuses the pack.
    write_json(
        &fixture
            .root
            .join(PLACED)
            .join("resources/operations/task/task.json"),
        &json!({}),
    );

    let answer = fixture.status(None);

    let placed = row(&answer, PLACED);
    assert!(!placed.formed.admitted, "{placed:?}");
    assert_eq!(placed.stage_state, StageState::NotAdmitted);
    let admitted = placed
        .conditions
        .iter()
        .find(|item| item.item == ReadinessItem::Admitted)
        .expect("its S1");
    assert_eq!(admitted.state, ItemState::Fails, "{admitted:?}");
    assert_eq!(answer.stage_states.not_admitted, 1, "{answer:?}");
    let serialized = serde_json::to_value(placed).expect("the row");
    assert_eq!(serialized["stage_state"], "not_admitted");
}

#[test]
fn s314_train_a_failed_run_answers_failed() {
    let fixture = Fixture::write();
    let content = fixture.record(
        PLACED,
        None,
        3_000,
        "req-failed",
        end(TaskOutcome::Failure, None),
    );

    let answer = fixture.status(Some(PLACED));

    let placed = row(&answer, PLACED);
    assert_eq!(
        placed.formed.content_hash_prefix.as_deref(),
        Some(content.as_str())
    );
    let trial = placed.trial.as_ref().expect("the trial of its content");
    assert_eq!(trial.state, TrialState::Failed);
    assert!(!trial.passed_on_record, "{trial:?}");
    assert_eq!(
        trial
            .record
            .as_ref()
            .map(|record| record.request_id.as_str()),
        Some("req-failed")
    );
    let serialized = serde_json::to_value(placed).expect("the row");
    assert_eq!(serialized["trial"]["state"], "failed");
    assert_eq!(serialized["trial"]["passed_on_record"], json!(false));
}

#[test]
fn s314_train_a_supporting_pack_stands_supporting_and_shows_its_hold() {
    let fixture = Fixture::write();

    let answer = fixture.status(None);

    let home = row(&answer, HOME);
    assert!(home.formed.admitted, "{home:?}");
    assert_eq!(home.enabled.state, EnabledState::NotScheduled);
    assert_eq!(home.stage_state, StageState::Supporting);
    assert_eq!(home.held, None);
    assert_eq!(answer.stage_states.supporting, 1, "{answer:?}");
    let serialized = serde_json::to_value(home).expect("the row");
    assert_eq!(serialized["stage_state"], "supporting");
    // The held pack shows its hold on its row too.
    assert_eq!(
        row(&answer, HELD).held,
        Some(ReadinessHold {
            rationale: REASON.to_owned(),
            reference: Some(REFERENCE.to_owned()),
        })
    );

    // Held in its supporting role: its S2 holds by its role, so the readiness's row names no
    // hold; the status row shows it.
    fixture.write_schedule(
        vec![rule(TRIED), rule(PLACED), rule(RUNS)],
        vec![hold(HELD), hold(HOME)],
    );
    let judged = fixture.readiness();
    let home_judged = judged
        .packs
        .iter()
        .find(|pack| pack.package_id == HOME)
        .unwrap_or_else(|| panic!("{HOME} is judged: {judged:?}"));
    assert_eq!(home_judged.held, None, "{home_judged:?}");

    let answer = fixture.status(None);

    let home = row(&answer, HOME);
    assert_eq!(home.enabled.state, EnabledState::NotScheduled);
    assert_eq!(home.stage_state, StageState::Supporting);
    assert_eq!(
        home.held,
        Some(ReadinessHold {
            rationale: REASON.to_owned(),
            reference: Some(REFERENCE.to_owned()),
        })
    );
    let serialized = serde_json::to_value(home).expect("the row");
    assert_eq!(
        serialized["held"],
        json!({"reason": REASON, "reference": REFERENCE})
    );
}

#[test]
fn s314_train_repo_measures_the_checkout_pack() {
    let fixture = Fixture::write();
    let checkout = fixture.temp.path().join("checkout");
    write_pack(&checkout.join("packs").join(TRIED), TRIED);
    let mut request = fixture.request(None);
    request.repo = Some(checkout);

    let answer = standard_pack_status(&request)
        .unwrap_or_else(|error| panic!("the status: {}: {}", error.code, error.message));

    assert!(answer.repo.is_some(), "{answer:?}");
    let tried = row(&answer, TRIED)
        .formed
        .checkout
        .as_ref()
        .expect("the checkout's pack");
    assert!(
        Path::new(&tried.pack).ends_with(Path::new("packs").join(TRIED)),
        "{tried:?}"
    );
    // The same bytes as the root's pack: the same content.
    assert_eq!(
        tried.content_hash_prefix.as_deref(),
        Some(fixture.tried_content.as_str())
    );
    let held = row(&answer, HELD)
        .formed
        .checkout
        .as_ref()
        .expect("the checkout's place of the pack");
    assert_eq!(held.content_hash_prefix, None, "{held:?}");
    let serialized = serde_json::to_value(row(&answer, HELD)).expect("the row");
    assert_eq!(
        serialized["formed"]["checkout"]["content_hash_prefix"],
        Value::Null
    );
    assert!(
        fixture
            .status(None)
            .packs
            .iter()
            .all(|pack| pack.formed.checkout.is_none())
    );

    // A --repo that is no directory.
    let mut file = fixture.request(None);
    file.repo = Some(fixture.root.join("bundle.json"));
    let error = standard_pack_status(&file).expect_err("a --repo that is no directory");
    assert_eq!(error.code, VALIDATION_FAILED, "{}", error.message);
}

#[test]
fn s314_train_an_unreadable_record_leaves_only_its_pack_not_read() {
    let fixture = Fixture::write();
    // A file of a record's name beside the record of the pack that runs, which is no record.
    fs::write(
        fixture
            .state
            .join("trials")
            .join(RUNS)
            .join(format!("{OTHER_CONTENT}-9.json")),
        b"{}",
    )
    .expect("the file");

    let answer = fixture.status(None);

    assert_eq!(answer.packs.len(), 5, "{answer:?}");
    let runs = row(&answer, RUNS);
    assert!(runs.trial.is_none(), "{runs:?}");
    let not_read = runs
        .trial_not_read
        .as_ref()
        .expect("why its records were not read");
    assert!(
        not_read
            .outcome
            .is(ResourceToolingCode::TrialRecordUnreadable),
        "{not_read:?}"
    );
    assert_eq!(
        not_read.outcome.value("trial_record_problem"),
        Some(&json!(TrialRecordProblem::Undecodable.as_str())),
        "{not_read:?}"
    );
    let serialized = serde_json::to_value(runs).expect("the row");
    assert!(serialized.get("trial").is_none(), "{serialized}");
    assert_eq!(
        serialized["trial_not_read"]["outcome"]["code"],
        "trial_record_unreadable"
    );
    // The other packs are answered.
    let tried = row(&answer, TRIED);
    assert!(tried.trial_not_read.is_none(), "{tried:?}");
    assert_eq!(
        tried.trial.as_ref().map(|trial| trial.state),
        Some(TrialState::Passed)
    );
}
