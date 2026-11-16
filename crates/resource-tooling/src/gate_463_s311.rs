// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-11 (register §1.418; contracts/standard-pack-trial.md, "The run", "The end",
// "The record"; coordinator work order rd-1115/brief-463-s311-trial-run.md, with what the review of
// S3-10's third round left for this slice): the offline parts of the trial's run.
// - `s311_the_runs_own_refusals_come_after_the_plan_in_their_order`: `check_trial_run` refuses a
//   run without an authorization (`trial_authorization_missing`) and a package the irreversible
//   mark flags without `--irreversible` (`trial_irreversible_unacknowledged`), in that order, each
//   naming the package; a mark not judged (no root) refuses nothing; an acknowledged mark passes;
//   what passes is the plan as the record keeps it, the authorization as given. Adapted by
//   Workflow #463 S3-8, S3-9 (register §1.444): a `--variable` refuses nothing any more
//   (`trial_variables_not_carried` is retired); the plan the record keeps names the value given.
// - `s311_a_record_plan_holds_the_authorization_to_the_plans_bounds` (the review's item 3): the
//   conversion of a plan into the record's plan refuses a blank authorization (`value_missing`),
//   one of 1025 bytes (`out_of_range`) and one with a control character (`not_recognised`), each
//   `validation_failed` naming `--authorization`, as the plan's own check does; 1024 bytes pass.
// - `s311_a_record_of_a_run_says_whether_its_end_matched`: `PackTrialRecord::of_run` keeps the end
//   and judges `matched` against `--expect` (true on the expected key, false on another key and on
//   a failed run, absent without `--expect`); `unexpected_end` is the warning
//   `trial_end_unexpected` (`package_id`, the end's `outcome_key`) for a run that ended normally on
//   another key, and none for the expected key, a failed run or no `--expect`.
// The writer's failure paths (the review's item 1), each with the writer's steps given in the
// module (`write_trial_record_with`):
// - `s311_without_hard_links_the_record_is_created_exclusively`: a link the operating system
//   refuses as unsupported falls back to an exclusive create of the record's name: the record is
//   written whole and read back, and no `.part` is left; when the flush of that file is refused too,
//   the answer is `trial_record_write_failed` naming the record, its cause `io_op` `sync`, the
//   link's refusal a `note` link, and both files this call created are removed;
// - `s311_a_part_the_writer_cannot_remove_leaves_the_record_written_with_a_warning`: the record is
//   in place and its `.part` cannot be removed (a directory stands at its name): the writer
//   succeeds, `part_left` the `.part`, and its one warning `trial_record_part_left` names it, the
//   refusal (`io_op` `remove`) its cause; the record reads back;
// - `s311_a_refused_flush_removes_the_part_and_names_sync`: the flush of the `.part` is refused:
//   `trial_record_write_failed` naming the `.part`, its cause `io_op` `sync`, the `.part` removed,
//   nothing left in the record's directory, and the same record is then written by the
//   operating system's own steps;
// - `s311_a_part_whose_removal_is_refused_is_named_by_a_cleanup_link`: a refused write whose
//   `.part` cannot be removed after it: `trial_record_write_failed`, its cause `io_op` `write`, and a
//   `cleanup` link (`foreign_os_error`, `io_op` `remove`, `path` the `.part`) naming the file left.
// No device, no Runtime, no clock read, no sleep: temporary directories and made-up packages.

use crate::codes::ResourceToolingCode;
use crate::standard_pack_trial::{WriteSteps, write_trial_record_with};
use crate::{
    MAX_AUTHORIZATION_BYTES, PackTrialRecord, StandardPackTrialRequest, TrialIrreversible,
    TrialRecordPlan, TrialRunEnd, WrittenTrialRecord, check_trial_run, plan_standard_pack_trial,
    trial_record_path, trial_records, write_trial_record,
};
use actingcommand_contract::outcome::{CodeStr, IoOp};
use actingcommand_contract::{LabError, RuntimeErrorCode, TaskOutcome};
use serde_json::{Map, Value, json};
use std::cell::Cell;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The `linear_steps` package tried, with a target page and an ending page, and one variable.
const TRIED: &str = "neutral.test.trial";
/// Another package of the game root, which registers it and not [`TRIED`].
const OTHER: &str = "neutral.test.other";
const DONE_KEY: &str = "task_done";
const SHORT_KEY: &str = "task_short";
const INSTANCE: &str = "node-a";
const AUTHORIZATION: &str = "board item 463, comment 0000000002";
const REQUEST_ID: &str = "req-s311";
/// When the run's terminal was recorded, which names the record.
const AT_UNIX_MS: u64 = 4_102_444_800_000;
const VALIDATION_FAILED: &str = "validation_failed";

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

/// A colour probe of the first pixel that tells `page`.
fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

/// A `scheduling_outcome` mapping of `key` on `page`.
fn mapping(key: &str, page: &str) -> Value {
    json!({"outcome_key": key, "effect": "no_designated_effect", "terminal_pages": [page]})
}

fn write_files(directory: &Path, control: Value, task: Value) {
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

/// Writes [`TRIED`] at `directory`: a `linear_steps` package of one trusted click from `home` (a
/// blue first pixel) to its target page `done` (white), its ending page `short` (green), the keys
/// [`DONE_KEY`] and [`SHORT_KEY`], and the variable `floor` (a whole number of 0 to 200 on a step
/// of 10, default 0).
fn write_trial_package(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": TRIED,
        "execution_mode": "linear_steps",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 64, "height": 24},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 1,
        "variables": [
            {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
             "min": 0, "max": 200, "step": 10, "default": 0}
        ]
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "S3-11 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [
            probe("home", [0, 0, 255]),
            probe("done", [255, 255, 255]),
            probe("short", [0, 255, 0])
        ],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "done": {"required": ["done_colour"]},
            "short": {"required": ["short_colour"]}
        },
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 1,
        "scheduling_outcome": {"mappings": [
            mapping(DONE_KEY, "done"),
            mapping(SHORT_KEY, "short")
        ]},
        "operations": [{
            "id": "go", "from": "home", "to": "done",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    write_files(directory, control, task);
}

/// Writes [`OTHER`] at `directory`: a page-graph package from `home` to `page_b` that provides
/// nothing.
fn write_other_package(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": OTHER,
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
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "S3-11 fixture",
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
    write_files(directory, control, task);
}

/// A game root `<temp>/root/neutral` whose `bundle.json` registers [`OTHER`] alone, so that the
/// package tried, which it does not register, counts as marked (the type's safe side).
fn game_root(temp: &TempDir) -> PathBuf {
    let root = temp.path().join("root").join(GAME);
    write_other_package(&root.join(OTHER));
    write_json(
        &root.join("bundle.json"),
        &json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": [{
            "package_id": OTHER, "server": SERVER, "entry_task_id": "task",
            "path": format!("packs/{OTHER}"), "file_count": 3, "byte_count": 64
        }]}),
    );
    root
}

/// A trial of [`TRIED`] at `package` on [`INSTANCE`] with [`AUTHORIZATION`], its records kept
/// under `state`; nothing else given.
fn request(package: &Path, state: &Path) -> StandardPackTrialRequest {
    StandardPackTrialRequest {
        package: package.to_path_buf(),
        instance: INSTANCE.to_owned(),
        authorization: Some(AUTHORIZATION.to_owned()),
        variables: Vec::new(),
        expect: None,
        irreversible: false,
        root: None,
        state_dir: state.to_path_buf(),
    }
}

/// `request`'s plan, then the run's own refusals.
fn checked(request: &StandardPackTrialRequest) -> Result<TrialRecordPlan, LabError> {
    let answer = plan_standard_pack_trial(request)
        .unwrap_or_else(|error| panic!("the plan: {}: {}", error.code, error.message));
    check_trial_run(&answer)
}

/// The values of the refusal `error`, which must be `code` with its outcome in the details.
#[track_caller]
fn refused_with(error: &LabError, code: ResourceToolingCode) -> Value {
    assert_eq!(error.code, code.as_str(), "{}", error.message);
    let details = error.details.clone().expect("the refusal's details");
    assert_eq!(details["outcome"]["code"], code.as_str(), "{details}");
    details["outcome"]["values"].clone()
}

/// The links of the refusal `error`'s outcome with the relation `relation`.
fn links(error: &LabError, relation: &str) -> Vec<Value> {
    let details = error.details.clone().expect("the refusal's details");
    details["outcome"]["causes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|link| link["relation"] == relation)
        .collect()
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).expect("an absolute path")
}

/// The record of a run of [`TRIED`] (written at `temp/packs`) that ended `end`, expecting
/// `expect`, its state directory `temp/state`.
fn record_of(temp: &TempDir, expect: Option<&str>, end: TrialRunEnd) -> PackTrialRecord {
    let package = temp.path().join("packs").join(TRIED);
    write_trial_package(&package);
    let state = temp.path().join("state");
    let kept = checked(&StandardPackTrialRequest {
        expect: expect.map(str::to_owned),
        ..request(&package, &state)
    })
    .expect("the run may be sent");
    PackTrialRecord::of_run(
        kept,
        AT_UNIX_MS,
        REQUEST_ID.to_owned(),
        Some("run-s311".to_owned()),
        end,
    )
}

/// A run that ended normally on the page of `key`.
fn ended_on(key: &str, page: &str) -> TrialRunEnd {
    TrialRunEnd {
        outcome: TaskOutcome::Success,
        outcome_key: Some(key.to_owned()),
        final_page: Some(page.to_owned()),
        failure_code: None,
        failure_values: Map::new(),
    }
}

/// A run that failed, its code and one value.
fn failed() -> TrialRunEnd {
    let mut failure_values = Map::new();
    failure_values.insert("receipt_state".to_owned(), json!("failed"));
    TrialRunEnd {
        outcome: TaskOutcome::Failure,
        outcome_key: None,
        final_page: Some("neutral/home".to_owned()),
        failure_code: Some(CodeStr::from(
            RuntimeErrorCode::ContainedTaskDeadlineExceeded.code(),
        )),
        failure_values,
    }
}

/// The record's file and its `.part` under the state directory `state`.
fn places(state: &Path, record: &PackTrialRecord) -> (PathBuf, PathBuf) {
    let path = trial_record_path(
        &absolute(state),
        TRIED,
        &record.plan.content_hash_prefix,
        record.at_unix_ms,
    );
    let part = path.with_extension("json.part");
    (path, part)
}

/// The names in the record's directory.
fn names(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .expect("the record's directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// The operating system's own write step: the bytes written whole and flushed.
fn written_whole(mut file: fs::File, bytes: &[u8]) -> Result<(), (io::Error, IoOp)> {
    file.write_all(bytes)
        .map_err(|error| (error, IoOp::Write))?;
    file.sync_all().map_err(|error| (error, IoOp::Sync))
}

/// A link a file system without hard links refuses.
fn unsupported(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// A refusal of a step the test makes.
fn refused_here() -> io::Error {
    io::Error::other("refused by the test")
}

#[test]
fn s311_the_runs_own_refusals_come_after_the_plan_in_their_order() {
    let temp = TempDir::new().expect("a temporary directory");
    let package = temp.path().join("packs").join(TRIED);
    write_trial_package(&package);
    let state = temp.path().join("state");
    let root = game_root(&temp);
    let floor = vec![("floor".to_owned(), json!(20))];

    // All three at once: the authorization is refused first.
    let error = checked(&StandardPackTrialRequest {
        authorization: None,
        root: Some(root.clone()),
        variables: floor.clone(),
        ..request(&package, &state)
    })
    .expect_err("no authorization");
    assert_eq!(
        refused_with(&error, ResourceToolingCode::TrialAuthorizationMissing),
        json!({"package_id": TRIED})
    );
    // With it, the mark is next.
    let error = checked(&StandardPackTrialRequest {
        root: Some(root.clone()),
        variables: floor.clone(),
        ..request(&package, &state)
    })
    .expect_err("a marked package not acknowledged");
    assert_eq!(
        refused_with(&error, ResourceToolingCode::TrialIrreversibleUnacknowledged),
        json!({"package_id": TRIED})
    );
    // Acknowledged, a variable given refuses nothing (Workflow #463 S3-8, S3-9): the plan the
    // record keeps names the value the run is sent with.
    let given = checked(&StandardPackTrialRequest {
        root: Some(root.clone()),
        irreversible: true,
        variables: floor,
        ..request(&package, &state)
    })
    .expect("a variable given passes");
    assert_eq!(
        serde_json::to_value(&given.variables).expect("the variables as JSON"),
        json!([{"name": "floor", "value": 20, "source": "given"}])
    );
    // A marked package acknowledged, and a mark not judged, pass; the record keeps the plan.
    let acknowledged = checked(&StandardPackTrialRequest {
        root: Some(root),
        irreversible: true,
        ..request(&package, &state)
    })
    .expect("an acknowledged mark passes");
    assert_eq!(acknowledged.irreversible, TrialIrreversible::Marked);
    assert!(acknowledged.irreversible_acknowledged);
    let not_judged = checked(&request(&package, &state)).expect("a mark not judged passes");
    assert_eq!(not_judged.irreversible, TrialIrreversible::NotJudged);
    assert!(!not_judged.irreversible_acknowledged);
    assert_eq!(not_judged.authorization, AUTHORIZATION);
    assert_eq!(not_judged.package_id, TRIED);
    assert!(!state.exists(), "the refusals made the state directory");
}

#[test]
fn s311_a_record_plan_holds_the_authorization_to_the_plans_bounds() {
    let temp = TempDir::new().expect("a temporary directory");
    let package = temp.path().join("packs").join(TRIED);
    write_trial_package(&package);
    let plan = plan_standard_pack_trial(&request(&package, &temp.path().join("state")))
        .expect("the plan")
        .plan;
    let longest = "a".repeat(MAX_AUTHORIZATION_BYTES);
    for (authorization, problem) in [
        ("   ".to_owned(), "value_missing"),
        ("a".repeat(MAX_AUTHORIZATION_BYTES + 1), "out_of_range"),
        ("board item\t463".to_owned(), "not_recognised"),
    ] {
        let mut out_of_bounds = plan.clone();
        out_of_bounds.authorization = Some(authorization);
        let error = TrialRecordPlan::try_from(out_of_bounds).expect_err("refused");
        assert_eq!(error.code, VALIDATION_FAILED, "{}", error.message);
        let details = error.details.clone().expect("the refusal's details");
        assert_eq!(details["argument"], "--authorization", "{details}");
        assert_eq!(details["argument_problem"], problem, "{details}");
    }
    let mut within = plan;
    within.authorization = Some(longest.clone());
    let kept = TrialRecordPlan::try_from(within).expect("1024 bytes are taken");
    assert_eq!(kept.authorization, longest);
}

#[test]
fn s311_a_record_of_a_run_says_whether_its_end_matched() {
    let target = record_of(
        &TempDir::new().expect("a temporary directory"),
        Some(DONE_KEY),
        ended_on(DONE_KEY, "neutral/done"),
    );
    assert_eq!(target.matched, Some(true));
    assert_eq!(target.end, ended_on(DONE_KEY, "neutral/done"));
    assert_eq!(target.request_id, REQUEST_ID);
    assert_eq!(target.at_unix_ms, AT_UNIX_MS);
    assert_eq!(target.plan.expect.as_deref(), Some(DONE_KEY));
    assert_eq!(target.unexpected_end(), None);

    let ending = record_of(
        &TempDir::new().expect("a temporary directory"),
        Some(DONE_KEY),
        ended_on(SHORT_KEY, "neutral/short"),
    );
    assert_eq!(ending.matched, Some(false));
    let warning = ending.unexpected_end().expect("the warning");
    assert_eq!(
        warning.code().as_str(),
        ResourceToolingCode::TrialEndUnexpected.as_str()
    );
    assert_eq!(
        json!(warning.values()),
        json!({"package_id": TRIED, "outcome_key": SHORT_KEY})
    );

    let failure = record_of(
        &TempDir::new().expect("a temporary directory"),
        Some(DONE_KEY),
        failed(),
    );
    assert_eq!(failure.matched, Some(false));
    assert_eq!(failure.end.outcome_key, None);
    assert_eq!(failure.unexpected_end(), None);

    let unexpected = record_of(
        &TempDir::new().expect("a temporary directory"),
        None,
        ended_on(SHORT_KEY, "neutral/short"),
    );
    assert_eq!(unexpected.matched, None);
    assert_eq!(unexpected.unexpected_end(), None);
}

#[test]
fn s311_without_hard_links_the_record_is_created_exclusively() {
    let temp = TempDir::new().expect("a temporary directory");
    let record = record_of(&temp, Some(DONE_KEY), ended_on(DONE_KEY, "neutral/done"));
    let state = temp.path().join("state");
    let (path, part) = places(&state, &record);
    let written = write_trial_record_with(
        &state,
        &record,
        &WriteSteps {
            link: &unsupported,
            write: &written_whole,
        },
    )
    .expect("the record is created exclusively");
    assert_eq!(
        written,
        WrittenTrialRecord {
            path: path.clone(),
            part_left: None,
            warnings: Vec::new(),
        }
    );
    let directory = path.parent().expect("the record's directory").to_path_buf();
    assert_eq!(
        names(&directory),
        vec![format!(
            "{}-{AT_UNIX_MS}.json",
            record.plan.content_hash_prefix
        )]
    );
    assert_eq!(
        trial_records(&state, TRIED).expect("the records"),
        vec![record.clone()]
    );

    // Without hard links and with the record's own flush refused: no record, both files gone.
    let later = PackTrialRecord {
        at_unix_ms: AT_UNIX_MS + 1,
        ..record.clone()
    };
    let (later_path, later_part) = places(&state, &later);
    let calls = Cell::new(0_u32);
    let flush_refused_second = |file: fs::File, bytes: &[u8]| -> Result<(), (io::Error, IoOp)> {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            return written_whole(file, bytes);
        }
        drop(file);
        Err((refused_here(), IoOp::Sync))
    };
    let error = write_trial_record_with(
        &state,
        &later,
        &WriteSteps {
            link: &unsupported,
            write: &flush_refused_second,
        },
    )
    .expect_err("the exclusive create's flush is refused");
    assert_eq!(calls.get(), 2, "the .part and then the record were written");
    assert_eq!(
        refused_with(&error, ResourceToolingCode::TrialRecordWriteFailed)["path"],
        json!(later_path.display().to_string())
    );
    let caused = links(&error, "caused_by");
    assert_eq!(caused.len(), 1, "{caused:?}");
    assert_eq!(caused[0]["values"]["io_op"], "sync", "{caused:?}");
    let noted = links(&error, "note");
    assert_eq!(noted.len(), 1, "{noted:?}");
    assert_eq!(noted[0]["code"], "foreign_os_error", "{noted:?}");
    assert!(links(&error, "cleanup").is_empty());
    assert!(
        !later_path.exists(),
        "the record's file of the create is left"
    );
    assert!(!later_part.exists(), "the .part is left");
    assert!(!part.exists());
    assert_eq!(
        names(&directory),
        vec![format!(
            "{}-{AT_UNIX_MS}.json",
            record.plan.content_hash_prefix
        )]
    );
}

#[test]
fn s311_a_part_the_writer_cannot_remove_leaves_the_record_written_with_a_warning() {
    let temp = TempDir::new().expect("a temporary directory");
    let record = record_of(&temp, Some(DONE_KEY), ended_on(SHORT_KEY, "neutral/short"));
    let state = temp.path().join("state");
    let (path, part) = places(&state, &record);
    // The link is made, and a directory then stands at the `.part`'s name, which no removal of a
    // file removes.
    let linked_then_held = |from: &Path, to: &Path| -> io::Result<()> {
        fs::hard_link(from, to)?;
        fs::remove_file(from)?;
        fs::create_dir(from)?;
        fs::write(from.join("held"), b"held")
    };
    let written = write_trial_record_with(
        &state,
        &record,
        &WriteSteps {
            link: &linked_then_held,
            write: &written_whole,
        },
    )
    .expect("the record is written");
    assert_eq!(written.path, path);
    assert_eq!(written.part_left, Some(part.clone()));
    assert_eq!(written.warnings.len(), 1, "{:?}", written.warnings);
    let warning = json!(written.warnings[0]);
    assert_eq!(
        warning["code"],
        ResourceToolingCode::TrialRecordPartLeft.as_str(),
        "{warning}"
    );
    assert_eq!(
        warning["values"]["path"],
        json!(part.display().to_string()),
        "{warning}"
    );
    assert_eq!(warning["causes"][0]["relation"], "caused_by", "{warning}");
    assert_eq!(
        warning["causes"][0]["values"]["io_op"], "remove",
        "{warning}"
    );
    assert_eq!(
        trial_records(&state, TRIED).expect("the records"),
        vec![record]
    );
}

#[test]
fn s311_a_refused_flush_removes_the_part_and_names_sync() {
    let temp = TempDir::new().expect("a temporary directory");
    let record = record_of(&temp, Some(DONE_KEY), failed());
    let state = temp.path().join("state");
    let (path, part) = places(&state, &record);
    let flush_refused = |mut file: fs::File, bytes: &[u8]| -> Result<(), (io::Error, IoOp)> {
        file.write_all(bytes)
            .map_err(|error| (error, IoOp::Write))?;
        Err((refused_here(), IoOp::Sync))
    };
    let error = write_trial_record_with(
        &state,
        &record,
        &WriteSteps {
            link: &unsupported,
            write: &flush_refused,
        },
    )
    .expect_err("the flush is refused");
    assert_eq!(
        refused_with(&error, ResourceToolingCode::TrialRecordWriteFailed)["path"],
        json!(part.display().to_string())
    );
    let caused = links(&error, "caused_by");
    assert_eq!(caused.len(), 1, "{caused:?}");
    assert_eq!(caused[0]["code"], "foreign_os_error", "{caused:?}");
    assert_eq!(caused[0]["values"]["io_op"], "sync", "{caused:?}");
    assert!(links(&error, "cleanup").is_empty());
    assert!(!part.exists(), "the .part is left");
    assert!(!path.exists(), "a record was written");
    let directory = path.parent().expect("the record's directory");
    assert_eq!(names(directory), Vec::<String>::new());

    // Its own leftovers block nothing: the same record is written by the operating system's steps.
    let written = write_trial_record(&state, &record).expect("the record is written");
    assert_eq!(written.path, path);
    assert_eq!(written.part_left, None);
    assert_eq!(
        trial_records(&state, TRIED).expect("the records"),
        vec![record]
    );
}

#[test]
fn s311_a_part_whose_removal_is_refused_is_named_by_a_cleanup_link() {
    let temp = TempDir::new().expect("a temporary directory");
    let record = record_of(&temp, Some(DONE_KEY), ended_on(DONE_KEY, "neutral/done"));
    let state = temp.path().join("state");
    let (path, part) = places(&state, &record);
    // The write is refused, and a directory then stands at the `.part`'s name.
    let held = part.clone();
    let write_refused_and_held = move |file: fs::File, _: &[u8]| -> Result<(), (io::Error, IoOp)> {
        drop(file);
        let held_up = fs::remove_file(&held)
            .and_then(|()| fs::create_dir(&held))
            .and_then(|()| fs::write(held.join("held"), b"held"));
        match held_up {
            Ok(()) => Err((refused_here(), IoOp::Write)),
            Err(error) => Err((error, IoOp::Create)),
        }
    };
    let error = write_trial_record_with(
        &state,
        &record,
        &WriteSteps {
            link: &unsupported,
            write: &write_refused_and_held,
        },
    )
    .expect_err("the write is refused");
    assert_eq!(
        refused_with(&error, ResourceToolingCode::TrialRecordWriteFailed)["path"],
        json!(part.display().to_string())
    );
    let caused = links(&error, "caused_by");
    assert_eq!(caused.len(), 1, "{caused:?}");
    assert_eq!(caused[0]["values"]["io_op"], "write", "{caused:?}");
    let cleanup = links(&error, "cleanup");
    assert_eq!(cleanup.len(), 1, "{cleanup:?}");
    assert_eq!(cleanup[0]["code"], "foreign_os_error", "{cleanup:?}");
    assert_eq!(cleanup[0]["values"]["io_op"], "remove", "{cleanup:?}");
    assert_eq!(
        cleanup[0]["values"]["path"],
        json!(part.display().to_string()),
        "{cleanup:?}"
    );
    assert!(error.message.contains("is left"), "{}", error.message);
    assert!(part.is_dir(), "the file left is named, not removed");
    assert!(!path.exists(), "a record was written");
}
