// SPDX-License-Identifier: AGPL-3.0-only

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_contract::{
    ContainedTaskRequest, EventActor, EventPayload, EventQuery, EventSource, EventType,
    IdentifierIssuer, ProjectionPayload, ProjectionProfile, RuntimeInfo, RuntimeOperation,
    RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, TaskOutcome, TaskPayload,
    TaskSemanticFact,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::Value;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

#[test]
fn actingctl_observe_and_reset_leave_runtime_alive_and_share_projection_shape() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let mut runtime = support::RuntimeChild::spawn(root.path(), "c4_runtime_child_process");
    runtime.wait_ready(root.path());

    let observe = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "observe",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "node.a",
        ])
        .output()
        .expect("run actingctl observe");
    assert!(
        observe.status.success(),
        "actingctl observe failed: {}",
        String::from_utf8_lossy(&observe.stderr)
    );
    let observe_json: Value = serde_json::from_slice(&observe.stdout).expect("observe JSON");
    assert_eq!(
        observe_json["receipt"]["result"]["kind"],
        "readonly_observation_completed"
    );
    assert!(observe_json["events"].as_array().is_some_and(|events| {
        events
            .iter()
            .any(|event| event["event_type"] == "recognition.completed")
    }));
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture"]
    );
    runtime.assert_alive();

    let reset = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "reset",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "node.a",
        ])
        .output()
        .expect("run actingctl reset");
    assert!(
        reset.status.success(),
        "actingctl reset failed: {}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let reset_json: Value = serde_json::from_slice(&reset.stdout).expect("reset JSON");
    assert_eq!(
        reset_json["receipt"]["result"]["kind"],
        "safe_reset_completed"
    );
    assert!(reset_json["events"].is_array());
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture", "open", "reset"]
    );
    runtime.assert_alive();
    runtime.stop_clean();
    assert_eq!(
        support::backend_events(root.path()),
        [
            "capture_open",
            "capture",
            "open",
            "reset",
            "capture_close",
            "close"
        ]
    );
}

#[test]
fn actingctl_status_monitor_and_stream_are_runtime_backed() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let mut runtime = support::RuntimeChild::spawn(root.path(), "c4_runtime_child_process");
    runtime.wait_ready(root.path());
    let binary = env!("CARGO_BIN_EXE_actingctl");
    let state_root = root.path().to_str().expect("state root");

    let status = run_json(binary, ["status", "--state-root", state_root]);
    assert_eq!(status["instances"][0]["instance_alias"], "node.a");
    let holder = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("maintenance client");
    let token = holder
        .acquire_lease("node.a")
        .expect("lease for maintenance exclusion");
    let busy = Command::new(binary)
        .args(["request-shutdown", "--state-root", state_root])
        .output()
        .expect("request shutdown through ordinary CLI");
    assert!(!busy.status.success());
    assert!(String::from_utf8_lossy(&busy.stderr).contains("RuntimeBusy"));
    runtime.assert_alive();
    holder
        .release_lease(&token)
        .expect("release maintenance exclusion");
    drop(holder);

    let monitor_status = run_json(binary, ["monitor-status", "--state-root", state_root]);
    assert_eq!(monitor_status["instances"][0]["instance_alias"], "node.a");
    assert!(monitor_status["instances"][0].get("policy").is_none());

    let configured = run_json(
        binary,
        [
            "monitor-set",
            "--state-root",
            state_root,
            "--instance",
            "node.a",
            "--interval-ms",
            "60000",
            "--expect",
            "home",
        ],
    );
    assert_eq!(configured["instance_alias"], "node.a");
    assert_eq!(configured["policy"]["expected_page"], "home");

    let cleared = run_json(
        binary,
        [
            "monitor-clear",
            "--state-root",
            state_root,
            "--instance",
            "node.a",
        ],
    );
    assert_eq!(cleared["instance_alias"], "node.a");
    assert!(cleared.get("policy").is_none());

    let stream = run_json(
        binary,
        [
            "stream",
            "--state-root",
            state_root,
            "--instance",
            "node.a",
            "--max-frames",
            "2",
            "--interval-ms",
            "1",
        ],
    );
    assert_eq!(
        stream["receipt"]["result"]["kind"],
        "capture_sequence_completed"
    );
    assert_eq!(
        stream["receipt"]["result"]["sequence"]["observations"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    runtime.assert_alive();
    runtime.stop_clean();
}

#[test]
fn actingctl_runs_neutral_contained_task_without_lab_and_runtime_survives_client_exit() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.path().join("neutral-task.zip");
    let expected_sha256 = write_neutral_contained_task_package(&package);
    let mut runtime = support::RuntimeChild::spawn_for_instance(
        root.path(),
        "c4_runtime_child_process",
        "neutral.instance",
    );
    runtime.wait_ready(root.path());

    let output = run_json(
        env!("CARGO_BIN_EXE_actingctl"),
        [
            "task-run",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "neutral.instance",
            "--package",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );

    assert_eq!(
        output["receipt"]["result"]["kind"],
        "contained_task_completed"
    );
    assert_eq!(
        output["receipt"]["result"]["final_page"],
        "neutral/terminal"
    );
    assert_eq!(output["receipt"]["result"]["executed_steps"], 1);
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture", "open", "tap", "capture"]
    );
    runtime.assert_alive();
    runtime.stop_clean();
    assert_eq!(
        support::backend_events(root.path()),
        [
            "capture_open",
            "capture",
            "open",
            "tap",
            "capture",
            "capture_close",
            "close"
        ]
    );
    // One-off (to be reverted), Workflow #308 D2: keep this end-to-end state root.
    if let Some(keep) = std::env::var_os("D2_KEEP_STATE_ROOT") {
        d2_copy_tree(root.path(), Path::new(&keep));
    }
}

// One-off (to be reverted), Workflow #308 D2 evidence. The notice-to-home task package of the
// public umbrella bundle runs end to end through the Runtime and actingctl on frames composed
// of the package's own images at their declared rectangles: the event reminder with the HUD
// shown, then, after the OK tap, the home screen. `sealed` runs the published package as it is
// (pack schema 0.6); `pack07` adds a schema 0.7 color digest of the home cafe label (authored
// from the home frame) and a composite of that digest and the home template, which the home page
// requires. The state root is kept for the forensic comparison of the one-off job.
const D2_ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const D2_BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const D2_FRAME: (usize, usize) = (1280, 720);
const D2_DIGEST_REGION: (usize, usize, usize, usize) = (77, 680, 49, 18);
const D2_GRID: (usize, usize) = (8, 8);

type D2Entries = std::collections::BTreeMap<String, Vec<u8>>;

fn d2_copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create kept state root");
    for entry in fs::read_dir(from).expect("read state root") {
        let entry = entry.expect("state root entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("state root entry type").is_dir() {
            d2_copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy state root file");
        }
    }
}

fn d2_unzip(bytes: &[u8]) -> D2Entries {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("open zip");
    let mut entries = D2Entries::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).expect("zip entry");
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_owned();
        let mut data = Vec::new();
        file.read_to_end(&mut data).expect("read zip entry");
        entries.insert(name, data);
    }
    entries
}

fn d2_zip(entries: &D2Entries) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        writer
            .start_file(name.as_str(), options)
            .expect("zip entry");
        writer.write_all(data).expect("zip contents");
    }
    writer.finish().expect("finish zip").into_inner()
}

fn d2_entry_path(entries: &D2Entries, suffix: &str) -> String {
    entries
        .keys()
        .find(|path| path.ends_with(suffix))
        .unwrap_or_else(|| panic!("no entry ending with {suffix}"))
        .clone()
}

fn d2_json(entries: &D2Entries, suffix: &str) -> Value {
    serde_json::from_slice(&entries[&d2_entry_path(entries, suffix)]).expect("package JSON")
}

fn d2_target<'a>(pack: &'a Value, id: &str) -> &'a Value {
    pack["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .find(|target| target["id"] == id)
        .unwrap_or_else(|| panic!("target {id} missing"))
}

/// An RGB8 frame showing each listed template target's own image at its declared rectangle;
/// every other pixel is black.
fn d2_compose(entries: &D2Entries, pack: &Value, ids: &[&str]) -> Vec<u8> {
    let (width, height) = D2_FRAME;
    let mut canvas = vec![0_u8; width * height * 3];
    for id in ids {
        let declared = d2_target(pack, id);
        let path = declared["template_path"].as_str().expect("template_path");
        let image = actingcommand_device::Frame::from_png(
            entries[&format!("resources/{path}")].clone(),
            actingcommand_device::CaptureBackendName::AdbScreencap,
        )
        .expect("decode template image");
        assert_eq!(
            image.pixel_format,
            actingcommand_device::PixelFormat::Rgba8,
            "{path}"
        );
        let x = declared["region"]["x"].as_u64().expect("x") as usize;
        let y = declared["region"]["y"].as_u64().expect("y") as usize;
        assert_eq!(
            declared["region"]["width"].as_u64(),
            Some(u64::from(image.width)),
            "{id}"
        );
        for row in 0..image.height as usize {
            for column in 0..image.width as usize {
                let source = (row * image.width as usize + column) * 4;
                let target = ((y + row) * width + x + column) * 3;
                canvas[target..target + 3].copy_from_slice(&image.pixels[source..source + 3]);
            }
        }
    }
    canvas
}

fn d2_png(canvas: &[u8]) -> Vec<u8> {
    actingcommand_device::Frame::from_pixels(
        D2_FRAME.0 as u32,
        D2_FRAME.1 as u32,
        canvas.to_vec(),
        actingcommand_device::PixelFormat::Rgb8,
        actingcommand_device::CaptureBackendName::AdbScreencap,
    )
    .expect("composed frame")
    .encode_png_fast()
    .expect("encode composed frame")
}

/// `color_digest.v1` of the region, pixel by pixel as contracts/color-digest.md states it.
fn d2_digest(canvas: &[u8]) -> String {
    let (x, y, width, height) = D2_DIGEST_REGION;
    let (columns, rows) = D2_GRID;
    let mut hex = String::new();
    for row in 0..rows {
        for column in 0..columns {
            let (x0, x1) = (
                x + column * width / columns,
                x + (column + 1) * width / columns,
            );
            let (y0, y1) = (y + row * height / rows, y + (row + 1) * height / rows);
            let count = ((x1 - x0) * (y1 - y0)) as u64;
            let mut sums = [0_u64; 3];
            for pixel_y in y0..y1 {
                for pixel_x in x0..x1 {
                    let offset = (pixel_y * D2_FRAME.0 + pixel_x) * 3;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += u64::from(canvas[offset + channel]);
                    }
                }
            }
            for sum in sums {
                hex.push_str(&format!("{:02x}", sum / (8 * count)));
            }
        }
    }
    hex
}

/// The package with its derived pack and page set replaced and the manifest hashes renewed.
fn d2_rebuild(entries: &D2Entries, pack: &Value, pages: &Value) -> Vec<u8> {
    let mut package = entries.clone();
    let pack_path = d2_entry_path(entries, ".pack.json");
    let pages_path = d2_entry_path(entries, ".pages.json");
    package.insert(pack_path.clone(), serde_json::to_vec(pack).expect("pack"));
    package.insert(
        pages_path.clone(),
        serde_json::to_vec(pages).expect("pages"),
    );
    let mut manifest = d2_json(entries, "resources/manifest.json");
    for file in manifest["files"].as_array_mut().expect("manifest files") {
        let path = format!("resources/{}", file["path"].as_str().expect("file path"));
        if path == pack_path || path == pages_path {
            file["sha256"] =
                serde_json::json!(format!("sha256:{}", Sha256Hash::digest(&package[&path])));
        }
    }
    package.insert(
        "resources/manifest.json".to_owned(),
        serde_json::to_vec(&manifest).expect("manifest"),
    );
    d2_zip(&package)
}

#[test]
#[ignore = "one-off (to be reverted): Workflow #308 D2 evidence, run by its own CI job"]
fn one_off_d2_bundle_notice_home_task_run() {
    let scenario = std::env::var("D2_SCENARIO").expect("D2_SCENARIO");
    let keep = std::env::var_os("D2_KEEP_STATE_ROOT").expect("D2_KEEP_STATE_ROOT");
    let download = Command::new("curl")
        .args(["-sSfL", "--retry", "3", D2_ARCHIVE_URL])
        .output()
        .expect("start curl");
    assert!(
        download.status.success(),
        "curl failed: {} {}",
        download.status,
        String::from_utf8_lossy(&download.stderr)
    );
    let archive = d2_unzip(&download.stdout);
    let (bundle_name, bundle_bytes) = archive
        .iter()
        .find(|(path, _)| path.contains("/bundles/") && path.ends_with(".zip"))
        .expect("bundle in the archive");
    assert_eq!(
        Sha256Hash::digest(bundle_bytes).to_string(),
        D2_BUNDLE_SHA256,
        "{bundle_name}"
    );
    let bundle = d2_unzip(bundle_bytes);
    let index: Value = serde_json::from_slice(&bundle["bundle.json"]).expect("bundle.json");
    let declared = index["packs"]
        .as_array()
        .expect("packs")
        .iter()
        .find(|pack| {
            pack["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(".notice_home.zip"))
        })
        .expect("notice package in the bundle");
    let package_path = declared["path"].as_str().expect("path");
    let sealed = bundle[package_path].clone();
    assert_eq!(
        Sha256Hash::digest(&sealed).to_string(),
        declared["sha256"].as_str().expect("sha256")
    );
    let entries = d2_unzip(&sealed);
    let pack = d2_json(&entries, ".pack.json");
    let pages = d2_json(&entries, ".pages.json");
    let reminder = d2_compose(
        &entries,
        &pack,
        &[
            "ui/event_reminder_hud_cafe",
            "ui/event_reminder_hud_work",
            "ui/event_reminder_header",
            "page/event_reminder_visible",
            "ui/event_reminder_ok",
        ],
    );
    let home = d2_compose(&entries, &pack, &["page/home", "ui/notice_home_work"]);
    let home_page = pages["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .map(|page| page["id"].as_str().expect("page id").to_owned())
        .find(|id| id.ends_with("/home"))
        .expect("home page");
    let root = TempDir::new().expect("tempdir");
    let zip = match scenario.as_str() {
        "sealed" => sealed.clone(),
        "pack07" => {
            let cells = d2_digest(&home);
            let (x, y, width, height) = D2_DIGEST_REGION;
            let mut pack07 = pack.clone();
            pack07["schema_version"] = serde_json::json!("0.7");
            let targets = pack07["targets"].as_array_mut().expect("targets");
            targets.push(serde_json::json!({
                "type": "color_digest",
                "id": "digest/home_cafe",
                "region": {"x": x, "y": y, "width": width, "height": height},
                "algorithm": "color_digest.v1",
                "columns": D2_GRID.0,
                "rows": D2_GRID.1,
                "cells": cells,
                "exclude_cells": [],
                "max_mean_milli": 1500,
                "max_cell": 12
            }));
            targets.push(serde_json::json!({
                "type": "composite",
                "id": "check/home",
                "mode": "all_of",
                "members": ["digest/home_cafe", "page/home"]
            }));
            let mut pages07 = pages.clone();
            for page in pages07["pages"].as_array_mut().expect("pages") {
                if page["id"] == home_page.as_str() {
                    page["required"] = serde_json::json!(["check/home", "ui/notice_home_work"]);
                    page["optional"] = serde_json::json!(["digest/home_cafe"]);
                }
            }
            println!("D2 declared digest/home_cafe cells={cells}");
            println!(
                "D2 declared 0.7 targets {} {}",
                d2_target(&pack07, "digest/home_cafe"),
                d2_target(&pack07, "check/home")
            );
            fs::write(root.path().join("d2-declared-cells.txt"), cells.as_bytes())
                .expect("write declared cells");
            d2_rebuild(&entries, &pack07, &pages07)
        }
        other => panic!("unknown D2_SCENARIO {other}"),
    };
    fs::write(root.path().join("sealed.png"), d2_png(&reminder)).expect("write first frame");
    fs::write(root.path().join("after-tap.png"), d2_png(&home)).expect("write tapped frame");
    let package = root.path().join("task.zip");
    fs::write(&package, &zip).expect("write package");
    let expected_sha256 = Sha256Hash::digest(&zip).to_string();
    println!("D2 scenario={scenario} package={package_path} sha256={expected_sha256}");
    let mut runtime = support::RuntimeChild::spawn_for_instance(
        root.path(),
        "c4_runtime_child_process",
        "neutral.instance",
    );
    runtime.wait_ready(root.path());
    let output = run_json(
        env!("CARGO_BIN_EXE_actingctl"),
        [
            "task-run",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "neutral.instance",
            "--package",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    println!(
        "D2 scenario={scenario} result={}",
        output["receipt"]["result"]
    );
    assert_eq!(
        output["receipt"]["result"]["kind"],
        "contained_task_completed"
    );
    assert_eq!(
        output["receipt"]["result"]["final_page"],
        home_page.as_str()
    );
    runtime.assert_alive();
    runtime.stop_clean();
    println!(
        "D2 scenario={scenario} backend events {:?}",
        support::backend_events(root.path())
    );
    d2_copy_tree(root.path(), Path::new(&keep));
}

#[test]
fn process_replay_cannot_duplicate_or_conflict_a_contained_task_terminal() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.path().join("neutral-task.zip");
    let expected_sha256 = write_neutral_contained_task_package(&package);
    let mut runtime = support::RuntimeChild::spawn_for_instance(
        root.path(),
        "c4_runtime_child_process",
        "neutral.instance",
    );
    let info = runtime.wait_ready(root.path());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request_id = ids.mint_request_id().expect("request id");
    let correlation_id = ids.mint_correlation_id().expect("correlation id");
    let request = RuntimeRequest::new(
        request_id,
        correlation_id,
        None,
        EventActor::Cli,
        EventSource::Cli,
        1_752_147_200_000,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("holder id"),
            ContainedTaskRequest::new(package.display().to_string(), &expected_sha256)
                .expect("contained task request"),
        ),
    )
    .expect("Runtime request");

    let first = raw_exchange(&info, &request);
    if first.state() != RuntimeReceiptState::Completed {
        // Keep the first receipt and the test-owned ledger's raw sequence/link fields.
        // The receipt is already bounded by raw_exchange's one-MiB frame limit.
        let receipt = serde_json::to_string(&first).unwrap_or_else(|error| {
            format!("receipt serialization failed: {error}; original receipt: {first:?}")
        });
        eprintln!("C4 first receipt: {receipt}");
        let mut remaining = (1024_usize * 1024).saturating_sub(receipt.len());
        let evidence = (|| -> std::io::Result<()> {
            let snapshot = actingcommand_ledger::GlobalLedger::open_evidence(
                actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root.path()).with_budget(
                    64 * 1024 * 1024,
                    100_000,
                    Instant::now() + Duration::from_secs(5),
                ),
                |reference| {
                    actingcommand_artifact_store::verify_projected_read_only(root.path(), reference)
                        .ok()
                },
            )
            .map_err(std::io::Error::other)?;
            eprintln!(
                "C4 ledger backend={} through_sequence={} complete={}; original sequence/links follow:",
                snapshot.backend(),
                snapshot.latest_sequence(),
                snapshot.is_complete()
            );
            for event in snapshot.events() {
                let line = serde_json::to_string(event).map_err(std::io::Error::other)?;
                if line.len() + 1 > remaining {
                    eprintln!(
                        "C4 ledger evidence incomplete: one-MiB receipt/ledger limit reached; remaining facts omitted"
                    );
                    break;
                }
                remaining -= line.len() + 1;
                eprintln!("{line}");
            }
            Ok(())
        })();
        if let Err(error) = evidence {
            eprintln!(
                "C4 ledger evidence read failed: {error}; original receipt retained; original assertion follows"
            );
        }
    }
    assert_eq!(first.state(), RuntimeReceiptState::Completed);
    let replayed = raw_exchange(&info, &request);
    assert_eq!(replayed, first);

    let conflicting = RuntimeRequest::new(
        request_id,
        correlation_id,
        None,
        EventActor::Cli,
        EventSource::Cli,
        1_752_147_200_001,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("conflicting holder id"),
            ContainedTaskRequest::new(package.display().to_string(), "0".repeat(64))
                .expect("conflicting contained task request"),
        ),
    )
    .expect("conflicting Runtime request");
    let denied = raw_exchange(&info, &conflicting);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.error_projection().expect("denial projection").code,
        actingcommand_contract::RuntimeErrorCode::ProtocolInvalid
    );

    let ledger_client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect ledger client");
    let events = ledger_client
        .query_events(
            EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("query contained task events");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskCompleted)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::TaskFailed | EventType::TaskCancelled
                )
            })
            .count(),
        0
    );
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture", "open", "tap", "capture"]
    );
    drop(ledger_client);
    runtime.assert_alive();
    runtime.stop_clean();
    assert_eq!(
        support::backend_events(root.path()),
        [
            "capture_open",
            "capture",
            "open",
            "tap",
            "capture",
            "capture_close",
            "close"
        ]
    );
}

#[test]
fn runtime_finishes_and_rebuilds_contained_task_after_client_is_killed() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.path().join("neutral-task.zip");
    let expected_sha256 = write_neutral_contained_task_package(&package);
    let mut runtime = support::RuntimeChild::spawn_for_instance_with_input_delay(
        root.path(),
        "c4_runtime_child_process",
        "neutral.instance",
        Duration::from_millis(500),
    );
    runtime.wait_ready(root.path());

    let mut client_process = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "task-run",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "neutral.instance",
            "--package",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn actingctl task-run");
    wait_until(Duration::from_secs(5), || {
        support::backend_events(root.path())
            .iter()
            .any(|event| event == "tap_started")
    });
    client_process.kill().expect("kill actingctl client");
    let status = client_process.wait().expect("wait killed actingctl client");
    assert!(!status.success(), "killed client unexpectedly succeeded");

    wait_until(Duration::from_secs(5), || {
        let events = support::backend_events(root.path());
        events.iter().any(|event| event == "tap")
            && events.iter().filter(|event| *event == "capture").count() == 2
    });
    runtime.assert_alive();

    let ledger_client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect fresh Runtime client");
    let events = wait_for_terminal_events(&ledger_client);
    let facts = events
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    for required in [
        "package_admitted",
        "run_started",
        "evidence_indexed",
        "recognition_started",
        "recognition_completed",
        "step_started",
        "effect_intent",
        "effect_completed",
        "step_finished",
        "finalizing",
        "terminal_committed",
    ] {
        assert!(
            facts.iter().any(|fact| task_fact_kind(fact) == required),
            "missing Runtime semantic fact {required}"
        );
    }
    assert!(
        facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::TerminalCommitted {
                outcome: TaskOutcome::Success,
                final_page: Some(page),
                executed_steps: Some(1),
                failure_code: None,
                ..
            } if page == "neutral/terminal"
        )),
        "unexpected Runtime semantic facts after client kill: {facts:#?}"
    );
    let sequence = |event_type| {
        events
            .iter()
            .find(|event| event.event_type == event_type)
            .map(|event| event.sequence)
            .unwrap_or_else(|| panic!("missing {event_type:?}"))
    };
    assert!(sequence(EventType::TaskEffectIntent) < sequence(EventType::InputIntent));
    assert!(sequence(EventType::InputIntent) < sequence(EventType::InputCommitted));
    assert!(sequence(EventType::InputCommitted) < sequence(EventType::TaskEffectCompleted));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskCompleted)
            .count(),
        1
    );

    drop(ledger_client);
    runtime.assert_alive();
    runtime.stop_clean();
}

fn wait_for_terminal_events(client: &RuntimeClient) -> Vec<actingcommand_contract::ProjectedEvent> {
    let started = Instant::now();
    loop {
        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query Runtime ledger after client kill");
        if events.iter().any(|event| {
            matches!(
                event.event_type,
                EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
            )
        }) {
            return events;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Runtime task terminal did not become durable after client kill"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn task_fact_kind(fact: &TaskSemanticFact) -> &'static str {
    match fact {
        TaskSemanticFact::PackageAdmitted { .. } => "package_admitted",
        TaskSemanticFact::RunStarted => "run_started",
        TaskSemanticFact::EvidenceIndexed { .. } => "evidence_indexed",
        TaskSemanticFact::GeometryObserved { .. } => "geometry_observed",
        TaskSemanticFact::RecognitionStarted { .. } => "recognition_started",
        TaskSemanticFact::RecognitionCompleted { .. } => "recognition_completed",
        TaskSemanticFact::EntryRecognition { .. } => "entry_recognition",
        TaskSemanticFact::EntryRecoveryDecision { .. } => "entry_recovery_decision",
        TaskSemanticFact::EntryRecoveryPackageAdmitted { .. } => "entry_recovery_package_admitted",
        TaskSemanticFact::EntryRecoveryCompleted { .. } => "entry_recovery_completed",
        TaskSemanticFact::EntryRecoveryFailed { .. } => "entry_recovery_failed",
        TaskSemanticFact::EntryTargetDisposition { .. } => "entry_target_disposition",
        TaskSemanticFact::StepStarted { .. } => "step_started",
        TaskSemanticFact::SelectionEvaluated { .. } => "selection_evaluated",
        TaskSemanticFact::EffectIntent { .. } => "effect_intent",
        TaskSemanticFact::EffectCompleted { .. } => "effect_completed",
        TaskSemanticFact::StepFinished { .. } => "step_finished",
        TaskSemanticFact::Finalizing { .. } => "finalizing",
        TaskSemanticFact::TerminalCommitted { .. } => "terminal_committed",
        TaskSemanticFact::TerminalRejected { .. } => "terminal_rejected",
    }
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(started.elapsed() < timeout, "condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn raw_exchange(info: &RuntimeInfo, request: &RuntimeRequest) -> RuntimeReceipt {
    let mut stream = TcpStream::connect(info.socket_addr().expect("Runtime socket"))
        .expect("connect raw Runtime client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set write timeout");
    let body = serde_json::to_vec(request).expect("serialize Runtime request");
    assert!(!body.is_empty() && body.len() <= 1024 * 1024);
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .expect("write request header");
    stream.write_all(&body).expect("write request body");
    stream.flush().expect("flush request");
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).expect("read receipt header");
    let length = u32::from_be_bytes(header) as usize;
    assert!((1..=1024 * 1024).contains(&length));
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).expect("read receipt body");
    let receipt = serde_json::from_slice::<RuntimeReceipt>(&body).expect("decode Runtime receipt");
    receipt.validate().expect("validate Runtime receipt");
    receipt
}

fn write_neutral_contained_task_package(path: &Path) -> String {
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let files: &[(&str, &[u8])] = &[
        (
            "control.json",
            br#"{
                "schema_version":"Lab-1y.control.v1",
                "package_id":"neutral.semantic.task",
                "execution_mode":"navigable_route",
                "game":"neutral",
                "server":"test",
                "resolution":{"width":16,"height":9},
                "entry_task_id":"task",
                "capture_interval_ms":1,
                "step_timeout_ms":50,
                "timeout_ms":3000,
                "max_steps":2
            }"#,
        ),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
        ),
        (
            "resources/operations/task/task.json",
            br#"{
                "schema_version":"0.6",
                "task_id":"task",
                "game":"neutral",
                "server_scope":["test"],
                "coordinate_space":{"width":16,"height":9},
                "entry_page":"home",
                "target_page":"terminal",
                "operations":[{
                    "id":"open_terminal",
                    "from":"home",
                    "to":"terminal",
                    "click":{"kind":"point","x":1,"y":0},
                    "unguarded_trusted_coordinate":true
                }]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            br#"{
                "schema_version":"0.3",
                "game":"neutral",
                "server":"test",
                "coordinate_space":{"width":16,"height":9},
                "defaults":{"color_max_distance":0.0},
                "targets":[
                    {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                ]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            br#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                    {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}
                ]
            }"#,
        ),
    ];
    for (entry, contents) in files {
        zip.start_file(*entry, options).expect("zip entry");
        zip.write_all(contents).expect("zip contents");
    }
    let bytes = zip.finish().expect("finish zip").into_inner();
    fs::write(path, &bytes).expect("write neutral contained package");
    Sha256Hash::digest(&bytes).to_string()
}

fn run_json<const N: usize>(binary: &str, arguments: [&str; N]) -> Value {
    let output = Command::new(binary)
        .args(arguments)
        .output()
        .expect("run actingctl");
    if !output.status.success() {
        const OUTPUT_LIMIT: usize = 1024 * 1024;
        const TAIL_RESERVE: usize = 256;
        let mut evidence = String::new();
        let mut truncated = false;
        let append = |evidence: &mut String, text: &str| {
            let remaining = (OUTPUT_LIMIT - TAIL_RESERVE).saturating_sub(evidence.len());
            let mut length = text.len().min(remaining);
            while !text.is_char_boundary(length) {
                length -= 1;
            }
            evidence.push_str(&text[..length]);
            length != text.len()
        };
        truncated |= append(&mut evidence, &format!("C4 CLI exit: {}\n", output.status));
        for (label, original) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
            truncated |= append(&mut evidence, &format!("C4 CLI {label}:\n"));
            let captured = original
                .len()
                .min((OUTPUT_LIMIT - TAIL_RESERVE).saturating_sub(evidence.len()));
            match std::str::from_utf8(&original[..captured]) {
                Ok(text) => truncated |= append(&mut evidence, text),
                Err(error) => {
                    truncated |= append(
                        &mut evidence,
                        &String::from_utf8_lossy(&original[..error.valid_up_to()]),
                    );
                    truncated |= append(
                        &mut evidence,
                        &format!("\nC4 {label} evidence incomplete: UTF-8 error: {error}\n"),
                    );
                }
            }
            truncated |= captured != original.len();
            truncated |= append(&mut evidence, "\n");
        }
        let read = (|| -> std::io::Result<()> {
            let Some(state_root) = arguments
                .windows(2)
                .find(|pair| pair[0] == "--state-root")
                .map(|pair| pair[1])
                .filter(|path| !path.is_empty())
            else {
                truncated |= append(
                    &mut evidence,
                    "C4 ledger evidence incomplete: no explicit --state-root path\n",
                );
                return Ok(());
            };
            let snapshot = actingcommand_ledger::GlobalLedger::open_evidence(
                actingcommand_ledger::GlobalLedgerEvidenceConfig::new(state_root).with_budget(
                    64 * 1024 * 1024,
                    100_000,
                    Instant::now() + Duration::from_secs(5),
                ),
                |reference| {
                    actingcommand_artifact_store::verify_projected_read_only(
                        Path::new(state_root),
                        reference,
                    )
                    .ok()
                },
            )
            .map_err(std::io::Error::other)?;
            truncated |= append(
                &mut evidence,
                &format!(
                    "C4 ledger backend={} through_sequence={} complete={}; original sequence/links follow:\n",
                    snapshot.backend(),
                    snapshot.latest_sequence(),
                    snapshot.is_complete()
                ),
            );
            for event in snapshot.events() {
                let line = serde_json::to_string(event).map_err(std::io::Error::other)?;
                truncated |= append(&mut evidence, &line);
                truncated |= append(&mut evidence, "\n");
                if evidence.len() == OUTPUT_LIMIT - TAIL_RESERVE {
                    truncated = true;
                    break;
                }
            }
            Ok(())
        })();
        if let Err(error) = read {
            truncated |= append(
                &mut evidence,
                &format!("\nC4 ledger evidence read failed: {error}; original assertion follows\n"),
            );
        }
        if truncated {
            evidence.push_str("\nC4 evidence incomplete: one-MiB output limit reached; remaining bytes omitted; original assertion follows\n");
        }
        eprint!("{evidence}");
    }
    assert!(
        output.status.success(),
        "actingctl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("actingctl JSON")
}
