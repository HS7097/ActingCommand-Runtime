// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-14 (register §1.446; contracts/standard-pack-schedule.md, "Where a pack stands:
// `standard-pack status`"; the design rd-1115/pack-stage3-design.md §2.3 and §4.2 "S3-14";
// coordinator work order rd-1116/brief-463-s314-pack-status.md): one read that says where each
// pack of a game root stands. One installed game root of five packs, inside a made-up installation
// whose operation journal names one of them in a placement's `enabled`, and a state directory
// beside it holding trial records written by the trial's own writer:
// - `s314_a_held_pack_can_be_tried_and_its_hold_is_missing`: the held pack is admitted, stands
//   `can_be_tried`, answers `enabled.state` `held`, and its only missing item is S2, failing
//   `held`, with the hold's reason and reference as the schedule's entry writes them;
// - `s314_a_pack_ready_to_place_answers_the_passed_trial_of_its_content`: a pack the readiness
//   answers `ready_to_place` answers its content hash prefix, the trial's own, and the passed
//   trial of that content with its request, its business end and its time;
// - `s314_a_pack_placed_under_a_journal_that_names_it_answers_journals_not_read`: the placement
//   journals belong to the rescue module, which this read cannot reach: the pack a journal names
//   is `enabled` with `not_recorded` and `journals_not_read`, nothing inferred from the journal;
// - `s314_a_pack_that_runs_unnamed_by_any_journal_answers_not_recorded`: a pack a rule enables and
//   no journal names answers the same two words;
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
// Made-up packs only; no device, no Runtime, no clock read, no sleep.

use crate::codes::{ReadinessItem, ReadinessProblem, ReadinessState};
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
/// A pack a rule enables, which the made-up placement journal names in its `enabled`.
const PLACED: &str = "neutral.test.placed";
/// A pack a rule enables, which no journal names.
const RUNS: &str = "neutral.test.runs";
/// The pool the rules produce into, with a valuation.
const VALUED: &str = "pool.valued";
/// The hold's reason and reference.
const REASON: &str = "waits for its first trial on a real application";
const REFERENCE: &str = "fixture reference 1";
/// The trial's instance and authorization, as the trial records them.
const INSTANCE: &str = "node-a";
const AUTHORIZATION: &str = "board item 463, comment 0000000001";
/// The passed trial of [`TRIED`]'s content: its request, its business end and its time.
const PASSED_REQUEST: &str = "req-1000";
const BUSINESS_END: &str = "completed";
const PASSED_AT: u64 = 1_000;
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

/// What the tests read: the made-up installation, its game root, its operation journal, the
/// state directory beside it and the content prefix the trial gives [`TRIED`].
struct Fixture {
    temp: TempDir,
    root: PathBuf,
    state: PathBuf,
    journal: PathBuf,
    tried_content: String,
}

impl Fixture {
    /// The installation `<temp>/installation` (its installer's state file `install\state.json`
    /// and one `pack_place` journal, `install\operations\op-1000.json`, whose inputs name the
    /// authorization and [`PLACED`] in `enabled`, the members a placement records); its game root
    /// `packages/neutral` with the five packs, the three rules, the hold of [`HELD`] and the
    /// server's return-home package; and the state directory `<temp>/state` with three trial
    /// records the trial's writer writes: [`TRIED`]'s content passed at 1000, another content of
    /// [`TRIED`] failed at 2000, and another content of [`RUNS`] passed at 1500.
    fn write() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let installation = temp.path().join("installation");
        write_json(&installation.join("install").join("state.json"), &json!({}));
        let journal = installation
            .join("install")
            .join("operations")
            .join("op-1000.json");
        write_json(
            &journal,
            &json!({
                "schema_version": "actingcommand.install-operation.v1",
                "operation_id": "op-1000",
                "kind": "pack_place",
                "initiator": "Lab-CLI",
                "inputs": {
                    "game": GAME,
                    "authorization": AUTHORIZATION,
                    "enabled": [PLACED]
                },
                "finished_at_unix_ms": 1_001
            }),
        );
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
            &json!({
                "schema_version": "actingcommand.schedule.v1",
                "rules": [rule(TRIED), rule(PLACED), rule(RUNS)],
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
                "held": [{"package_id": HELD, "reason": REASON, "reference": REFERENCE}]
            }),
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
        let kept = TrialRecordPlan::try_from(plan.plan).expect("a plan with an authorization");
        let tried_content = kept.content_hash_prefix.clone();
        assert_ne!(tried_content, OTHER_CONTENT);
        let mut other = kept.clone();
        other.content_hash_prefix = OTHER_CONTENT.to_owned();
        let mut other_of_runs = other.clone();
        other_of_runs.package_id = RUNS.to_owned();
        other_of_runs.package = root.join(RUNS).display().to_string();
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
            journal,
            tried_content,
        }
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
    assert_eq!(trial.request_id.as_deref(), Some(PASSED_REQUEST));
    assert_eq!(trial.at_unix_ms, Some(PASSED_AT));
    assert_eq!(
        trial
            .end
            .as_ref()
            .and_then(|end| end.outcome_key.as_deref()),
        Some(BUSINESS_END)
    );
    let serialized = serde_json::to_value(tried).expect("the row");
    assert_eq!(serialized["trial"]["state"], "passed");
    assert_eq!(serialized["trial"]["end"]["outcome"], "success");
    assert_eq!(serialized["trial"]["end"]["outcome_key"], BUSINESS_END);
}

#[test]
fn s314_a_pack_placed_under_a_journal_that_names_it_answers_journals_not_read() {
    let fixture = Fixture::write();
    let journal: Value =
        serde_json::from_slice(&fs::read(&fixture.journal).expect("the journal")).expect("JSON");
    assert_eq!(journal["inputs"]["enabled"], json!([PLACED]));

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
fn s314_a_pack_that_runs_unnamed_by_any_journal_answers_not_recorded() {
    let fixture = Fixture::write();
    let journal = fs::read_to_string(&fixture.journal).expect("the journal");
    assert!(!journal.contains(RUNS), "{journal}");

    let answer = fixture.status(None);

    let runs = row(&answer, RUNS);
    assert_eq!(runs.stage_state, StageState::Enabled);
    assert_eq!(runs.enabled.state, EnabledState::Enabled);
    assert_eq!(runs.enabled.authorization, PackAuthorization::NotRecorded);
    assert_eq!(
        runs.enabled.authorization_unrecorded,
        AuthorizationUnrecorded::JournalsNotRead
    );
}

#[test]
fn s314_the_counts_add_up() {
    let fixture = Fixture::write();

    let answer = fixture.status(None);

    assert_eq!(answer.packs.len(), 5, "{answer:?}");
    assert_eq!(
        answer.stage_states,
        StageStateCounts {
            not_admitted: 0,
            can_be_tried: 2,
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
    assert_eq!(tried.at_unix_ms, Some(PASSED_AT));
    // The only record of the pack that runs names another content.
    let runs = row(&answer, RUNS).trial.as_ref().expect("a trial");
    assert_eq!(runs.state, TrialState::NoRecord);
    assert_eq!(runs.request_id, None);
    let serialized = serde_json::to_value(row(&answer, RUNS)).expect("the row");
    assert_eq!(serialized["trial"], json!({"state": "none"}));
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
