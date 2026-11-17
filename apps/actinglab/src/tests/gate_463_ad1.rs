// SPDX-License-Identifier: AGPL-3.0-only

// #463 AD-1 (register §1.464; contracts/standard-pack-authoring.md, "The recordings' directory" and
// "The warnings of other packages"; coordinator work order rd-1117/brief-463-ad1-adopt-recording-dir.md):
// `standard-pack adopt --record-id` reads its recording from `--session-state-dir` when it is given,
// only reading it, while everything it writes or moves stays under `--state-dir`; and its answer
// gives the warnings of the adopted package whole and counts those of the root's other packages by
// code. The recordings are made as gate_463_ac1 makes them: screens of a 4x4 square of one colour
// each through the Lab's recording API (a tap on each but the last), the session, every mark and
// the stop through the command line; the checkouts are made-up resource repositories of the game
// `neutral`, server `test`; every adoption is of the class held.
// - `ad1_a_recording_in_its_own_directory_is_adopted_and_replaced_into_the_state_directory`: two
//   recordings in one directory, the state directory another. By the command line with
//   `--session-state-dir` the first is adopted: judged by `record ready`'s function there (no
//   `ready_not_read`), its five cases run and written byte for byte, the check tree under the state
//   directory. Then with `--replace` from the second: the old pack directory and the old case set
//   moved into the state directory's `adopt/`, the new set written. The recordings' directory, every
//   file and directory of it, is what it was before the first adoption.
// - `ad1_without_the_option_a_recording_outside_the_state_directory_is_not_found`: the same line
//   without `--session-state-dir` is refused `recording_not_current` at the state directory's
//   `record-artifacts/<record>/lab/recording.json`, as before; nothing written in the checkout.
// - `ad1_warnings_of_other_packages_are_counted_and_the_adopted_packages_answered_whole`: a checkout
//   of three held packages and a package adopted by `--package`, each with a remedy no package of
//   the root provides (one `package_bundle_remedy_invalid` each): the adopted package's warning is
//   in `warnings` whole, no warning there names another package, `other_packages_warnings` counts
//   the three by code with its total; the dry run answers the same two members; `standard-pack
//   check` on the check tree's game root finds the three that were counted.
// - `ad1_the_option_beside_package_is_refused_before_anything_is_read`: `--session-state-dir`
//   beside `--package` is `validation_failed`, exit 2, naming it, `argument_problem`
//   `conflicting`, as `--held-reference` without `--held-reason` is; none of the paths the line
//   names is made.
// No Runtime (the state root's variable names a folder that does not exist), no device, no clock,
// no sleep: made-up recordings, checkouts and values.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use crate::cli_module::own_line;
use crate::standard_pack_cli::LabAdoption;
use actingcommand_contract::canonical_json_bytes;
use actingcommand_contract::outcome::Outcome;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::{
    AttachFrameRequest, ClickEffect, CommitClickRequest, OpaqueJson, PlanClickRequest, RecordRect,
    RecordingLock, record_attach_frame, record_commit_click, record_plan_click,
};
use actingcommand_resource_tooling::{
    AdoptLab, AdoptSource, AdoptWorktree, StandardPackAdoptRequest, standard_pack_adopt,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
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
/// The time the direct adoption gives itself, which names its kept places.
const SECOND: u64 = 2_000;
/// Test 3: the root's packages beside the adopted one, the adopted one, and what each lacks.
const OTHERS: [&str; 3] = [
    "neutral.test.other_a",
    "neutral.test.other_b",
    "neutral.test.other_c",
];
const ADOPTED: &str = "neutral.test.adopted";
const REMEDY_INVALID: &str = "package_bundle_remedy_invalid";
const UNPROVIDED: &str = "capability_nobody_provides";

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

/// What `actinglab --json --instance neutral.a record <action> --state-dir <session> <more>`
/// prints: its exit code and its line.
fn record(session: &str, action: &str, more: &[&str]) -> (u8, Value) {
    let mut line = vec![
        "--json",
        "--instance",
        INSTANCE,
        "record",
        action,
        "--state-dir",
        session,
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
fn step(session: &str, color: [u8; 3], request: Option<&str>) {
    let lock = RecordingLock::acquire(Path::new(session), INSTANCE, "gate_463_ad1", "Lab-CLI")
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

/// The recording `record_id` under `session`, stopped with the game, server and locale of the
/// checkout: the first `screens` of [`SCREENS`], a tap on each but the last, each step marked with
/// a colour mark of its square; its package [`PACKAGE`], a JSON container, and its cases written.
/// The ids of its cases, in the order of their names.
fn stopped_recording(session: &str, record_id: &str, screens: usize) -> Vec<String> {
    let (code, started) = record(
        session,
        "start",
        &["--task-id", TASK, "--record-id", record_id],
    );
    assert_eq!(code, 0, "{started}");
    for (index, (_, color)) in SCREENS.iter().take(screens).enumerate() {
        let request = format!("{record_id}-request-{index}");
        let tapped = index + 1 < screens;
        step(session, *color, tapped.then_some(request.as_str()));
    }
    for (index, (name, _)) in SCREENS.iter().take(screens).enumerate() {
        let number = (index + 1).to_string();
        let mark = format!("{name}_color=0,0,4,4");
        let (code, marked) = record(
            session,
            "mark",
            &["--step", number.as_str(), "--color", mark.as_str()],
        );
        assert_eq!(code, 0, "{marked}");
    }
    let (code, stopped) = record(
        session,
        "stop",
        &["--game", GAME, "--server", SERVER, "--locale", LOCALE],
    );
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["lab"]["status"], "generated", "{stopped}");
    assert_eq!(stopped["lab"]["package_id"], PACKAGE, "{stopped}");
    let mut ids = stopped["lab"]["cases"]["ids"]
        .as_array()
        .unwrap_or_else(|| panic!("the stop answers its cases: {stopped}"))
        .iter()
        .map(|id| id.as_str().expect("a case id").to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids.len(), screens, "{stopped}");
    ids
}

/// The recording's Lab directory, `<session>/record-artifacts/<record_id>/lab`.
fn lab_dir(session: &str, record_id: &str) -> PathBuf {
    Path::new(session)
        .join("record-artifacts")
        .join(record_id)
        .join("lab")
}

/// The recording's cases, `out/<package_id>.cases/`.
fn recording_cases(session: &str, record_id: &str) -> PathBuf {
    lab_dir(session, record_id)
        .join("out")
        .join(format!("{PACKAGE}.cases"))
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

/// `applications.json` of the game [`GAME`], server [`SERVER`], and `maintenance.json` (`[]`) at
/// `repo`, with an empty `packs/`.
fn root_files(repo: &Path) {
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
}

/// A made-up checkout at `repo`: [`root_files`] and `schedule.json` without a rule.
fn checkout(repo: &Path) {
    root_files(repo);
    write_json(
        &repo.join("schedule.json"),
        &json!({"schema_version": "actingcommand.schedule.v1", "rules": []}),
    );
}

/// `standard-pack adopt` of the recording `record_id` into `repo` as held, the state directory
/// `state`, and `--session-state-dir <session>` when `session` is given.
fn record_id_line(
    repo: &Path,
    state: &str,
    session: Option<&str>,
    record_id: &str,
) -> crate::CliResult {
    let repo = repo.to_str().expect("a UTF-8 path");
    let mut line = vec![
        "--json",
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
    ];
    if let Some(session) = session {
        line.extend(["--session-state-dir", session]);
    }
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

/// Every entry below `root`, by its `/`-separated path relative to it: a file with its bytes, a
/// directory with none.
fn entries(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut found = BTreeMap::new();
    gather(root, root, &mut found);
    found
}

fn gather(root: &Path, directory: &Path, found: &mut BTreeMap<String, Option<Vec<u8>>>) {
    let Ok(listed) = fs::read_dir(directory) else {
        return;
    };
    for entry in listed {
        let path = entry.expect("a directory entry").path();
        let relative = path
            .strip_prefix(root)
            .expect("below the root")
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if path.is_dir() {
            found.insert(relative, None);
            gather(root, &path, found);
        } else {
            found.insert(relative, Some(fs::read(&path).expect("the file")));
        }
    }
}

/// Every file below `root`, by its `/`-separated path relative to it, with its bytes.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    entries(root)
        .into_iter()
        .filter_map(|(path, bytes)| bytes.map(|bytes| (path, bytes)))
        .collect()
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

/// Test 3: the source package `package_id` at `directory`: a route of 16x9 from `home` (a blue
/// first pixel) to `page_b` (a red one), where its run ends on the key `blocked_k`, whose remedy
/// names a capability no package of the root provides (as gate_411_b15b writes such a package).
fn remedy_pack(directory: &Path, package_id: &str) {
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
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
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": LOCALE,
        "goal": "AD-1 fixture",
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
            "outcome_key": "blocked_k",
            "effect": "no_designated_effect",
            "terminal_pages": ["page_b"],
            "remedy": {"need": "need_neutral", "capabilities": [UNPROVIDED]}
        }]},
        "operations": [{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("the pack directory");
        fs::write(path, serde_json::to_vec_pretty(&value).expect("JSON")).expect("a pack file");
    }
}

/// Test 3: `standard-pack adopt` of the package at `package` into `repo` as held, the global flags
/// `global` before the command.
fn package_line(repo: &Path, state: &str, package: &Path, global: &[&str]) -> crate::CliResult {
    let repo = repo.to_str().expect("a UTF-8 path");
    let package = package.to_str().expect("a UTF-8 path");
    let mut line = vec!["--json"];
    line.extend(global);
    line.extend([
        "standard-pack",
        "adopt",
        "--repo",
        repo,
        "--package",
        package,
        "--state-dir",
        state,
        "--held-reason",
        REASON,
    ]);
    crate::run_cli(line, true)
}

/// Test 3: the package id a warning of an answer names by its value, if any.
fn named_by(warning: &Value) -> Option<&str> {
    warning["values"]["package_id"].as_str()
}

#[test]
fn ad1_a_recording_in_its_own_directory_is_adopted_and_replaced_into_the_state_directory() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let session = isolated.path("session");
    let state = isolated.path("adoption-state");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    let first = stopped_recording(&session, "rec_ad1_first", 5);
    let second = stopped_recording(&session, "rec_ad1_second", 3);
    let recordings_before = entries(Path::new(&session));

    // By the command line, the recording read from its own directory.
    let data = answered(&record_id_line(
        &repo,
        &state,
        Some(session.as_str()),
        "rec_ad1_first",
    ));

    assert_eq!(data["status"], "adopted", "{data}");
    // Judged by record ready's function on the recordings' directory: it read the recording.
    assert!(data["review"].get("ready_not_read").is_none(), "{data}");
    assert_eq!(data["review"]["record_id"], "rec_ad1_first", "{data}");
    // The cases read from the recording, run, and written byte for byte.
    assert_eq!(data["cases"]["state"], "taken", "{data}");
    assert_eq!(
        data["cases"]["written"],
        json!(destinations(&first)),
        "{data}"
    );
    assert_eq!(data["cases"]["not_proven"], json!([]), "{data}");
    let from = data["cases"]["from"]
        .as_str()
        .expect("where the cases came from");
    assert_eq!(
        PathBuf::from(from),
        recording_cases(&session, "rec_ad1_first"),
        "{data}"
    );
    let regression = repo.join("regression").join(GAME);
    for id in &first {
        assert_eq!(
            tree(&regression.join(case_directory(id))),
            tree(&recording_cases(&session, "rec_ad1_first").join(id)),
            "{id}"
        );
    }
    // What it writes for itself lies under the state directory.
    let check_tree = data["check_tree"].as_str().expect("the check tree");
    assert!(Path::new(check_tree).starts_with(&state), "{data}");
    let first_pack = tree(&repo.join("packs").join(PACKAGE));
    assert!(first_pack.contains_key("control.json"), "{first_pack:?}");

    // Again with --replace, from the second recording of the same directory.
    let answer = standard_pack_adopt(
        StandardPackAdoptRequest {
            repo: repo.clone(),
            source: AdoptSource::Recording {
                record_id: "rec_ad1_second".to_owned(),
                lab_dir: lab_dir(&session, "rec_ad1_second"),
                session_state_dir: PathBuf::from(&session),
            },
            decision: None,
            replace: true,
            dry_run: false,
            state_dir: PathBuf::from(&state),
            at_unix_ms: SECOND,
        },
        &Committed,
    )
    .unwrap_or_else(|error| panic!("the package is adopted again: {error}"));

    let printed = serde_json::to_value(&answer).expect("the answer serializes");
    assert_eq!(printed["replaced"], json!(true), "{printed}");
    assert!(
        printed["review"].get("ready_not_read").is_none(),
        "{printed}"
    );
    // The old pack directory, moved into the state directory.
    let kept = Path::new(&state)
        .join("adopt")
        .join(format!("{PACKAGE}.old-{SECOND}"));
    let kept_text = printed["kept"].as_str().expect("the kept place");
    assert_eq!(PathBuf::from(kept_text), kept, "{printed}");
    assert_eq!(tree(&kept), first_pack);
    // The old case set, moved beside it; the new one written.
    let cases_kept = Path::new(&state)
        .join("adopt")
        .join(format!("{PACKAGE}.cases.old-{SECOND}"));
    let cases_kept_text = printed["cases"]["kept"]
        .as_str()
        .expect("the cases' kept place");
    assert_eq!(PathBuf::from(cases_kept_text), cases_kept, "{printed}");
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
    assert_eq!(
        names_in(&cases_kept),
        first
            .iter()
            .map(|id| case_directory(id))
            .collect::<Vec<_>>()
    );
    for id in &first {
        assert_eq!(
            tree(&cases_kept.join(case_directory(id))),
            tree(&recording_cases(&session, "rec_ad1_first").join(id)),
            "{id}"
        );
    }
    assert_eq!(
        names_in(&regression),
        second
            .iter()
            .map(|id| case_directory(id))
            .collect::<Vec<_>>()
    );
    for id in &second {
        assert_eq!(
            tree(&regression.join(case_directory(id))),
            tree(&recording_cases(&session, "rec_ad1_second").join(id)),
            "{id}"
        );
    }

    // Nothing in the recordings' directory changed: every file and directory as it was.
    assert_eq!(entries(Path::new(&session)), recordings_before);
}

#[test]
fn ad1_without_the_option_a_recording_outside_the_state_directory_is_not_found() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let session = isolated.path("session");
    let state = isolated.path("adoption-state");
    let repo = isolated.temp.path().join("repo");
    checkout(&repo);
    stopped_recording(&session, "rec_ad1_elsewhere", 3);
    let before = tree(&repo);

    let result = record_id_line(&repo, &state, None, "rec_ad1_elsewhere");

    let details = refused_with(&result, "adopt_refused");
    let values = &details["outcome"]["values"];
    assert_eq!(
        values["adopt_problem"], "recording_not_current",
        "{details}"
    );
    let at = values["path"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal names where it looked: {details}"));
    assert_eq!(
        PathBuf::from(at),
        lab_dir(&state, "rec_ad1_elsewhere").join("recording.json"),
        "{details}"
    );
    assert_eq!(tree(&repo), before, "nothing written in the checkout");
}

#[test]
fn ad1_warnings_of_other_packages_are_counted_and_the_adopted_packages_answered_whole() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("adoption-state");
    let repo = isolated.temp.path().join("repo");
    root_files(&repo);
    for other in OTHERS {
        remedy_pack(&repo.join("packs").join(other), other);
    }
    let held = OTHERS
        .iter()
        .map(|other| json!({"package_id": other, "reason": REASON}))
        .collect::<Vec<_>>();
    write_json(
        &repo.join("schedule.json"),
        &json!({"schema_version": "actingcommand.schedule.v1", "rules": [], "held": held}),
    );
    let package = isolated.temp.path().join("adopted-package");
    remedy_pack(&package, ADOPTED);

    let dry = answered(&package_line(&repo, &state, &package, &["--dry-run"]));
    let adopted = answered(&package_line(&repo, &state, &package, &[]));

    assert_eq!(dry["status"], "validated", "{dry}");
    assert_eq!(adopted["status"], "adopted", "{adopted}");
    for answer in [&dry, &adopted] {
        let warnings = answer["warnings"].as_array().expect("the warnings");
        // The adopted package's warning, whole.
        let own = warnings
            .iter()
            .filter(|warning| {
                warning["code"] == REMEDY_INVALID && named_by(warning) == Some(ADOPTED)
            })
            .collect::<Vec<_>>();
        assert_eq!(own.len(), 1, "{answer}");
        assert_eq!(
            own[0]["values"]["remedy_problem"], "capability_unprovided",
            "{answer}"
        );
        assert_eq!(own[0]["values"]["capability"], UNPROVIDED, "{answer}");
        assert_eq!(own[0]["values"]["outcome_key"], "blocked_k", "{answer}");
        // None of another package's.
        for warning in warnings {
            assert!(
                named_by(warning).is_none_or(|named| !OTHERS.contains(&named)),
                "{warning}"
            );
        }
        // Those counted by code, with their total.
        let others = &answer["other_packages_warnings"];
        assert_eq!(others["counts"][REMEDY_INVALID], json!(3), "{answer}");
        let counted = others["counts"]
            .as_object()
            .expect("the counts")
            .values()
            .map(|count| count.as_u64().expect("a count"))
            .sum::<u64>();
        assert_eq!(others["total"], json!(counted), "{answer}");
    }
    // The dry run answers the same.
    assert_eq!(dry["warnings"], adopted["warnings"]);
    assert_eq!(
        dry["other_packages_warnings"],
        adopted["other_packages_warnings"]
    );

    // Nothing was dropped: the check of the check tree's game root finds the three counted.
    let installed = Path::new(adopted["check_tree"].as_str().expect("the check tree"))
        .join("installed")
        .join(GAME);
    let root = installed.to_str().expect("a UTF-8 path");
    let checked = answered(&crate::run_cli(
        vec!["--json", "standard-pack", "check", "--root", root],
        true,
    ));
    let of_others = checked["warnings"]
        .as_array()
        .expect("the check's warnings")
        .iter()
        .filter(|warning| {
            warning["code"] == REMEDY_INVALID
                && named_by(warning).is_some_and(|named| OTHERS.contains(&named))
        })
        .count();
    assert_eq!(of_others, 3, "{checked}");
}

#[test]
fn ad1_the_option_beside_package_is_refused_before_anything_is_read() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let repo = isolated.path("neutral-repo");
    let package = isolated.path("neutral-package");
    let state = isolated.path("neutral-state");
    let session = isolated.path("neutral-session");
    for flags in [
        [
            "--package",
            package.as_str(),
            "--session-state-dir",
            session.as_str(),
        ],
        [
            "--session-state-dir",
            session.as_str(),
            "--package",
            package.as_str(),
        ],
    ] {
        let mut line = vec![
            "--json",
            "standard-pack",
            "adopt",
            "--repo",
            repo.as_str(),
            "--state-dir",
            state.as_str(),
            "--held-reason",
            REASON,
        ];
        line.extend(flags);
        let result = crate::run_cli(line.clone(), true);
        let shown = result.envelope_json();
        assert_eq!(result.exit_code(), 2, "{line:?}: {shown}");
        assert!(result.envelope.data.is_none(), "{line:?}: {shown}");
        let error = result
            .envelope
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("{line:?} was not refused: {shown}"));
        assert_eq!(error.code, "validation_failed", "{line:?}: {shown}");
        let details = error.details.clone().unwrap_or(Value::Null);
        assert_eq!(details["argument"], "--session-state-dir", "{shown}");
        assert_eq!(details["argument_problem"], "conflicting", "{shown}");
        assert_eq!(
            details["arguments"],
            json!([{"argument": "--session-state-dir", "argument_problem": "conflicting"}]),
            "{shown}"
        );
    }
    for path in [&repo, &package, &state, &session] {
        assert!(!Path::new(path).exists(), "{path} was made");
    }
    assert!(
        !isolated.runtime.exists(),
        "a refusal reached the Runtime's state root"
    );
}
