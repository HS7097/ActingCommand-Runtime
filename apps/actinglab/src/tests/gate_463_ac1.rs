// SPDX-License-Identifier: AGPL-3.0-only

// #463 AC-1 (register §1.461; contracts/standard-pack-authoring.md, "The cases" and "One judgement
// of the recording"; coordinator work order rd-1117/brief-463-ac1-adopt-cases.md): `standard-pack
// adopt --record-id` takes the regression cases the recording's last stop wrote beside its package
// (#463 S2-10) into the checkout, each run first by the one runner on the package as the adoption
// writes it, and judges the recording by `record ready`'s one function (S3-17). The recordings are
// made as gate_463_s21b makes them: screens of a 4x4 square of one colour each through the Lab's
// recording API (a tap on each but the last), the session, every mark and the stop through the
// command line; the checkout is a made-up resource repository of the game `neutral`, server
// `test`, without a rule; every adoption is of the class held.
// - `ac1_five_cases_are_taken_byte_for_byte_and_pass_resource_regression`: a recording with five
//   cases adopted: five directories `regression/neutral/<package_id>.<case>`, each byte for byte the
//   recording's, in `written` after the package and the root file and in `cases.written`;
//   `resource regression` on the checkout afterwards passes all five; S3-17: the packaged
//   recording is taken as before, `review` without `ready_not_read`.
// - `ac1_replace_moves_the_earlier_set_and_leaves_a_hand_written_case`: the package adopted from a
//   recording of five screens, then adopted with `--replace` from one of three: the five earlier
//   cases are under `<state>/adopt/<package_id>.cases.old-<ms>/` byte for byte, the three new ones
//   in the checkout, a case of the package written by hand untouched.
// - `ac1_a_failing_case_refuses_and_writes_nothing`: the recording's package changed after its
//   stop (every colour probe expects white): `case_failed`, the details listing all five cases
//   (fix round 1: as `resource regression` lists them, each row with its id), `failed`; nothing
//   written in the checkout and nothing kept.
// - `ac1_a_destination_that_is_no_recordings_case_refuses_before_the_first_write`: a directory of a
//   case's name holding a file by hand: `case_place_taken` naming it; nothing written.
// - `ac1_the_dry_run_lists_the_cases_and_the_two_words_write_as_before`: `--dry-run` writes nothing
//   and lists the cases in `would_write`; the recording with its cases directory set aside answers
//   `none_found`, the package by `--package` `package_path`, each writing the package and the root
//   file alone, byte for byte what the adoption that took the cases wrote of them, and `next` naming
//   the regression case where none was taken.
// - `ac1_an_adoption_stopped_after_its_first_case_leaves_it_and_the_next_call_answers_the_contract`:
//   the test's machine stops the adoption right after it renamed its first case into place: the
//   package and the root file are written, that case alone in place and no `.part`; the next call
//   without `--replace` is refused `pack_exists`; with `--replace` (the person committed what was
//   written) it moves that case to the kept place and writes the whole set.
// - `ac1_s317_a_draft_recording_is_refused_with_record_readys_missing`: an active recording `record
//   ready` calls a draft is refused `recording_not_current`, its details' `record_ready` that
//   function's answer, `missing` the items `record ready --record-id` lists.
// - `ac1_s317_recording_not_current_against_record_ready`: four recordings: marked again after
//   their stop, their package moved away, packaged, active. The old check's members (stopped, not
//   marked again, the package's file there), `record ready`'s status and the adoption agree on
//   each, but on the moved package, which `record ready` calls packaged and the old check, kept for
//   that difference, refuses at `artifact`.
// - `ac1_s317_recording_unreviewed_against_record_ready`: a packaged recording whose package says
//   its cross-check was partially evaluated: `record ready` answers packaged with nothing missing,
//   the review, kept whole, refuses it for a business decision (`recording_unreviewed`) and takes it
//   as held.
// Fix round 1 (coordinator work order rd-1117/brief-463-ac1-fix1.md, its rulings 2 and 3):
// - `ac1_fix1_a_lab_failure_refuses_and_an_unreadable_recording_falls_back`: a packaged recording
//   the Lab's own function refuses otherwise than as one it cannot read whole (asked in a state
//   directory where the recording is not, `lab_recording_missing`) is refused
//   `recording_not_judged`, the Lab's refusal its cause, nothing written; the same recording with a
//   member added to its `recording.json` by hand, which the Lab cannot read whole
//   (`lab_recording_unreadable`), is judged by the check of its own members and adopted, said in
//   `review.ready_not_read`.
// - `ac1_fix1_failing_cases_beyond_the_bound_are_listed_as_resource_regression_lists_them`: fifteen
//   failing cases (the five of a recording and two copies of each in its set, every scene changed
//   after the stop): `case_failed` with `count` 15, `failed_total` 15, `listed` and the rows the
//   first `ROWS_LISTED_MAX` in the order of their names, each a short row without a check's
//   `detail`; nothing written. The 0.12.71 train: a copy keeps the `id` of the case it copies, and
//   nothing compares two cases' ids (the reader reads it as a text it answers, the runner runs each
//   directory by itself): each row carries the `id` of its own `case.json`, and rows of one id are
//   listed, each by its own path.
// No Runtime (the state root's variable names a folder that does not exist), no device, no clock,
// no sleep: made-up recordings, checkouts and values.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use crate::cli_module::own_line;
use crate::resource_verify::regression::ROWS_LISTED_MAX;
use crate::standard_pack_cli::LabAdoption;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{LabResult, canonical_json_bytes};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::{
    AttachFrameRequest, ClickEffect, CommitClickRequest, OpaqueJson, PlanClickRequest, RecordRect,
    RecordingLock, record_attach_frame, record_commit_click, record_plan_click,
};
use actingcommand_resource_tooling::{
    AdoptCaseAt, AdoptCasesRun, AdoptDecision, AdoptLab, AdoptSource, AdoptWorktree, ReadyRefused,
    RecordingReady, StandardPackAdoptRequest, standard_pack_adopt,
};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const INSTANCE: &str = "neutral.a";
const TASK: &str = "neutral_task";
const GAME: &str = "neutral";
const SERVER: &str = "test";
const LOCALE: &str = "en-US";
/// The package `record stop` builds of every recording here: `<game>.<server>.<task_id>`.
const PACKAGE: &str = "neutral.test.neutral_task";
const REASON: &str = "Not run through on a real device yet.";
const GRAY: [u8; 3] = [60, 60, 60];
/// The screens of a recording, in order, each a 4x4 square of its colour at the top left, marked
/// by a colour mark of its name.
const SCREENS: [(&str, [u8; 3]); 5] = [
    ("red", [230, 20, 20]),
    ("green", [20, 230, 20]),
    ("blue", [20, 20, 230]),
    ("yellow", [230, 230, 20]),
    ("cyan", [20, 230, 230]),
];
const TAP: RecordRect = RecordRect {
    x: 8,
    y: 2,
    width: 4,
    height: 4,
};
/// The times the direct adoptions give themselves, which name their kept places.
const SECOND: u64 = 2_000;
const THIRD: u64 = 3_000;

/// The version control of a checkout whose every change is committed, and the Lab's own parts as
/// actinglab gives them to adoption.
struct Committed;

impl AdoptWorktree for Committed {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn lab(&self) -> &dyn AdoptLab {
        &LabAdoption
    }
}

/// [`Committed`] on which the adoption stops right after it renamed its first case into place, as
/// a crash there would leave it: that rename is made and answered failed; every other rename is
/// made.
struct StopsAfterTheFirstCase {
    /// `regression/<game>/` of the checkout.
    regression: PathBuf,
    stopped: Cell<bool>,
}

impl AdoptWorktree for StopsAfterTheFirstCase {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)?;
        if to.starts_with(&self.regression) && !self.stopped.replace(true) {
            return Err(io::Error::other("the test stops the adoption here"));
        }
        Ok(())
    }

    fn lab(&self) -> &dyn AdoptLab {
        &LabAdoption
    }
}

/// Fix round 1: [`Committed`] whose Lab asks `record ready`'s own function about the recording in
/// another state directory, `elsewhere`, where it is not: the function refuses it with the Lab's
/// `lab_recording_missing`, as when the recording was moved away after adoption read its
/// `recording.json`, a failure of the Lab other than a recording it cannot read whole. Its cases
/// are run by the Lab's own runner.
struct RecordingGone {
    elsewhere: PathBuf,
}

impl AdoptWorktree for RecordingGone {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn lab(&self) -> &dyn AdoptLab {
        self
    }
}

impl AdoptLab for RecordingGone {
    fn recording_ready(
        &self,
        _state_dir: &Path,
        record_id: &str,
    ) -> Result<RecordingReady, ReadyRefused> {
        LabAdoption.recording_ready(&self.elsewhere, record_id)
    }

    fn run_cases(&self, package_dir: &Path, cases: &[AdoptCaseAt]) -> LabResult<AdoptCasesRun> {
        LabAdoption.run_cases(package_dir, cases)
    }
}

/// A 16x9 frame: a 4x4 square of `color` at the top left on gray.
fn frame_png(color: [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..9 {
        for x in 0..16 {
            pixels.extend(if x < 4 && y < 4 { color } else { GRAY });
        }
    }
    Frame::from_pixels(
        16,
        9,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

/// What `actinglab --json --instance neutral.a record <action> --state-dir <state> <more>` prints:
/// its exit code and its line.
fn record(state: &str, action: &str, more: &[&str]) -> (u8, Value) {
    let mut line = vec![
        "--json",
        "--instance",
        INSTANCE,
        "record",
        action,
        "--state-dir",
        state,
    ];
    line.extend(more);
    let exit = printed(&line);
    let answered = exit
        .line
        .clone()
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    (exit.code, answered)
}

fn opaque(value: Value) -> Option<OpaqueJson> {
    Some(OpaqueJson::from_serializable(&value).expect("opaque value"))
}

/// A frame showing `color` as `observe --capture --record` attaches it; with `request`, then a tap
/// of [`TAP`] as `do --capture --record --tap-rect` records a performed one.
fn step(state: &str, color: [u8; 3], request: Option<&str>) {
    let lock = RecordingLock::acquire(Path::new(state), INSTANCE, "gate_463_ac1", "Lab-CLI")
        .expect("recording lock");
    record_attach_frame(
        &lock,
        AttachFrameRequest {
            png: frame_png(color),
            source: "runtime_observation".to_owned(),
            runtime_artifact: None,
            capture_backend: None,
            freshness: None,
        },
    )
    .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message));
    let Some(request) = request else {
        return;
    };
    let plan = record_plan_click(
        &lock,
        &PlanClickRequest {
            tap_rect: Some(TAP),
            ..PlanClickRequest::default()
        },
    )
    .unwrap_or_else(|error| panic!("plan: {}: {}", error.code, error.message));
    record_commit_click(
        &lock,
        &plan,
        &CommitClickRequest {
            effect: ClickEffect::Performed,
            has_failure: false,
            carrier_package: opaque(json!({"package_label": "neutral.package"})),
            req_id: opaque(json!(request)),
            correlation_id: opaque(json!(format!("{request}-correlation"))),
            action_id: opaque(json!(format!("{request}-action"))),
            lease_id: opaque(json!("lease-0463")),
            failure: None,
            before: opaque(
                json!({"frame_id": format!("{request}-before"), "page": "neutral_page"}),
            ),
            after: opaque(json!({"frame_id": format!("{request}-after"), "page": null})),
        },
    )
    .unwrap_or_else(|error| panic!("tap: {}: {}", error.code, error.message));
}

/// The recording `record_id` under `state`, active: the first `screens` of [`SCREENS`], a tap on
/// each but the last, each step marked with a colour mark of its square.
fn marked_recording(state: &str, record_id: &str, screens: usize) {
    let (code, started) = record(
        state,
        "start",
        &["--task-id", TASK, "--record-id", record_id],
    );
    assert_eq!(code, 0, "{started}");
    for (index, (_, color)) in SCREENS.iter().take(screens).enumerate() {
        let request = format!("{record_id}-request-{index}");
        let tapped = index + 1 < screens;
        step(state, *color, tapped.then_some(request.as_str()));
    }
    for (index, (name, _)) in SCREENS.iter().take(screens).enumerate() {
        let number = (index + 1).to_string();
        let mark = format!("{name}_color=0,0,4,4");
        let (code, marked) = record(
            state,
            "mark",
            &["--step", number.as_str(), "--color", mark.as_str()],
        );
        assert_eq!(code, 0, "{marked}");
    }
}

/// [`marked_recording`], stopped with the game, server and locale of the checkout: its package
/// [`PACKAGE`], a JSON container, and its cases are written. The answer of `record stop`.
fn stopped_recording(state: &str, record_id: &str, screens: usize) -> Value {
    marked_recording(state, record_id, screens);
    let (code, stopped) = record(
        state,
        "stop",
        &["--game", GAME, "--server", SERVER, "--locale", LOCALE],
    );
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["lab"]["status"], "generated", "{stopped}");
    assert_eq!(stopped["lab"]["package_id"], PACKAGE, "{stopped}");
    assert_eq!(stopped["lab"]["container"], "json", "{stopped}");
    stopped
}

/// The ids of the cases `record stop` answered it wrote, in the order of their names.
fn case_ids(stopped: &Value) -> Vec<String> {
    let mut ids = stopped["lab"]["cases"]["ids"]
        .as_array()
        .unwrap_or_else(|| panic!("the stop answers its cases: {stopped}"))
        .iter()
        .map(|id| id.as_str().expect("a case id").to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

/// The recording's Lab directory, `<state>/record-artifacts/<record_id>/lab`.
fn lab_dir(state: &str, record_id: &str) -> PathBuf {
    Path::new(state)
        .join("record-artifacts")
        .join(record_id)
        .join("lab")
}

/// The recording's cases, `out/<package_id>.cases/`.
fn recording_cases(state: &str, record_id: &str) -> PathBuf {
    lab_dir(state, record_id)
        .join("out")
        .join(format!("{PACKAGE}.cases"))
}

/// The recording's package, `out/<package_id>.json`.
fn recording_package(state: &str, record_id: &str) -> PathBuf {
    lab_dir(state, record_id)
        .join("out")
        .join(format!("{PACKAGE}.json"))
}

/// The name of the checkout's directory of the case `id`: `<package_id>.<id>`.
fn case_directory(id: &str) -> String {
    format!("{PACKAGE}.{id}")
}

/// The directories of the cases `ids` relative to the checkout, in their order.
fn destinations(ids: &[String]) -> Vec<String> {
    ids.iter()
        .map(|id| format!("regression/{GAME}/{}", case_directory(id)))
        .collect()
}

/// Writes `value` at `path` in the canonical form.
fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
    fs::write(path, canonical_json_bytes(value).expect("canonical JSON")).expect("the file");
}

/// A made-up checkout of the game [`GAME`], server [`SERVER`] at `repo`: `applications.json`,
/// `maintenance.json` (`[]`), an empty `packs/` and `schedule.json` without a rule, in the
/// canonical form.
fn checkout(repo: &Path) {
    write_json(
        &repo.join("applications.json"),
        &json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {SERVER: {"application_id": "com.example.neutral", "label": "Test"}}
        }),
    );
    write_json(&repo.join("maintenance.json"), &json!([]));
    fs::create_dir_all(repo.join("packs")).expect("the packs directory");
    write_json(
        &repo.join("schedule.json"),
        &json!({"schema_version": "actingcommand.schedule.v1", "rules": []}),
    );
}

/// `standard-pack adopt` of the recording `record_id` into `repo` as held, the global flags
/// `global` before the command, as actinglab runs it.
fn adopt_line(repo: &Path, state: &str, record_id: &str, global: &[&str]) -> crate::CliResult {
    let repo = repo.to_str().expect("a UTF-8 path");
    let mut line = vec!["--json"];
    line.extend(global);
    line.extend([
        "standard-pack",
        "adopt",
        "--repo",
        repo,
        "--record-id",
        record_id,
        "--state-dir",
        state,
        "--held-reason",
        REASON,
    ]);
    crate::run_cli(line, true)
}

/// The answer of a line that succeeded on actinglab's own line.
fn answered(result: &crate::CliResult) -> Value {
    let shown = result.envelope_json();
    assert_eq!(own_line::exit(result).code, 0, "{shown}");
    result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("the line answers: {shown}"))
}

/// The details of a line refused on actinglab's own line (exit 1) with `code`.
fn refused_with(result: &crate::CliResult, code: &str) -> Value {
    let shown = result.envelope_json();
    assert_eq!(own_line::exit(result).code, 1, "{shown}");
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("the line was not refused: {shown}"));
    assert_eq!(error.code, code, "{shown}");
    error.details.clone().unwrap_or(Value::Null)
}

/// A direct adoption of the recording `record_id` into `repo` at the time `at_unix_ms`: as held,
/// or with `replace` and no decision.
fn request(
    repo: &Path,
    state: &str,
    record_id: &str,
    replace: bool,
    at_unix_ms: u64,
) -> StandardPackAdoptRequest {
    StandardPackAdoptRequest {
        repo: repo.to_path_buf(),
        source: AdoptSource::Recording {
            record_id: record_id.to_owned(),
            lab_dir: lab_dir(state, record_id),
        },
        decision: (!replace).then(|| AdoptDecision::Held {
            rationale: REASON.to_owned(),
            reference: None,
        }),
        replace,
        dry_run: false,
        state_dir: PathBuf::from(state),
        at_unix_ms,
    }
}

/// Every file below `root`, by its `/`-separated path relative to it, with its bytes.
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

/// The names of the entries of `directory`, in their order.
fn names_in(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .expect("the directory lists")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// Changes the entry task of the recording's package, the JSON container at `package`, by `edit`.
fn edit_task(package: &Path, edit: impl Fn(&mut Value)) {
    let mut container = serde_json::from_slice::<Value>(&fs::read(package).expect("the package"))
        .expect("the container reads");
    let files = container["files"]
        .as_object_mut()
        .expect("the container's files");
    for (name, text) in files.iter_mut() {
        if !name.ends_with("task.json") {
            continue;
        }
        let mut task = serde_json::from_str::<Value>(text.as_str().expect("a file's text"))
            .expect("the task reads");
        edit(&mut task);
        *text = Value::String(serde_json::to_string_pretty(&task).expect("the task writes"));
    }
    fs::write(
        package,
        serde_json::to_vec(&container).expect("the container writes"),
    )
    .expect("the package written");
}

/// Every colour probe below `value` expects white.
fn whiten(value: &mut Value) {
    match value {
        Value::Object(members) => {
            for (name, member) in members.iter_mut() {
                if name == "color_probes"
                    && let Some(probes) = member.as_array_mut()
                {
                    for probe in probes {
                        probe["expected"] = json!([255, 255, 255]);
                    }
                } else {
                    whiten(member);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(whiten),
        _ => {}
    }
}

#[test]
fn ac1_five_cases_are_taken_byte_for_byte_and_pass_resource_regression() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    let stopped = stopped_recording(&state, "rec_ac1_five", 5);
    let ids = case_ids(&stopped);
    assert_eq!(ids.len(), 5, "{stopped}");

    let data = answered(&adopt_line(&repo, &state, "rec_ac1_five", &[]));

    assert_eq!(data["status"], "adopted", "{data}");
    // S3-17: the packaged recording is judged by record ready's function and taken as before.
    assert!(data["review"].get("ready_not_read").is_none(), "{data}");
    let directories = destinations(&ids);
    let mut written = vec![json!(format!("packs/{PACKAGE}")), json!("schedule.json")];
    written.extend(directories.iter().map(|directory| json!(directory)));
    assert_eq!(data["written"], Value::Array(written), "{data}");
    assert_eq!(data["cases"]["state"], "taken", "{data}");
    assert_eq!(data["cases"]["written"], json!(directories), "{data}");
    assert_eq!(data["cases"]["would_write"], json!([]), "{data}");
    assert_eq!(data["cases"]["replaced"], json!([]), "{data}");
    assert_eq!(data["cases"]["not_proven"], json!([]), "{data}");
    assert!(data["cases"].get("kept").is_none(), "{data}");
    let from = data["cases"]["from"]
        .as_str()
        .expect("where the cases came from");
    assert!(
        Path::new(from).ends_with(format!("{PACKAGE}.cases")),
        "{data}"
    );
    // A regression case is taken, so `next` does not name one.
    assert_eq!(data["next"], json!([]), "{data}");
    let regression = repo.join("regression").join(GAME);
    assert_eq!(
        names_in(&regression),
        ids.iter().map(|id| case_directory(id)).collect::<Vec<_>>()
    );
    for id in &ids {
        let copied = tree(&regression.join(case_directory(id)));
        assert!(copied.contains_key("case.json"), "{id}: {copied:?}");
        assert_eq!(
            copied,
            tree(&recording_cases(&state, "rec_ac1_five").join(id)),
            "{id}"
        );
    }

    // `resource regression` on the checkout afterwards: all five pass.
    let repo_text = repo.to_str().expect("a UTF-8 path");
    let checked = answered(&crate::run_cli(
        vec!["--json", "resource", "regression", "--repo", repo_text],
        true,
    ));
    assert_eq!(
        checked["counts"],
        json!({"cases": 5, "passed": 5, "failed": 0, "partial": 0, "not_run": 0}),
        "{checked}"
    );
}

#[test]
fn ac1_replace_moves_the_earlier_set_and_leaves_a_hand_written_case() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    let first = case_ids(&stopped_recording(&state, "rec_ac1_first", 5));
    answered(&adopt_line(&repo, &state, "rec_ac1_first", &[]));
    // A case of the same package written by hand: another source.
    let regression = repo.join("regression").join(GAME);
    let hand = regression.join("neutral_hand_case");
    write_json(
        &hand.join("case.json"),
        &json!({
            "schema_version": "actingcommand.resource-regression-case.v1",
            "id": "hand",
            "package_id": PACKAGE,
            "scene": "scene.png",
            "source": {"written_by": "hand"},
            "expect": {"observe": {"page": null}}
        }),
    );
    fs::write(hand.join("scene.png"), b"a scene kept by hand").expect("the hand's scene");
    let hand_before = tree(&hand);
    // The package recorded again with other steps.
    let second = case_ids(&stopped_recording(&state, "rec_ac1_second", 3));

    let answer = standard_pack_adopt(
        request(&repo, &state, "rec_ac1_second", true, SECOND),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the package is adopted again: {error}"));

    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(printed["cases"]["state"], "taken", "{printed}");
    assert_eq!(
        printed["cases"]["replaced"],
        json!(destinations(&first)),
        "{printed}"
    );
    assert_eq!(
        printed["cases"]["written"],
        json!(destinations(&second)),
        "{printed}"
    );
    let kept = Path::new(&state)
        .join("adopt")
        .join(format!("{PACKAGE}.cases.old-{SECOND}"));
    let kept_text = printed["cases"]["kept"].as_str().expect("the kept place");
    assert!(
        Path::new(kept_text).ends_with(format!("{PACKAGE}.cases.old-{SECOND}")),
        "{printed}"
    );
    // The earlier set under the kept place, byte for byte the first recording's.
    assert_eq!(
        names_in(&kept),
        first
            .iter()
            .map(|id| case_directory(id))
            .collect::<Vec<_>>()
    );
    for id in &first {
        assert_eq!(
            tree(&kept.join(case_directory(id))),
            tree(&recording_cases(&state, "rec_ac1_first").join(id)),
            "{id}"
        );
    }
    // The new set in the checkout, beside the hand's case, which is untouched.
    let mut expected = second
        .iter()
        .map(|id| case_directory(id))
        .collect::<Vec<_>>();
    expected.push("neutral_hand_case".to_owned());
    expected.sort();
    assert_eq!(names_in(&regression), expected);
    for id in &second {
        assert_eq!(
            tree(&regression.join(case_directory(id))),
            tree(&recording_cases(&state, "rec_ac1_second").join(id)),
            "{id}"
        );
    }
    assert_eq!(tree(&hand), hand_before);
}

#[test]
fn ac1_a_failing_case_refuses_and_writes_nothing() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    let ids = case_ids(&stopped_recording(&state, "rec_ac1_failing", 5));
    // The content adopted differs from the one the cases were run on at the stop.
    edit_task(&recording_package(&state, "rec_ac1_failing"), whiten);
    let before = tree(&repo);

    let result = adopt_line(&repo, &state, "rec_ac1_failing", &[]);

    let details = refused_with(&result, "adopt_refused");
    let values = &details["outcome"]["values"];
    assert_eq!(values["adopt_problem"], "case_failed", "{details}");
    assert_eq!(values["count"], 5, "{details}");
    assert_eq!(values["package_id"], PACKAGE, "{details}");
    let cases = details["cases"].as_array().expect("the failing cases");
    let mut listed = cases
        .iter()
        .map(|case| {
            assert_eq!(case["verdict"], "failed", "{case}");
            assert_eq!(case["package_id"], PACKAGE, "{case}");
            case["id"].as_str().expect("a case's id").to_owned()
        })
        .collect::<Vec<_>>();
    listed.sort();
    assert_eq!(listed, ids, "{details}");
    assert_eq!(tree(&repo), before, "nothing written in the checkout");
    assert!(!Path::new(&state).join("adopt").exists(), "nothing kept");
}

#[test]
fn ac1_a_destination_that_is_no_recordings_case_refuses_before_the_first_write() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    let ids = case_ids(&stopped_recording(&state, "rec_ac1_taken", 3));
    let taken = format!("regression/{GAME}/{}", case_directory(&ids[0]));
    fs::create_dir_all(repo.join(&taken)).expect("the directory by hand");
    fs::write(repo.join(&taken).join("notes.txt"), b"kept by hand\n").expect("a file by hand");
    let before = tree(&repo);

    let result = adopt_line(&repo, &state, "rec_ac1_taken", &[]);

    let details = refused_with(&result, "adopt_refused");
    let values = &details["outcome"]["values"];
    assert_eq!(values["adopt_problem"], "case_place_taken", "{details}");
    assert_eq!(values["entry"], taken, "{details}");
    assert_eq!(tree(&repo), before, "nothing written in the checkout");
    assert!(!Path::new(&state).join("adopt").exists(), "nothing kept");
}

#[test]
fn ac1_the_dry_run_lists_the_cases_and_the_two_words_write_as_before() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let ids = case_ids(&stopped_recording(&state, "rec_ac1_words", 3));
    let directories = destinations(&ids);
    let repos = ["repo-taken", "repo-none-found", "repo-package-path"]
        .map(|name| isolated.temp.path().join(name));
    for repo in &repos {
        checkout(repo);
        fs::create_dir_all(repo.join("regression")).expect("the regression directory");
    }
    let [taken_repo, found_repo, path_repo] = &repos;

    // Under --dry-run nothing is written, the cases are run and listed.
    let before = tree(taken_repo);
    let dry = answered(&adopt_line(
        taken_repo,
        &state,
        "rec_ac1_words",
        &["--dry-run"],
    ));
    assert_eq!(dry["status"], "validated", "{dry}");
    assert_eq!(dry["written"], json!([]), "{dry}");
    let mut would_write = vec![json!(format!("packs/{PACKAGE}")), json!("schedule.json")];
    would_write.extend(directories.iter().map(|directory| json!(directory)));
    assert_eq!(dry["would_write"], Value::Array(would_write), "{dry}");
    assert_eq!(dry["cases"]["state"], "taken", "{dry}");
    assert_eq!(dry["cases"]["written"], json!([]), "{dry}");
    assert_eq!(dry["cases"]["would_write"], json!(directories), "{dry}");
    assert_eq!(tree(taken_repo), before, "the dry run writes nothing");

    // The cases taken: `next` names no regression case.
    let taken = answered(&adopt_line(taken_repo, &state, "rec_ac1_words", &[]));
    assert_eq!(taken["cases"]["written"], json!(directories), "{taken}");
    assert_eq!(taken["next"], json!([]), "{taken}");

    // The recording with its cases directory set aside: none found, as before this slice.
    fs::rename(
        recording_cases(&state, "rec_ac1_words"),
        isolated.temp.path().join("set-aside.cases"),
    )
    .expect("the cases set aside");
    let found = answered(&adopt_line(found_repo, &state, "rec_ac1_words", &[]));
    // The package by its path: it carries none.
    let package = recording_package(&state, "rec_ac1_words");
    let package_text = package.to_str().expect("a UTF-8 path");
    let path_text = path_repo.to_str().expect("a UTF-8 path");
    let by_path = answered(&crate::run_cli(
        vec![
            "--json",
            "standard-pack",
            "adopt",
            "--repo",
            path_text,
            "--package",
            package_text,
            "--state-dir",
            state.as_str(),
            "--held-reason",
            REASON,
        ],
        true,
    ));
    for (answer, word) in [(&found, "none_found"), (&by_path, "package_path")] {
        assert_eq!(
            answer["cases"],
            json!({"state": word, "written": [], "would_write": [], "replaced": [],
                   "not_proven": []}),
            "{answer}"
        );
        assert_eq!(
            answer["written"],
            json!([format!("packs/{PACKAGE}"), "schedule.json"]),
            "{answer}"
        );
        assert_eq!(
            answer["next"],
            json!([{"path": format!("regression/{GAME}/")}]),
            "{answer}"
        );
    }
    // Byte for byte what the adoption that took the cases wrote of the package and the root file.
    let without_cases = tree(taken_repo)
        .into_iter()
        .filter(|(path, _)| !path.starts_with("regression/"))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(tree(found_repo), without_cases);
    assert_eq!(tree(path_repo), without_cases);
    for repo in [found_repo, path_repo] {
        assert!(names_in(&repo.join("regression")).is_empty());
    }
}

#[test]
fn ac1_an_adoption_stopped_after_its_first_case_leaves_it_and_the_next_call_answers_the_contract() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    let ids = case_ids(&stopped_recording(&state, "rec_ac1_stopped", 5));
    let regression = repo.join("regression").join(GAME);
    let stops = StopsAfterTheFirstCase {
        regression: regression.clone(),
        stopped: Cell::new(false),
    };

    let error = standard_pack_adopt(
        request(&repo, &state, "rec_ac1_stopped", false, SECOND),
        &stops,
    )
    .expect_err("the test stops the adoption");

    let first = case_directory(&ids[0]);
    let outcome = error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .cloned()
        .unwrap_or_else(|| panic!("the failure carries its outcome: {error}"));
    assert_eq!(outcome["code"], "adopt_write_failed", "{error}");
    let stopped_at = outcome["values"]["path"]
        .as_str()
        .expect("where it stopped");
    assert!(Path::new(stopped_at).ends_with(&first), "{error}");
    // What it leaves: the package directory and the root file written, the first case in place,
    // nothing else of the cases and no `.part`.
    assert!(repo.join("packs").join(PACKAGE).is_dir());
    let schedule = serde_json::from_slice::<Value>(
        &fs::read(repo.join("schedule.json")).expect("schedule.json"),
    )
    .expect("schedule.json reads");
    assert_eq!(schedule["held"][0]["package_id"], PACKAGE, "{schedule}");
    assert_eq!(names_in(&regression), [first.as_str()]);
    assert_eq!(
        tree(&regression.join(&first)),
        tree(&recording_cases(&state, "rec_ac1_stopped").join(&ids[0]))
    );

    // The next call without --replace: the package was adopted.
    let refused = standard_pack_adopt(
        request(&repo, &state, "rec_ac1_stopped", false, THIRD),
        &Committed,
    )
    .expect_err("the package was adopted");
    let refused_outcome = refused
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .cloned()
        .unwrap_or_else(|| panic!("the refusal carries its outcome: {refused}"));
    assert_eq!(
        refused_outcome["values"]["adopt_problem"], "pack_exists",
        "{refused}"
    );
    assert_eq!(
        refused_outcome["values"]["entry"],
        format!("packs/{PACKAGE}"),
        "{refused}"
    );

    // With --replace, once the person committed what that call wrote: the case it put in place is
    // moved to the kept place and the whole set is written.
    let answer = standard_pack_adopt(
        request(&repo, &state, "rec_ac1_stopped", true, THIRD),
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the adoption is run again: {error}"));
    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(
        printed["cases"]["replaced"],
        json!([format!("regression/{GAME}/{first}")]),
        "{printed}"
    );
    assert_eq!(
        printed["cases"]["written"],
        json!(destinations(&ids)),
        "{printed}"
    );
    let kept = Path::new(&state)
        .join("adopt")
        .join(format!("{PACKAGE}.cases.old-{THIRD}"));
    assert_eq!(names_in(&kept), [first.as_str()]);
    assert_eq!(
        names_in(&regression),
        ids.iter().map(|id| case_directory(id)).collect::<Vec<_>>()
    );
}

#[test]
fn ac1_s317_a_draft_recording_is_refused_with_record_readys_missing() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    marked_recording(&state, "rec_ac1_draft", 3);
    let (code, ready) = record(&state, "ready", &["--record-id", "rec_ac1_draft"]);
    assert_eq!(code, 0, "{ready}");
    assert_eq!(ready["status"], "draft", "{ready}");
    let before = tree(&repo);

    let result = adopt_line(&repo, &state, "rec_ac1_draft", &[]);

    let details = refused_with(&result, "adopt_refused");
    assert_eq!(
        details["outcome"]["values"]["adopt_problem"], "recording_not_current",
        "{details}"
    );
    assert_eq!(details["record_ready"]["status"], "draft", "{details}");
    assert_eq!(
        details["record_ready"]["missing"], ready["missing"],
        "{details}"
    );
    assert_eq!(
        details["record_ready"]["missing_total"], ready["missing_total"],
        "{details}"
    );
    assert_eq!(tree(&repo), before, "nothing written in the checkout");
}

#[test]
fn ac1_s317_recording_not_current_against_record_ready() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    // Marked again after its stop.
    stopped_recording(&state, "rec_ac1_amended", 3);
    let (code, marked) = record(
        &state,
        "mark",
        &["--step", "1", "--color", "again_color=12,0,4,4"],
    );
    assert_eq!(code, 0, "{marked}");
    // Its package moved away after its stop.
    stopped_recording(&state, "rec_ac1_moved", 3);
    fs::rename(
        recording_package(&state, "rec_ac1_moved"),
        isolated.temp.path().join("moved-package.json"),
    )
    .expect("the package moved away");
    // Packaged.
    stopped_recording(&state, "rec_ac1_packaged", 3);
    // Active, never stopped.
    marked_recording(&state, "rec_ac1_active", 3);

    // Each: what the old check read, record ready's status, and the adoption; the packaged one
    // last, since it writes.
    for (record_id, old_takes, status_packaged) in [
        ("rec_ac1_amended", false, false),
        ("rec_ac1_moved", false, true),
        ("rec_ac1_active", false, false),
        ("rec_ac1_packaged", true, true),
    ] {
        let recording = serde_json::from_slice::<Value>(
            &fs::read(lab_dir(&state, record_id).join("recording.json")).expect("recording.json"),
        )
        .expect("recording.json reads");
        let old = recording["status"] == "stopped"
            && recording
                .get("amended_at_unix_ms")
                .is_none_or(Value::is_null)
            && recording_package(&state, record_id).is_file();
        assert_eq!(old, old_takes, "{record_id}: {recording}");
        let (code, ready) = record(&state, "ready", &["--record-id", record_id]);
        assert_eq!(code, 0, "{record_id}: {ready}");
        assert_eq!(
            ready["status"] == "packaged",
            status_packaged,
            "{record_id}: {ready}"
        );

        let result = adopt_line(&repo, &state, record_id, &[]);

        if old_takes {
            let data = answered(&result);
            assert_eq!(data["status"], "adopted", "{record_id}: {data}");
            continue;
        }
        let details = refused_with(&result, "adopt_refused");
        let values = &details["outcome"]["values"];
        assert_eq!(
            values["adopt_problem"], "recording_not_current",
            "{record_id}: {details}"
        );
        if status_packaged {
            // The difference: record ready calls it packaged, the old check of its artifact, kept
            // for it, refuses it.
            assert_eq!(values["member"], "artifact", "{record_id}: {details}");
            assert!(
                details.get("record_ready").is_none(),
                "{record_id}: {details}"
            );
        } else {
            // The one function refuses it, with its answer.
            assert_eq!(
                details["record_ready"]["status"], ready["status"],
                "{record_id}: {details}"
            );
            assert_eq!(
                details["record_ready"]["missing"], ready["missing"],
                "{record_id}: {details}"
            );
        }
    }
}

#[test]
fn ac1_s317_recording_unreviewed_against_record_ready() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    stopped_recording(&state, "rec_ac1_partial", 3);
    // Its package says that its cross-check was only partly evaluated.
    edit_task(&recording_package(&state, "rec_ac1_partial"), |task| {
        task["provenance"]["cross_check"]["status"] = json!("partially_evaluated");
    });
    let (code, ready) = record(&state, "ready", &["--record-id", "rec_ac1_partial"]);
    assert_eq!(code, 0, "{ready}");
    assert_eq!(ready["status"], "packaged", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    let decision = isolated.temp.path().join("decision.json");
    write_json(
        &decision,
        &json!({
            "schema_version": "actingcommand.pack-decision.v1",
            "class": "business",
            "trigger": {
                "kind": "clock",
                "schedule": {
                    "kind": "interval",
                    "clock_source": {"kind": "local"},
                    "every_ms": 86400000,
                    "anchor_ms": 0
                }
            },
            "value": {"fixed_value_milli": 9000, "origin": "pack"}
        }),
    );
    let repo_text = repo.to_str().expect("a UTF-8 path");
    let decision_text = decision.to_str().expect("a UTF-8 path");

    // The review, kept whole beside record ready, refuses it for a class other than held.
    let business = crate::run_cli(
        vec![
            "--json",
            "standard-pack",
            "adopt",
            "--repo",
            repo_text,
            "--record-id",
            "rec_ac1_partial",
            "--state-dir",
            state.as_str(),
            "--decide",
            decision_text,
        ],
        true,
    );
    let details = refused_with(&business, "adopt_refused");
    assert_eq!(
        details["outcome"]["values"]["adopt_problem"], "recording_unreviewed",
        "{details}"
    );
    assert_eq!(
        details["review"]["recording_checked"],
        json!(false),
        "{details}"
    );
    assert_eq!(
        details["review"]["cross_check"], "partially_evaluated",
        "{details}"
    );
    // ... and takes it as held, as before.
    let held = answered(&adopt_line(&repo, &state, "rec_ac1_partial", &[]));
    assert_eq!(held["class"], "held", "{held}");
    assert_eq!(held["review"]["recording_checked"], json!(false), "{held}");
}

#[test]
fn ac1_fix1_a_lab_failure_refuses_and_an_unreadable_recording_falls_back() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    stopped_recording(&state, "rec_ac1_judged", 3);
    let before = tree(&repo);

    // `record ready`'s function refuses the packaged recording otherwise than as one it cannot
    // read whole: a failure on the Lab's side is no pass.
    let gone = RecordingGone {
        elsewhere: isolated.temp.path().join("another-state"),
    };
    let error = standard_pack_adopt(
        request(&repo, &state, "rec_ac1_judged", false, SECOND),
        &gone,
    )
    .expect_err("a failure on the Lab's side refuses the adoption");
    assert_eq!(error.code, "adopt_refused", "{error}");
    let outcome = error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .cloned()
        .unwrap_or_else(|| panic!("the refusal carries its outcome: {error}"));
    assert_eq!(
        outcome["values"]["adopt_problem"], "recording_not_judged",
        "{error}"
    );
    let at = outcome["values"]["path"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal names the recording: {error}"));
    assert!(Path::new(at).ends_with("recording.json"), "{error}");
    // The Lab's refusal is its cause, and the message says it.
    assert!(
        outcome["causes"]
            .as_array()
            .is_some_and(|causes| !causes.is_empty()),
        "{error}"
    );
    assert!(error.message.contains("lab_recording_missing"), "{error}");
    assert_eq!(tree(&repo), before, "nothing written in the checkout");
    assert!(!Path::new(&state).join("adopt").exists(), "nothing kept");

    // A member added to its `recording.json` by hand: the Lab cannot read it whole, so the check of
    // its own members judges it (stopped, not marked again), and it is adopted, said in
    // `review.ready_not_read`.
    let file = lab_dir(&state, "rec_ac1_judged").join("recording.json");
    let mut recording = serde_json::from_slice::<Value>(&fs::read(&file).expect("recording.json"))
        .expect("recording.json reads");
    recording["note"] = json!("edited by hand");
    fs::write(
        file,
        serde_json::to_vec_pretty(&recording).expect("recording.json writes"),
    )
    .expect("recording.json written");

    let data = answered(&adopt_line(&repo, &state, "rec_ac1_judged", &[]));

    assert_eq!(data["status"], "adopted", "{data}");
    let text = data["review"]["ready_not_read"]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("said in review.ready_not_read: {data}"));
    assert!(text.contains("lab_recording_unreadable"), "{data}");
    assert_eq!(data["cases"]["state"], "taken", "{data}");
}

#[test]
fn ac1_fix1_failing_cases_beyond_the_bound_are_listed_as_resource_regression_lists_them() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    let ids = case_ids(&stopped_recording(&state, "rec_ac1_many", 5));
    // Two copies of each case beside it in the recording's set: fifteen cases.
    let set = recording_cases(&state, "rec_ac1_many");
    for id in &ids {
        let files = tree(&set.join(id));
        for copy in ["copy_a", "copy_b"] {
            let into = set.join(format!("{id}_{copy}"));
            for (name, bytes) in &files {
                let target = into.join(name);
                fs::create_dir_all(target.parent().expect("a parent")).expect("the copy");
                fs::write(target, bytes).expect("a file of the copy");
            }
        }
    }
    let names = names_in(&set);
    assert_eq!(names.len(), 15, "{names:?}");
    // Every scene changed after the stop: no case's scene is the one its `case.json` pins.
    // The 0.12.71 train: a copy's `case.json` keeps the `id` of the case it copies, so ten cases
    // repeat an id. The reader reads `id` as a text it answers and nothing else; the runner reads
    // and runs each case directory by itself, by its own path; adoption names a case by its
    // directory; none of them compares two cases' ids. So the fifteen are fifteen cases, each run
    // and listed by its own directory, two rows of one id told apart by their paths.
    let mut id_of = BTreeMap::new();
    for name in &names {
        let case = serde_json::from_slice::<Value>(
            &fs::read(set.join(name).join("case.json")).expect("case.json"),
        )
        .expect("case.json reads");
        id_of.insert(name.clone(), case["id"].clone());
        let scene = set
            .join(name)
            .join(case["scene"].as_str().expect("the case names its scene"));
        let mut bytes = fs::read(&scene).expect("the scene");
        bytes.push(0);
        fs::write(scene, bytes).expect("the scene changed");
    }
    let before = tree(&repo);

    let result = adopt_line(&repo, &state, "rec_ac1_many", &[]);

    let details = refused_with(&result, "adopt_refused");
    let values = &details["outcome"]["values"];
    assert_eq!(values["adopt_problem"], "case_failed", "{details}");
    assert_eq!(values["count"], 15, "{details}");
    assert_eq!(details["failed_total"], 15, "{details}");
    assert_eq!(details["listed"], json!(ROWS_LISTED_MAX), "{details}");
    let rows = details["cases"].as_array().expect("the rows listed");
    // The first of them in the order of their names, each a short row as `resource regression`
    // lists one: by the directory it would be written to, failed, and no check with its `detail`.
    let expected = names
        .iter()
        .take(ROWS_LISTED_MAX)
        .map(|name| json!(format!("regression/{GAME}/{}", case_directory(name))))
        .collect::<Vec<_>>();
    assert_eq!(
        rows.iter()
            .map(|row| row["path"].clone())
            .collect::<Vec<_>>(),
        expected,
        "{details}"
    );
    for (row, name) in rows.iter().zip(&names) {
        assert_eq!(row["verdict"], "failed", "{row}");
        assert_eq!(row["package_id"], PACKAGE, "{row}");
        // The 0.12.71 train: each row carries the `id` its own `case.json` gives, the copied
        // case's for a copy.
        assert_eq!(row["id"], id_of[name], "{row}");
        let checks = row["checks"].as_array().expect("the checks not passed");
        assert!(!checks.is_empty(), "{row}");
        assert!(
            checks.iter().all(|check| check.get("detail").is_none()),
            "{row}"
        );
    }
    // Among the rows listed, cases that share one id: each its own row.
    let listed_ids = rows
        .iter()
        .filter_map(|row| row["id"].as_str())
        .collect::<BTreeSet<_>>();
    assert!(listed_ids.len() < rows.len(), "{details}");
    assert_eq!(tree(&repo), before, "nothing written in the checkout");
    assert!(!Path::new(&state).join("adopt").exists(), "nothing kept");
}
