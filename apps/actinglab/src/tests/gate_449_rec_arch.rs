// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #449 REC-ARCH (contracts/lab-recording.md, "Archiving"; register §1.344):
// `record archive` ends a recording as material only and `record reopen` makes it
// active again, on actinglab's command line. The steps are recorded through the Lab's recording
// API as `observe --capture --record` and `do --capture --record` record them (no Runtime); the
// session and its header through the command line.
// - `rec_arch_record_archive_ends_a_recording_that_record_stop_refuses`: steps of two initiators
//   without marks, the last with a tap: `record stop` refuses as before
//   (`record_final_step_has_click`) and writes nothing; `record archive` ends it, the manifest's
//   rows the step table's steps; `record status` shows the session archived; `record start` then
//   starts another without `--force`; every frame and the step table keep their bytes.
// - `rec_arch_the_copy_goes_only_into_a_new_or_an_empty_directory`: with no recording and with an
//   id no recording has nothing is archived; the copy into a directory that does not exist holds
//   the recording's files, the manifest among them; named again by its id the archived recording
//   gets no second manifest and is copied into an empty directory; into a directory holding a file
//   `record archive` is refused before anything is written and the recording stays active.
// - `rec_arch_a_copy_that_does_not_verify_names_the_file_and_the_retry_by_id_copies`: a stored frame
//   that no longer holds its recorded sha256 (the module's tests' seam: they edit the files of the
//   temporary root) makes the copy's check fail naming that file; what was copied stays, nothing
//   is deleted and the recording is archived; with the frame put back, `record archive
//   --record-id` copies it into another directory and writes no second manifest.
// - `rec_arch_reopen_builds_the_package_the_recording_would_have_built`: `record reopen` is refused
//   on the active recording and while another one is active; after it the archived recording is
//   active again, marked and stopped it builds the package a twin never archived builds (the files
//   compared but the provenance's generation time), and its manifest stays.
// Fix 1 (coordinator work order rd-1113/brief-1675-fix1.md, after the independent review):
// - `rec_arch_a_copy_target_that_is_no_absolute_path_is_refused`: on the command line and through
//   `serve --stdio`, `--copy-to` empty, of white space only and relative are each refused
//   `validation_failed` (`details.reason` `empty` or `not_absolute`, `details.path` the value)
//   before anything is written: the session directory holds what it held (but the recording lock's
//   own files), nothing is at the relative target or in the current directory, and the recording is
//   still active.
// - `rec_arch_an_archived_recording_refuses_stop_and_reopens_by_its_header`: `record stop` on an
//   archived recording answers `record_session_not_active` with `details.reason`
//   `recording_archived` and its id and writes nothing; `record reopen` without a record id reopens
//   the recording the session header names, the header kept but for its status; an archive after
//   the reopen writes `archive-manifest-2.json` and leaves the first manifest as it was.
// - `rec_arch_a_recording_record_start_force_left_behind_is_archived_by_its_id`: a recording `record
//   start --force` left behind is archived by its id from its own step table and copied, the
//   session header (the other recording's) left as it is and named in `lab.session_header`; named
//   again it gets no second manifest; `record reopen --record-id` is refused while the other
//   recording is active and, once that one is archived, makes it the active recording; its step
//   table keeps its bytes.
// Temporary roots only; no Runtime, no device, no sleep.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use crate::cli_module::{self, serve};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::{
    AttachFrameRequest, ClickEffect, CommitClickRequest, OpaqueJson, PlanClickRequest, RecordRect,
    RecordingLock, record_attach_frame, record_commit_click, record_plan_click,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const INSTANCE: &str = "neutral.a";
const TASK: &str = "neutral_task";
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const BLUE: [u8; 3] = [20, 20, 230];
const GRAY: [u8; 3] = [60, 60, 60];
const TAP: RecordRect = RecordRect {
    x: 8,
    y: 2,
    width: 4,
    height: 4,
};
// Fix 1: a relative copy target, which would name a place under the current directory.
const RELATIVE: &str = "rec-arch-relative-copy";

// A 16x9 frame: a 4x4 square of `color` at the top left on gray.
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

// What `actinglab --json --instance neutral.a record <action> --state-dir <state> <more>` prints:
// its exit code and its line.
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

// `record start` of the recording `record_id`; its instance as the session header names it.
fn start(state: &str, record_id: &str) -> String {
    let (code, started) = record(
        state,
        "start",
        &["--task-id", TASK, "--record-id", record_id],
    );
    assert_eq!(code, 0, "{started}");
    assert_eq!(started["lab_recording"]["status"], "active", "{started}");
    started["record"]["instance"]
        .as_str()
        .expect("the instance")
        .to_owned()
}

fn opaque(value: Value) -> Option<OpaqueJson> {
    Some(OpaqueJson::from_serializable(&value).expect("opaque value"))
}

// A frame showing `color` as `observe --capture --record` attaches it, as `initiator`; with
// `request`, then a tap of `TAP` at its centre as `do --capture --record --tap-rect` records a
// Performed one.
fn step(state: &str, instance: &str, initiator: &str, color: [u8; 3], request: Option<&str>) {
    let lock = RecordingLock::acquire(Path::new(state), instance, "gate_449", initiator)
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
            lease_id: opaque(json!("lease-0449")),
            failure: None,
            before: opaque(
                json!({"frame_id": format!("{request}-before"), "page": "neutral_page"}),
            ),
            after: opaque(json!({"frame_id": format!("{request}-after"), "page": null})),
        },
    )
    .unwrap_or_else(|error| panic!("tap: {}: {}", error.code, error.message));
}

// The recording's own directory.
fn lab_dir(state: &str, record_id: &str) -> PathBuf {
    Path::new(state)
        .join("record-artifacts")
        .join(record_id)
        .join("lab")
}

// Every file under `dir`, by its `/`-separated path relative to it, with its bytes, in name order.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, prefix)) = pending.pop() {
        for entry in fs::read_dir(&path).expect("directory") {
            let entry = entry.expect("entry");
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().expect("file type").is_dir() {
                pending.push((entry.path(), format!("{name}/")));
            } else {
                files.push((name, fs::read(entry.path()).expect("file")));
            }
        }
    }
    files.sort();
    files
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("file")).expect("JSON")
}

// The manifest files of the recording's directory.
fn manifests(dir: &Path) -> Vec<String> {
    let mut names = fs::read_dir(dir)
        .expect("directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with("archive-manifest-"))
        .collect::<Vec<_>>();
    names.sort();
    names
}

// Fix 1: every file of the session directory `state`, by its `/`-separated path relative to it,
// with its bytes, in name order, but the recording lock's own files (`record-<instance>.lock` and
// its holder file), which every state-writing record command takes and rewrites, a refused one
// too.
fn session_files(state: &str) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(state).expect("session directory") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().expect("file type").is_dir() {
            files.extend(
                files_under(&entry.path())
                    .into_iter()
                    .map(|(file, bytes)| (format!("{name}/{file}"), bytes)),
            );
        } else if !name.contains(".lock") {
            files.push((name, fs::read(entry.path()).expect("file")));
        }
    }
    files.sort();
    files
}

// Fix 1: what `serve --stdio` answers to `record archive` with `copy_to` on the instance and the
// session directory `state`: its one line, as JSON.
fn served_archive(state: &str, copy_to: &str) -> Value {
    let table = cli_module::table().expect("the operation table");
    let request = json!({
        "op": "record archive",
        "params": {"instance": INSTANCE, "state_dir": state, "copy_to": copy_to}
    });
    let input = format!("{request}\n");
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve(table, input.as_bytes(), &mut output, &mut developer).expect("the loop ends");
    let output = String::from_utf8(output).expect("UTF-8 output");
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{output}");
    serde_json::from_str(lines[0]).expect("one JSON answer")
}

// Items 1 and 2 and the first test of the work order: a recording kept only as material, whose
// steps have no marks and whose last step has a tap.
#[test]
fn rec_arch_record_archive_ends_a_recording_that_record_stop_refuses() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let instance = start(&state, "rec_one");
    step(&state, &instance, "Lab-CLI", RED, Some("request-cli"));
    step(&state, &instance, "Lab-MCP", GREEN, Some("request-mcp"));
    let dir = lab_dir(&state, "rec_one");
    let before = files_under(&dir);

    let (code, stopped) = record(&state, "stop", &[]);
    assert_ne!(code, 0, "{stopped}");
    assert_eq!(
        stopped["outcome"]["code"], "record_final_step_has_click",
        "{stopped}"
    );
    assert_eq!(files_under(&dir), before, "record stop wrote nothing");

    let (code, archived) = record(&state, "archive", &[]);
    assert_eq!(code, 0, "{archived}");
    assert_eq!(archived["status"], "archived", "{archived}");
    assert_eq!(archived["record"]["record_id"], "rec_one");
    assert_eq!(archived["record"]["status"], "archived");
    assert_eq!(
        archived["lab"]["manifest"]["file"],
        "archive-manifest-1.json"
    );
    assert_eq!(archived["lab"]["manifest"]["steps"], 2);
    assert_eq!(archived["lab"]["manifest"]["written"], true);

    // The manifest's rows are the step table's steps.
    let table = read_json(&dir.join("recording.json"));
    let manifest = read_json(&dir.join("archive-manifest-1.json"));
    assert_eq!(manifest["record_id"], "rec_one");
    assert_eq!(manifest["instance"], instance.as_str());
    assert_eq!(manifest["step_count"], 2);
    let steps = table["steps"].as_array().expect("steps");
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), steps.len());
    for (row, step) in rows.iter().zip(steps) {
        let execution = &step["click"]["execution"];
        assert_eq!(row["step"], step["index"], "{row}");
        assert_eq!(row["kind"], "tap", "{row}");
        assert_eq!(
            row["frames"][0]["sha256"], step["frames"][0]["sha256"],
            "{row}"
        );
        assert_eq!(row["tap"]["rect"], step["click"]["rect"], "{row}");
        assert_eq!(row["tap"]["point"], execution["point"], "{row}");
        assert_eq!(row["request_id"], execution["req_id"], "{row}");
        assert_eq!(row["at_unix_ms"], execution["executed_at_unix_ms"], "{row}");
    }
    assert_eq!(rows[0]["request_id"], "request-cli");
    assert_eq!(rows[1]["request_id"], "request-mcp");

    let (code, status) = record(&state, "status", &[]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["record"]["status"], "archived", "{status}");
    assert_eq!(status["lab"]["status"], "archived", "{status}");

    let (code, started) = record(
        &state,
        "start",
        &["--task-id", TASK, "--record-id", "rec_two"],
    );
    assert_eq!(code, 0, "{started}");
    assert_eq!(started["record"]["record_id"], "rec_two");
    assert_eq!(started["record"]["status"], "active");

    // Every frame and the step table keep their bytes; the manifest is beside them.
    let mut expected = before;
    expected.push((
        "archive-manifest-1.json".to_owned(),
        fs::read(dir.join("archive-manifest-1.json")).expect("manifest"),
    ));
    expected.sort();
    assert_eq!(files_under(&dir), expected);
}

// Items 3 and 9: the copy's target, and what is refused before anything is written.
#[test]
fn rec_arch_the_copy_goes_only_into_a_new_or_an_empty_directory() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let (_, none) = record(&state, "archive", &[]);
    assert_eq!(
        none["outcome"]["code"], "record_session_not_active",
        "{none}"
    );

    let instance = start(&state, "rec_p");
    step(&state, &instance, "Lab-CLI", RED, Some("request-p"));
    step(&state, &instance, "Lab-CLI", BLUE, None);
    let (_, unknown) = record(&state, "archive", &["--record-id", "rec_none"]);
    assert_eq!(
        unknown["outcome"]["code"], "record_not_archived",
        "{unknown}"
    );
    assert_eq!(unknown["details"]["state"], "missing", "{unknown}");
    let dir = lab_dir(&state, "rec_p");
    assert!(manifests(&dir).is_empty(), "nothing was written");

    // Into a directory that does not exist.
    let created = isolated.path("copies/created");
    let (code, archived) = record(&state, "archive", &["--copy-to", created.as_str()]);
    assert_eq!(code, 0, "{archived}");
    assert_eq!(archived["status"], "archived");
    assert_eq!(files_under(Path::new(&created)), files_under(&dir));
    let copied = archived["lab"]["copy"]["files"].as_array().expect("files");
    assert_eq!(copied.len(), 4, "{archived}");

    // Named again by its id, into an empty directory: no second manifest.
    let empty = isolated.path("copies/empty");
    fs::create_dir_all(&empty).expect("empty directory");
    let (code, again) = record(
        &state,
        "archive",
        &["--record-id", "rec_p", "--copy-to", empty.as_str()],
    );
    assert_eq!(code, 0, "{again}");
    assert_eq!(again["status"], "already_archived", "{again}");
    assert_eq!(again["lab"]["manifest"]["written"], false);
    assert_eq!(manifests(&dir), ["archive-manifest-1.json"]);
    assert_eq!(files_under(Path::new(&empty)), files_under(&dir));

    // Into a directory that holds a file: refused before anything is written.
    let instance = start(&state, "rec_q");
    step(&state, &instance, "Lab-CLI", RED, Some("request-q"));
    let held = isolated.path("copies/held");
    fs::create_dir_all(&held).expect("held directory");
    fs::write(Path::new(&held).join("kept.txt"), b"kept").expect("kept file");
    let q = lab_dir(&state, "rec_q");
    let before = files_under(&q);
    let (code, refused) = record(&state, "archive", &["--copy-to", held.as_str()]);
    assert_ne!(code, 0, "{refused}");
    assert_eq!(
        refused["outcome"]["code"], "record_archive_target_not_empty",
        "{refused}"
    );
    assert_eq!(refused["details"]["reason"], "not_empty", "{refused}");
    assert_eq!(files_under(&q), before, "nothing was written");
    assert_eq!(
        files_under(Path::new(&held)),
        [("kept.txt".to_owned(), b"kept".to_vec())]
    );
    let (_, status) = record(&state, "status", &[]);
    assert_eq!(status["record"]["record_id"], "rec_q", "{status}");
    assert_eq!(status["record"]["status"], "active", "{status}");
}

// Items 3 and 4: a copy whose check fails, and the retry by id.
#[test]
fn rec_arch_a_copy_that_does_not_verify_names_the_file_and_the_retry_by_id_copies() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let instance = start(&state, "rec_c");
    step(&state, &instance, "Lab-CLI", RED, Some("request-c"));
    step(&state, &instance, "Lab-CLI", BLUE, None);
    let dir = lab_dir(&state, "rec_c");
    let table = read_json(&dir.join("recording.json"));
    let sha256 = table["steps"][0]["frames"][0]["sha256"]
        .as_str()
        .expect("sha256")
        .to_owned();
    let file = format!("frames/{sha256}.png");
    let frame = dir.join("frames").join(format!("{sha256}.png"));
    let original = fs::read(&frame).expect("frame");
    // The seam: the stored frame no longer holds the sha256 the step table records.
    fs::write(&frame, b"not the recorded frame").expect("edited frame");

    let first = isolated.path("first-copy");
    let (code, failed) = record(&state, "archive", &["--copy-to", first.as_str()]);
    assert_ne!(code, 0, "{failed}");
    assert_eq!(
        failed["outcome"]["code"], "record_archive_verify_failed",
        "{failed}"
    );
    let details = &failed["details"];
    assert_eq!(details["file"], file.as_str(), "{failed}");
    assert_eq!(details["reason"], "sha256_mismatch", "{failed}");
    assert_eq!(details["checked"], "copy", "{failed}");
    assert_eq!(details["archived"], true, "{failed}");
    assert_eq!(details["record_id"], "rec_c", "{failed}");
    // What was copied stays; nothing of the recording was deleted; it is archived.
    assert_eq!(
        fs::read(
            Path::new(&first)
                .join("frames")
                .join(format!("{sha256}.png"))
        )
        .expect("copy"),
        b"not the recorded frame"
    );
    assert!(Path::new(&first).join("recording.json").is_file());
    assert_eq!(manifests(&dir), ["archive-manifest-1.json"]);
    assert_eq!(
        fs::read(&frame).expect("frame"),
        b"not the recorded frame",
        "the archive rewrites no frame"
    );
    let (_, status) = record(&state, "status", &[]);
    assert_eq!(status["record"]["status"], "archived", "{status}");

    // The frame put back, the retry by id copies it and writes no second manifest.
    fs::write(&frame, &original).expect("frame put back");
    let second = isolated.path("second-copy");
    let (code, retried) = record(
        &state,
        "archive",
        &["--record-id", "rec_c", "--copy-to", second.as_str()],
    );
    assert_eq!(code, 0, "{retried}");
    assert_eq!(retried["status"], "already_archived", "{retried}");
    assert_eq!(
        retried["lab"]["manifest"]["file"],
        "archive-manifest-1.json"
    );
    assert_eq!(retried["lab"]["manifest"]["written"], false);
    assert_eq!(manifests(&dir), ["archive-manifest-1.json"]);
    assert_eq!(files_under(Path::new(&second)), files_under(&dir));
}

// The package file `record stop` answered, as `{path: text}` with the task's provenance without
// its generation time (the time the step table was last written, which differs between two
// recordings built one after the other).
fn package_files(stopped: &Value) -> Value {
    let package = read_json(Path::new(stopped["lab"]["path"].as_str().expect("path")));
    let mut files = package["files"].clone();
    for (path, text) in files.as_object_mut().expect("files") {
        if path.ends_with("/task.json") {
            let mut task: Value = serde_json::from_str(text.as_str().expect("text")).expect("task");
            let provenance = task["provenance"].as_object_mut().expect("provenance");
            assert!(provenance.remove("generated_at_unix_ms").is_some());
            *text = task;
        }
    }
    files
}

// Item 5: `record reopen`, and the package an archived recording builds once reopened.
#[test]
fn rec_arch_reopen_builds_the_package_the_recording_would_have_built() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let archived = isolated.path("archived");
    let twin = isolated.path("twin");
    for state in [&archived, &twin] {
        let instance = start(state, "rec_x");
        step(state, &instance, "Lab-CLI", RED, Some("request-1"));
        step(state, &instance, "Lab-MCP", GREEN, Some("request-2"));
        step(state, &instance, "Lab-CLI", BLUE, None);
    }

    // Refused on a recording that is not archived: the active one.
    let (_, active) = record(&archived, "reopen", &[]);
    assert_eq!(active["outcome"]["code"], "record_not_archived", "{active}");
    assert_eq!(active["details"]["state"], "active", "{active}");

    let (code, ended) = record(&archived, "archive", &[]);
    assert_eq!(code, 0, "{ended}");
    // Refused while another recording is active on the instance.
    start(&archived, "rec_y");
    let (_, busy) = record(&archived, "reopen", &["--record-id", "rec_x"]);
    assert_eq!(busy["outcome"]["code"], "record_session_active", "{busy}");
    let (code, other) = record(&archived, "archive", &[]);
    assert_eq!(code, 0, "{other}");
    assert_eq!(other["lab"]["manifest"]["steps"], 0, "{other}");

    let (code, reopened) = record(&archived, "reopen", &["--record-id", "rec_x"]);
    assert_eq!(code, 0, "{reopened}");
    assert_eq!(reopened["status"], "reopened", "{reopened}");
    assert_eq!(reopened["record"]["record_id"], "rec_x");
    assert_eq!(reopened["record"]["status"], "active");
    assert_eq!(reopened["lab"]["replaced_record_id"], "rec_y");
    assert_eq!(reopened["lab"]["manifest"], "archive-manifest-1.json");
    let (_, status) = record(&archived, "status", &[]);
    assert_eq!(status["record"]["record_id"], "rec_x", "{status}");
    assert_eq!(status["lab"]["status"], "active", "{status}");

    // Marked and stopped, it builds what its twin, never archived, builds.
    let mut built = Vec::new();
    for state in [&archived, &twin] {
        for (number, id) in [("1", "red"), ("2", "green"), ("3", "blue")] {
            let color = format!("{id}_color=0,0,4,4");
            let (code, marked) = record(
                state,
                "mark",
                &["--step", number, "--color", color.as_str()],
            );
            assert_eq!(code, 0, "{marked}");
        }
        let (code, stopped) = record(
            state,
            "stop",
            &["--game", "neutral", "--server", "test", "--locale", "en-US"],
        );
        assert_eq!(code, 0, "{stopped}");
        assert_eq!(stopped["lab"]["status"], "generated", "{stopped}");
        built.push(package_files(&stopped));
    }
    assert_eq!(built[0], built[1]);
    // The manifest written when it was archived stays.
    assert_eq!(
        manifests(&lab_dir(&archived, "rec_x")),
        ["archive-manifest-1.json"]
    );
}

// Fix 1, point 1: a copy target that is no absolute path, on the command line and through `serve
// --stdio` (the Lab's MCP server refuses such a path parameter itself before it sends anything,
// `arguments_invalid` `not_absolute`).
#[test]
fn rec_arch_a_copy_target_that_is_no_absolute_path_is_refused() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let instance = start(&state, "rec_t");
    step(&state, &instance, "Lab-CLI", RED, Some("request-t"));
    step(&state, &instance, "Lab-CLI", BLUE, None);
    let before = session_files(&state);
    let (_, active) = record(&state, "status", &[]);
    assert_eq!(active["record"]["status"], "active", "{active}");
    for (target, reason) in [("", "empty"), ("   ", "empty"), (RELATIVE, "not_absolute")] {
        let (code, refused) = record(&state, "archive", &["--copy-to", target]);
        assert_ne!(code, 0, "{target:?}: {refused}");
        let served = served_archive(&state, target);
        for answer in [&refused, &served] {
            assert_eq!(
                answer["outcome"]["code"], "validation_failed",
                "{target:?}: {answer}"
            );
            assert_eq!(answer["details"]["reason"], reason, "{target:?}: {answer}");
            assert_eq!(answer["details"]["path"], target, "{target:?}: {answer}");
        }
        assert_eq!(
            session_files(&state),
            before,
            "{target:?}: nothing was written"
        );
        let (_, status) = record(&state, "status", &[]);
        assert_eq!(status, active, "{target:?}: the recording is still active");
    }
    for name in [RELATIVE, "recording.json", "archive-manifest-1.json"] {
        assert!(
            !Path::new(name).exists(),
            "{name} was written in the current directory"
        );
    }
}

// Fix 1, point 2: `record stop` on an archived recording, `record reopen` without a record id, and
// an archive after a reopen.
#[test]
fn rec_arch_an_archived_recording_refuses_stop_and_reopens_by_its_header() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let instance = start(&state, "rec_s");
    step(&state, &instance, "Lab-CLI", RED, Some("request-s1"));
    step(&state, &instance, "Lab-MCP", GREEN, Some("request-s2"));
    let dir = lab_dir(&state, "rec_s");
    let (code, archived) = record(&state, "archive", &[]);
    assert_eq!(code, 0, "{archived}");
    let header = archived["record"].clone();
    assert_eq!(header["status"], "archived", "{archived}");
    let first = fs::read(dir.join("archive-manifest-1.json")).expect("first manifest");

    // `record stop`: the refusal the contract names; nothing written.
    let before = session_files(&state);
    let (code, stopped) = record(&state, "stop", &[]);
    assert_ne!(code, 0, "{stopped}");
    assert_eq!(
        stopped["outcome"]["code"], "record_session_not_active",
        "{stopped}"
    );
    assert_eq!(
        stopped["details"]["reason"], "recording_archived",
        "{stopped}"
    );
    assert_eq!(stopped["details"]["record_id"], "rec_s", "{stopped}");
    assert_eq!(session_files(&state), before, "record stop wrote nothing");

    // `record reopen` without a record id: the recording the session header names, its header kept
    // but for its status.
    let (code, reopened) = record(&state, "reopen", &[]);
    assert_eq!(code, 0, "{reopened}");
    assert_eq!(reopened["status"], "reopened", "{reopened}");
    assert_eq!(reopened["lab"]["record_id"], "rec_s", "{reopened}");
    assert_eq!(reopened["lab"]["replaced_record_id"], Value::Null);
    assert_eq!(reopened["lab"]["manifest"], "archive-manifest-1.json");
    let mut kept = header;
    kept["status"] = json!("active");
    kept["updated_at_unix_ms"] = reopened["record"]["updated_at_unix_ms"].clone();
    assert_eq!(reopened["record"], kept, "{reopened}");
    let (_, status) = record(&state, "status", &[]);
    assert_eq!(status["record"]["record_id"], "rec_s", "{status}");
    assert_eq!(status["record"]["status"], "active", "{status}");
    assert_eq!(status["lab"]["status"], "active", "{status}");

    // A step more, then an archive: the second manifest; the first as it was.
    step(&state, &instance, "Lab-CLI", BLUE, None);
    let (code, again) = record(&state, "archive", &[]);
    assert_eq!(code, 0, "{again}");
    assert_eq!(again["status"], "archived", "{again}");
    assert_eq!(
        again["lab"]["manifest"]["file"], "archive-manifest-2.json",
        "{again}"
    );
    assert_eq!(again["lab"]["manifest"]["written"], true, "{again}");
    assert_eq!(again["lab"]["manifest"]["steps"], 3, "{again}");
    assert_eq!(
        manifests(&dir),
        ["archive-manifest-1.json", "archive-manifest-2.json"]
    );
    assert_eq!(
        fs::read(dir.join("archive-manifest-1.json")).expect("first manifest"),
        first
    );
}

// Fix 1, points 2 and 4: a recording `record start --force` left behind, its directory and step
// table intact and the session header another recording's.
#[test]
fn rec_arch_a_recording_record_start_force_left_behind_is_archived_by_its_id() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let instance = start(&state, "rec_f");
    step(&state, &instance, "Lab-CLI", RED, Some("request-f1"));
    step(&state, &instance, "Lab-MCP", GREEN, Some("request-f2"));
    let dir = lab_dir(&state, "rec_f");
    let table_bytes = fs::read(dir.join("recording.json")).expect("step table");
    let table: Value = serde_json::from_slice(&table_bytes).expect("step table JSON");
    let before = files_under(&dir);
    let (code, forced) = record(
        &state,
        "start",
        &["--task-id", TASK, "--record-id", "rec_g", "--force"],
    );
    assert_eq!(code, 0, "{forced}");
    let (_, other) = record(&state, "status", &[]);
    assert_eq!(other["record"]["record_id"], "rec_g", "{other}");
    assert_eq!(other["record"]["status"], "active", "{other}");

    // Archived by its id from its own step table, and copied; the session header left as it is.
    let copy = isolated.path("copies/left-behind");
    let (code, archived) = record(
        &state,
        "archive",
        &["--record-id", "rec_f", "--copy-to", copy.as_str()],
    );
    assert_eq!(code, 0, "{archived}");
    assert_eq!(archived["status"], "archived", "{archived}");
    assert_eq!(archived["record"], Value::Null, "{archived}");
    let lab = &archived["lab"];
    assert_eq!(lab["record_id"], "rec_f", "{archived}");
    assert_eq!(
        lab["record_started_at_unix_ms"], table["record_started_at_unix_ms"],
        "{archived}"
    );
    assert_eq!(lab["manifest"]["file"], "archive-manifest-1.json");
    assert_eq!(lab["manifest"]["written"], true);
    assert_eq!(lab["manifest"]["steps"], 2);
    assert_eq!(
        lab["session_header"],
        json!({"record_id": "rec_g", "status": "active", "changed": false}),
        "{archived}"
    );
    let manifest = read_json(&dir.join("archive-manifest-1.json"));
    assert_eq!(manifest["record_id"], "rec_f");
    assert_eq!(manifest["instance"], instance.as_str());
    assert_eq!(manifest["task_id"], TASK);
    assert_eq!(
        manifest["started_at_unix_ms"],
        table["record_started_at_unix_ms"]
    );
    let rows = manifest["steps"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| (row["kind"].clone(), row["request_id"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            (json!("tap"), json!("request-f1")),
            (json!("tap"), json!("request-f2"))
        ]
    );
    let (_, status) = record(&state, "status", &[]);
    assert_eq!(
        status, other,
        "the session header and its recording as they were"
    );
    let mut expected = before;
    expected.push((
        "archive-manifest-1.json".to_owned(),
        fs::read(dir.join("archive-manifest-1.json")).expect("manifest"),
    ));
    expected.sort();
    assert_eq!(files_under(&dir), expected);
    assert_eq!(files_under(Path::new(&copy)), expected);

    // Named again: no second manifest; the session header still the other recording's.
    let (code, again) = record(&state, "archive", &["--record-id", "rec_f"]);
    assert_eq!(code, 0, "{again}");
    assert_eq!(again["status"], "already_archived", "{again}");
    assert_eq!(again["lab"]["manifest"]["written"], false, "{again}");
    assert_eq!(again["lab"]["session_header"]["record_id"], "rec_g");
    assert_eq!(manifests(&dir), ["archive-manifest-1.json"]);

    // `record reopen --record-id`: refused while the other recording is active; once that one is
    // archived, a header of this recording in place of the other's.
    let (_, busy) = record(&state, "reopen", &["--record-id", "rec_f"]);
    assert_eq!(busy["outcome"]["code"], "record_session_active", "{busy}");
    assert_eq!(busy["details"]["active_record_id"], "rec_g", "{busy}");
    let (code, ended) = record(&state, "archive", &[]);
    assert_eq!(code, 0, "{ended}");
    assert_eq!(ended["lab"]["record_id"], "rec_g", "{ended}");
    let (code, reopened) = record(&state, "reopen", &["--record-id", "rec_f"]);
    assert_eq!(code, 0, "{reopened}");
    assert_eq!(reopened["record"]["record_id"], "rec_f", "{reopened}");
    assert_eq!(reopened["record"]["status"], "active", "{reopened}");
    assert_eq!(
        reopened["record"]["started_at_unix_ms"], table["record_started_at_unix_ms"],
        "{reopened}"
    );
    assert_eq!(reopened["lab"]["replaced_record_id"], "rec_g", "{reopened}");
    let (_, status) = record(&state, "status", &[]);
    assert_eq!(status["record"]["record_id"], "rec_f", "{status}");
    assert_eq!(status["lab"]["status"], "active", "{status}");
    assert_eq!(
        status["lab"]["steps"].as_array().map(Vec::len),
        Some(2),
        "{status}"
    );
    assert_eq!(
        fs::read(dir.join("recording.json")).expect("step table"),
        table_bytes,
        "the step table keeps its bytes"
    );
}
