// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 SEL-L6a (FCM row SEL-L6a; compromises T-01 and T-03 §1): `actinglab replay
// --frames` replays a whole package over a directory of saved frames without asking anything,
// and its exit code tells a CI job whether the replayed run completed; `--facts` takes the
// instance answer of a Runtime; without an OCR model a package with an OCR target is reported
// as `lab_ocr_unavailable` (Workflow #396 LAB-3); offline `observe` lists an operation whose
// `to` names several pages. This actinglab runs outside an installation, so it has no OCR
// model.

use actingcommand_contract::{
    FactContent, FactRecord, FactScope, FactValue, InstanceFactContext, InstanceFactSnapshot,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const HOME: [u8; 3] = [255, 0, 0];
const TERMINAL: [u8; 3] = [0, 0, 255];
const ALTERNATE: [u8; 3] = [0, 255, 255];
const READY: [u8; 3] = [0, 255, 0];
const NOT_READY: [u8; 3] = [0, 0, 0];

// T-01, T-03: the replay runs the package's whole route, here one operation from `home` whose
// `to` names two pages, over the frames of a directory in file-name order; it asks nothing (its
// standard input is closed) and executes nothing. A run that completes exits 0 with the entry
// check, the step with its destination set and the page it reached, and the end; given
// `--facts`, the replay names the snapshot it evaluated a select step with and the instant it
// was read. A run that needs a frame after the last one exits non-zero with replay_run_refused,
// the step it had started and the run's own end.
#[test]
fn replay_frames_runs_a_package_without_asking_and_exits_by_how_the_run_ended() {
    let temp = TempDir::new().unwrap();
    let package = write_package(&temp, None);
    let facts = temp.path().join("facts.json");
    fs::write(&facts, serde_json::to_vec(&facts_answer()).unwrap()).unwrap();
    let completed = frames(&temp, "completed", &[(HOME, READY), (ALTERNATE, NOT_READY)]);

    let output = replay(&temp, &package, &completed, Some(&facts));
    assert!(output.status.success(), "{}", describe(&output));
    let data = &envelope(&output)["data"];
    assert_eq!(data["mode"], "offline_replay");
    assert_eq!(data["executed"], false);
    assert_eq!(data["production_global_ledger_written"], false);
    assert_eq!(data["package_id"], "neutral.replay.task");
    assert_eq!(
        data["entry"],
        json!({"required_pages": ["neutral/home"], "matched_page": "neutral/home"})
    );
    let steps = data["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["operation_label"], "branch");
    assert_eq!(steps[0]["destinations"], json!(["alternate", "terminal"]));
    assert_eq!(steps[0]["effects"].as_array().unwrap().len(), 1);
    assert_eq!(steps[0]["reached_page"], "neutral/alternate");
    assert_eq!(data["end"]["status"], "completed");
    assert_eq!(data["end"]["final_page"], "neutral/alternate");
    assert_eq!(data["frames_read"], 2);
    assert_eq!(data["frames_given"], 2);
    assert_eq!(data["facts"]["source"], "snapshot");
    assert_eq!(data["facts"]["snapshot_id"], "snapshot-a");
    assert_eq!(data["facts"]["read_at_unix_ms"], 1_000_500);

    let short = frames(&temp, "short", &[(HOME, READY)]);
    let output = replay(&temp, &package, &short, None);
    assert!(!output.status.success(), "{}", describe(&output));
    let error = &envelope(&output)["error"];
    assert_eq!(error["code"], "replay_run_refused");
    let details = &error["details"];
    assert_eq!(details["executed"], false);
    assert_eq!(details["end"]["status"], "refused");
    assert_eq!(
        details["end"]["outcome"]["code"],
        "offline_fixture_exhausted"
    );
    assert_eq!(details["steps"][0]["operation_label"], "branch");
    assert_eq!(details["steps"][0]["effects"].as_array().unwrap().len(), 1);
    assert_eq!(details["steps"][0]["reached_page"], Value::Null);
    assert_eq!(details["frames_read"], 1);
    assert_eq!(details["facts"]["source"], "empty");
}

// T-01 with Workflow #396 LAB-3: without an OCR model, a package whose recognition pack has an
// OCR target is refused with lab_ocr_unavailable, naming the target and its cause, before any
// frame is evaluated; the replay never skips the target.
#[test]
fn replay_frames_without_an_ocr_model_reports_ocr_unavailable_before_evaluating() {
    let temp = TempDir::new().unwrap();
    let ocr_target = json!({
        "type": "ocr",
        "id": "fixture/ocr",
        "region": {"x": 0, "y": 0, "width": 1, "height": 1},
        "languages": ["en"],
        "timeout_ms": 1000,
        "match_mode": "exact",
        "expected": ["synthetic truth"],
        "case_sensitive": false,
        "minimum_confidence": 0.0,
        "model_ref": "PP-OCRv6_medium",
        "model_sha256": "a".repeat(64)
    });
    let package = write_package(&temp, Some(ocr_target));
    let sequence = frames(&temp, "ocr", &[(HOME, READY), (ALTERNATE, NOT_READY)]);

    let output = replay(&temp, &package, &sequence, None);
    assert!(!output.status.success(), "{}", describe(&output));
    let error = &envelope(&output)["error"];
    assert_eq!(error["code"], "lab_ocr_unavailable");
    let outcome = &error["details"]["outcome"];
    assert_eq!(outcome["code"], "lab_ocr_unavailable");
    assert_eq!(outcome["values"]["target_id"], "fixture/ocr");
    assert_eq!(outcome["causes"][0]["code"], "lab_ocr_vision_unconfigured");
}

// T-03: offline observe lists, with the page's actions, an operation whose `to` names several
// pages, with every page it may reach and its declared click.
#[test]
fn observe_lists_an_operation_with_several_destinations() {
    let temp = TempDir::new().unwrap();
    let package = write_package(&temp, None);
    let scene = temp.path().join("home.png");
    fs::write(&scene, frame_png(HOME, READY)).unwrap();

    // Workflow #336 PKG-S1: the reference observe still requires is recorded and compared with
    // nothing.
    let output = run_actinglab(
        &temp,
        [
            OsStr::new("--json"),
            OsStr::new("observe"),
            OsStr::new("--zip"),
            package.as_os_str(),
            OsStr::new("--expected-sha256"),
            OsStr::new(&"0".repeat(64)),
            OsStr::new("--scene"),
            scene.as_os_str(),
            OsStr::new("--verbose"),
        ],
    );
    assert!(output.status.success(), "{}", describe(&output));
    let data = &envelope(&output)["data"];
    assert_eq!(data["page"], "neutral/home");
    let actions = data["actions"].as_array().unwrap();
    let branch = actions
        .iter()
        .find(|action| action["id"] == "branch")
        .unwrap_or_else(|| panic!("observe did not list the operation: {actions:?}"));
    assert_eq!(branch["task_id"], "task");
    assert_eq!(branch["from_page"], "neutral/home");
    assert_eq!(
        branch["to_pages"],
        json!(["neutral/terminal", "neutral/alternate"])
    );
    assert_eq!(branch["click"], json!({"kind": "point", "x": 1, "y": 0}));
}

// A package that checks its entry on `home` and has one guarded click whose `to` names
// `terminal` and `alternate`; `extra_target` joins its recognition pack.
fn write_package(temp: &TempDir, extra_target: Option<Value>) -> PathBuf {
    let color = |id: &str, region_x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": region_x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let mut targets = vec![
        color("page/home", 0, HOME),
        color("page/terminal", 0, TERMINAL),
        color("page/alternate", 0, ALTERNATE),
        color("guard/ready", 1, READY),
    ];
    let schema = if extra_target.is_some() { "0.6" } else { "0.3" };
    targets.extend(extra_target);
    let page = |name: &str| {
        json!({"id": format!("neutral/{name}"), "required": [format!("page/{name}")],
               "optional": [], "forbidden": []})
    };
    // A single `home` entry is checked only when its page is gated by an `any_of` group, so the
    // home page is gated that way and the run has an entry check to report.
    let mut home = page("home");
    home["required"] = json!([]);
    home["any_of"] = json!([["page/home"]]);
    let bytes = zip_entries(&[
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": "neutral.replay.task",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 2, "height": 1},
                "entry_task_id": "task",
                "capture_interval_ms": 1,
                "step_timeout_ms": 10_000,
                "timeout_ms": 60_000,
                "max_steps": 2
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "goal": "neutral fixture",
                "coordinate_space": {"width": 2, "height": 1},
                "entry_page": "home",
                "target_page": ["terminal", "alternate"],
                "operations": [{
                    "id": "branch",
                    "purpose": "reach either page",
                    "from": "home",
                    "to": ["terminal", "alternate"],
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "guard": {
                        "page_id": "home",
                        "target_id": "guard/ready",
                        "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                        "color_probe": "guard/ready"
                    }
                }]
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": schema,
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": targets
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3",
                   "pages": [home, page("terminal"), page("alternate")]}),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({"schema_version": "0.3", "game": "neutral", "navigation": [],
                   "destructive_actions": []}),
        ),
    ]);
    let path = temp.path().join("replay.package.zip");
    fs::write(&path, bytes).unwrap();
    path
}

// The instance answer of `actingctl facts --instance` / MCP `ac_facts`, read without a key
// prefix: one fact of the instance, read 500 ms after it was published.
fn facts_answer() -> Value {
    let record = FactRecord {
        scope: FactScope::Instance {
            instance_id: "instance-a".to_owned(),
        },
        key: "session.example.enabled".to_owned(),
        content: FactContent::Inline {
            value: FactValue::Boolean(true),
        },
        observed_at_unix_ms: 1_000_000,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "decision-state".to_owned(),
        source_snapshot_id: "decision-state:enabled".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "0".repeat(64),
        invalidate_on: Vec::new(),
    };
    let snapshot = InstanceFactSnapshot {
        snapshot_id: "snapshot-a".to_owned(),
        ledger_position: 7,
        context: InstanceFactContext {
            instance_id: "instance-a".to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        },
        records: vec![record],
    };
    json!({
        "scope": "instance",
        "read_at_unix_ms": 1_000_500,
        "snapshot": snapshot,
        "records_total": 1,
        "records_kept": 1
    })
}

// A directory of the frames `pixels` (left pixel, right pixel), named in their order.
fn frames(temp: &TempDir, name: &str, pixels: &[([u8; 3], [u8; 3])]) -> PathBuf {
    let directory = temp.path().join(name);
    fs::create_dir(&directory).unwrap();
    for (index, (left, right)) in pixels.iter().enumerate() {
        fs::write(
            directory.join(format!("frame-{index:04}.png")),
            frame_png(*left, *right),
        )
        .unwrap();
    }
    directory
}

fn frame_png(left: [u8; 3], right: [u8; 3]) -> Vec<u8> {
    Frame::from_pixels(
        2,
        1,
        [left, right].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .unwrap()
    .png_for_artifact()
    .unwrap()
}

fn replay(temp: &TempDir, package: &Path, frames: &Path, facts: Option<&Path>) -> Output {
    let mut args = vec![
        OsStr::new("--json"),
        OsStr::new("replay"),
        OsStr::new("--package"),
        package.as_os_str(),
        OsStr::new("--frames"),
        frames.as_os_str(),
    ];
    if let Some(facts) = facts {
        args.extend([OsStr::new("--facts"), facts.as_os_str()]);
    }
    run_actinglab(temp, args)
}

fn run_actinglab<'a>(temp: &TempDir, args: impl IntoIterator<Item = &'a OsStr>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(args)
        .env("ACTINGLAB_CONFIG_PATH", temp.path().join("config.json"))
        .stdin(Stdio::null())
        .output()
        .expect("run actinglab")
}

fn envelope(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("parse CLI JSON: {error}\n{}", describe(output)))
}

fn describe(output: &Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).unwrap();
        serde_json::to_writer(&mut zip, value).unwrap();
        zip.write_all(b"\n").unwrap();
    }
    zip.finish().unwrap().into_inner()
}
