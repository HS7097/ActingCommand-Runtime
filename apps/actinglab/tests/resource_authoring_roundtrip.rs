// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CorrelationId, EventActor, EventQuery, EventSource, EventType, IdentifierIssuer, InstanceId,
    ProjectionProfile,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_resource_tooling::open_published_package;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tempfile::TempDir;

#[derive(Default)]
struct SealedState {
    mail_visible: AtomicBool,
    taps: AtomicUsize,
    captures: AtomicUsize,
}

struct SealedInput {
    state: Arc<SealedState>,
}

impl InputBackend for SealedInput {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        if (x, y) != (5, 6) {
            return Err(DeviceError::fatal("unexpected sealed authoring tap"));
        }
        self.state.taps.fetch_add(1, Ordering::AcqRel);
        self.state.mail_visible.store(true, Ordering::Release);
        Ok(())
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        Err(DeviceError::fatal("unexpected sealed authoring long tap"))
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal("unexpected sealed authoring swipe"))
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal("unexpected sealed authoring key"))
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal("unexpected sealed authoring text"))
    }

    fn reset(&mut self) -> DeviceResult<()> {
        Err(DeviceError::fatal("unexpected sealed authoring reset"))
    }

    fn close(&mut self) -> DeviceResult<()> {
        Ok(())
    }
}

struct SealedCapture {
    state: Arc<SealedState>,
}

impl CaptureBackend for SealedCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        self.state.captures.fetch_add(1, Ordering::AcqRel);
        frame(self.state.mail_visible.load(Ordering::Acquire))
    }
}

struct SealedProvider {
    instance_id: InstanceId,
    state: Arc<SealedState>,
}

impl ExecutionBackendProvider for SealedProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec!["ak".to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "ak")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "sealed-authoring"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        if instance_alias != "ak" {
            return Err(DeviceError::fatal("unexpected sealed authoring instance"));
        }
        Ok(Box::new(SealedInput {
            state: Arc::clone(&self.state),
        }))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        if instance_alias != "ak" {
            return Err(DeviceError::fatal("unexpected sealed authoring instance"));
        }
        Ok(Box::new(SealedCapture {
            state: Arc::clone(&self.state),
        }))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: actingcommand_contract::ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "resource authoring test does not expose application control",
        ))
    }
}

#[test]
fn recorded_resource_is_deterministically_packaged_and_runs_from_containment() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let config = root.path().join("actinglab.json");
    let state_dir = root.path().join("record");
    let repo = root.path().join("resource-repo");
    let home_frame = root.path().join("home.png");
    let mail_frame = root.path().join("mail.png");
    let package_one = root.path().join("daily-check-1.zip");
    let package_two = root.path().join("daily-check-2.zip");
    let evidence = root.path().join("evidence.zip");
    fs::write(&config, "{}").expect("config");
    fs::create_dir_all(repo.join("ours/operations")).expect("operations root");
    fs::create_dir_all(repo.join("ours/recognition")).expect("recognition root");
    fs::write(&home_frame, frame_png(false)).expect("home frame");
    fs::write(&mail_frame, frame_png(true)).expect("mail frame");

    let state = Arc::new(SealedState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-authoring-roundtrip"),
        Arc::new(SealedProvider {
            instance_id,
            state: Arc::clone(&state),
        }),
    )
    .expect("runtime host");

    run_cli(
        &config,
        &runtime_root,
        vec![
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "start",
            "--state-dir",
            path(&state_dir),
            "--task-id",
            "daily-check",
        ],
    );
    record_anchor(
        &config,
        &runtime_root,
        &state_dir,
        "home-anchor",
        "page/home",
        &home_frame,
    );
    record_anchor(
        &config,
        &runtime_root,
        &state_dir,
        "mail-anchor",
        "page/mail",
        &mail_frame,
    );
    run_cli(
        &config,
        &runtime_root,
        vec![
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "step",
            "--state-dir",
            path(&state_dir),
            "--kind",
            "operation",
            "--step-id",
            "home-to-mail",
            "--from",
            "page/home",
            "--to",
            "page/mail",
            "--click",
            "5,6",
        ],
    );
    let promoted = run_cli(
        &config,
        &runtime_root,
        vec![
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "promote",
            "--state-dir",
            path(&state_dir),
            "--repo",
            path(&repo),
            "--game",
            "arknights",
            "--server",
            "cn",
            "--locale",
            "zh-CN",
        ],
    );

    let correlation = promoted
        .pointer("/data/authoring/runtime_correlation_id")
        .and_then(Value::as_str)
        .expect("authoring correlation");
    assert_eq!(
        promoted
            .pointer("/data/authoring/receipt/correlation_id")
            .and_then(Value::as_str),
        Some(correlation)
    );
    let source_artifacts = promoted
        .pointer("/data/authoring/receipt/provenance/source_artifact_ids")
        .and_then(Value::as_array)
        .expect("source artifact provenance");
    assert_eq!(source_artifacts.len(), 2);
    let file_hashes = promoted
        .pointer("/data/authoring/receipt/file_hashes")
        .and_then(Value::as_array)
        .expect("published file hashes");
    for artifact in source_artifacts {
        let digest = artifact
            .as_str()
            .and_then(|value| value.strip_prefix("sha256:"))
            .expect("source artifact SHA-256");
        assert!(file_hashes.iter().any(|file| {
            file.get("sha256").and_then(Value::as_str) == Some(digest)
                && file
                    .get("relative_path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| path.ends_with(".png"))
        }));
    }

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("runtime client");
    let correlation_id: CorrelationId =
        serde_json::from_value(Value::String(correlation.to_string())).expect("correlation id");
    let events = client
        .query_events(
            EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("authoring events");
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::ResourceAuthoringStarted,
            EventType::ResourceDraftBuilt,
            EventType::ResourceValidationCompleted,
            EventType::ResourcePromoteIntent,
            EventType::ResourcePromoted,
        ]
    );
    assert!(
        events
            .iter()
            .all(|event| event.links.correlation_id().copied() == Some(correlation_id))
    );

    build_package(&config, &runtime_root, &repo, &package_one);
    build_package(&config, &runtime_root, &repo, &package_two);
    let package_hash = sha256_file(&package_one);
    assert_eq!(read_published(&package_one), read_published(&package_two));
    assert_eq!(
        promoted
            .pointer("/data/authoring/receipt/validation/package_sha256")
            .and_then(Value::as_str),
        Some(package_hash.as_str())
    );

    let completed = run_cli(
        &config,
        &runtime_root,
        vec![
            "--json",
            "--instance",
            "ak",
            "--game",
            "arknights",
            "--server",
            "cn",
            "lab",
            "run",
            "--zip",
            path(&package_one),
            "--expected-sha256",
            package_hash.as_str(),
            "--out",
            path(&evidence),
        ],
    );
    assert_eq!(completed.get("ok").and_then(Value::as_bool), Some(true));
    assert!(evidence.is_file());
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert!(state.captures.load(Ordering::Acquire) >= 2);

    drop(client);
    host.close().expect("close runtime host");
}

fn record_anchor(
    config: &Path,
    runtime_root: &Path,
    state_dir: &Path,
    step_id: &str,
    anchor_id: &str,
    frame_path: &Path,
) {
    run_cli(
        config,
        runtime_root,
        vec![
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "step",
            "--state-dir",
            path(state_dir),
            "--kind",
            "anchor",
            "--step-id",
            step_id,
            "--id",
            anchor_id,
            "--region",
            "2,3,4,5",
            "--frame",
            path(frame_path),
        ],
    );
}

fn build_package(config: &Path, runtime_root: &Path, repo: &Path, out: &Path) {
    let output = run_cli(
        config,
        runtime_root,
        vec![
            "--json",
            "package",
            "build-task",
            "--repo",
            path(repo),
            "--task",
            "daily-check",
            "--out",
            path(out),
        ],
    );
    assert_eq!(
        output.pointer("/data/status").and_then(Value::as_str),
        Some("written")
    );
}

fn run_cli(config: &Path, runtime_root: &Path, args: Vec<&str>) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(args)
        .env("ACTINGLAB_CONFIG_PATH", config)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", runtime_root)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .output()
        .expect("run actinglab");
    let envelope = serde_json::from_slice::<Value>(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid ActingLab JSON ({error}); stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(
        output.status.success(),
        "ActingLab failed: {envelope}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    envelope
}

fn path(path: &Path) -> &str {
    path.to_str().expect("UTF-8 test path")
}

fn sha256_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(read_published(path)))
}

fn read_published(path: &Path) -> Vec<u8> {
    open_published_package(path)
        .expect("open published package")
        .read_all()
        .expect("read published package")
}

fn frame_png(mail_visible: bool) -> Vec<u8> {
    frame(mail_visible)
        .expect("frame")
        .png_for_artifact()
        .expect("frame PNG")
}

fn frame(mail_visible: bool) -> DeviceResult<Frame> {
    let mut pixels = Vec::new();
    for y in 0..10_u32 {
        for x in 0..12_u32 {
            if mail_visible {
                pixels.extend_from_slice(&[
                    ((x * 37 + y * 17 + 91) % 256) as u8,
                    ((x * 13 + y * 53 + 7) % 256) as u8,
                    ((x * 97 + y * 11 + 3) % 256) as u8,
                    255,
                ]);
            } else {
                pixels.extend_from_slice(&[x as u8, y as u8, 128, 255]);
            }
        }
    }
    Frame::from_pixels(
        12,
        10,
        pixels,
        PixelFormat::Rgba8,
        CaptureBackendName::AdbScreencap,
    )
}
