// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    ApplicationLifecycleAction, CaptureSequenceSpec, EventAction, EventActor, EventPayload,
    EventQuery, EventSource, EventType, IdentifierIssuer, InstanceId, PackageDebugRequest,
    ProjectionPayload, ProjectionProfile, RetentionClass, RuntimeEvidenceExportRequest,
    RuntimeResult, TaskOutcome, TaskPayload, TaskSemanticFact,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

#[derive(Default)]
struct FakeState {
    taps: AtomicUsize,
    captures: AtomicUsize,
    closes: AtomicUsize,
    application_calls: AtomicUsize,
    application_action: AtomicUsize,
    transition_after_tap: AtomicBool,
    tap_started: AtomicBool,
    tap_delay_ms: AtomicUsize,
}

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

impl InputBackend for FakeBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.state.tap_started.store(true, Ordering::Release);
        let delay_ms = self.state.tap_delay_ms.load(Ordering::Acquire);
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms as u64));
        }
        self.state.taps.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        Ok(())
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn reset(&mut self) -> DeviceResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DeviceResult<()> {
        if !self.closed {
            self.closed = true;
            self.state.closes.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }
}

struct FakeProvider {
    instance_alias: &'static str,
    instance_id: InstanceId,
    state: Arc<FakeState>,
    frame_size: u32,
}

struct FakeCapture {
    state: Arc<FakeState>,
    frame_size: u32,
}

impl CaptureBackend for FakeCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        self.state.captures.fetch_add(1, Ordering::AcqRel);
        let color = if self.state.transition_after_tap.load(Ordering::Acquire)
            && self.state.taps.load(Ordering::Acquire) > 0
        {
            [0, 0, 255]
        } else {
            [255, 0, 0]
        };
        let pixels = (0..self.frame_size * self.frame_size)
            .flat_map(|_| color)
            .collect();
        Frame::from_pixels(
            self.frame_size,
            self.frame_size,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec![self.instance_alias.to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == self.instance_alias)
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<sealed-test>"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        assert_eq!(instance_alias, self.instance_alias);
        Ok(Box::new(FakeBackend {
            state: Arc::clone(&self.state),
            closed: false,
        }))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        assert_eq!(instance_alias, self.instance_alias);
        Ok(Box::new(FakeCapture {
            state: Arc::clone(&self.state),
            frame_size: self.frame_size,
        }))
    }

    fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        assert_eq!(instance_alias, self.instance_alias);
        self.state.application_calls.fetch_add(1, Ordering::AcqRel);
        self.state.application_action.store(
            match action {
                ApplicationLifecycleAction::Launch => 1,
                ApplicationLifecycleAction::Stop => 2,
                ApplicationLifecycleAction::Restart => 3,
            },
            Ordering::Release,
        );
        Ok(())
    }
}

#[test]
fn session_app_routes_application_lifecycle_through_runtime_without_client_package_identity() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-application-test"),
        Arc::new(FakeProvider {
            instance_alias: "neutral.instance",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let lease_client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("lease client");
    let token = lease_client
        .acquire_lease("neutral.instance")
        .expect("active lease");
    let (busy_exit, busy) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "neutral.instance",
            "session",
            "app",
            "force-stop",
        ],
    );
    assert_eq!(busy_exit, 4);
    assert_eq!(busy["error"]["code"], "device_error");
    assert!(
        busy["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("LeaseBusy"))
    );
    assert_eq!(state.application_calls.load(Ordering::Acquire), 0);
    lease_client
        .release_lease(&token)
        .expect("release active lease");

    let output = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "neutral.instance",
            "session",
            "app",
            "force-stop",
        ],
    );
    assert_eq!(
        output["data"]["receipt"]["result"]["kind"],
        "application_lifecycle_completed"
    );
    assert_eq!(output["data"]["receipt"]["result"]["action"], "stop");
    assert_eq!(state.application_calls.load(Ordering::Acquire), 1);
    assert_eq!(state.application_action.load(Ordering::Acquire), 2);

    let (exit_code, rejected) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "neutral.instance",
            "session",
            "app",
            "launch",
            "--package",
            "client.supplied.identity",
        ],
    );
    assert_eq!(exit_code, 2);
    assert_eq!(rejected["error"]["code"], "validation_failed");
    assert_eq!(state.application_calls.load(Ordering::Acquire), 1);
    drop(lease_client);
    host.close().expect("close host");
}

#[test]
fn session_status_and_monitor_policy_project_resident_runtime_without_legacy_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let legacy_session_root = local_app_data.join("ActingCommand/actinglab/session");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-session-adapter-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state,
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let status = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        ["--json", "session", "status", "--diagnostics"],
    );
    assert_eq!(status["data"]["running"], true);
    assert_eq!(
        status["data"]["diagnostics"]["liveness"]["authority"],
        "runtime"
    );
    assert_eq!(
        status["data"]["diagnostics"]["instances"]["instances"][0]["instance_alias"],
        "node.a"
    );

    let unconfigured = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "monitor-policy",
            "status",
        ],
    );
    assert_eq!(unconfigured["data"]["configured"], false);

    let configured = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "monitor-policy",
            "set",
            "--capture",
            "--expect",
            "home",
            "--interval-ms",
            "60000",
        ],
    );
    assert_eq!(configured["data"]["status"], "configured");
    assert_eq!(
        configured["data"]["policy"]["runtime_policy"]["expected_page"],
        "home"
    );

    let cleared = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "monitor-policy",
            "clear",
        ],
    );
    assert_eq!(cleared["data"]["status"], "cleared");
    assert_eq!(cleared["data"]["state_preserved"], false);
    assert!(!legacy_session_root.exists());
    host.close().expect("close host");
}

#[test]
fn session_stream_projects_runtime_capture_sequence_without_legacy_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let legacy_session_root = local_app_data.join("ActingCommand/actinglab/session");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-stream-adapter-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let stream = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "stream",
            "--max-frames",
            "2",
            "--interval-ms",
            "1",
        ],
    );

    assert_eq!(stream["data"]["mode"], "bounded_stream");
    for field in [
        "stream_id",
        "mode",
        "instance",
        "transport",
        "max_frames",
        "interval_ms",
        "capture",
        "trusted_channel",
        "contract",
        "input_relay",
        "events",
        "frames",
    ] {
        assert!(stream["data"].get(field).is_some(), "missing {field}");
    }
    assert_eq!(
        stream["data"]["contract"]["schema_version"],
        "session.stream.v0.1"
    );
    assert_eq!(stream["data"]["input_relay"]["status"], "disabled");
    let frames = stream["data"]["frames"].as_array().expect("stream frames");
    assert_eq!(frames.len(), 2);
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame["index"], index);
        assert_eq!(frame["captured"], true);
        assert_eq!(frame["frame"]["width"], 2);
        assert_eq!(frame["frame"]["height"], 2);
        assert_eq!(frame["freshness"]["status"], "runtime_artifact_verified");
        assert!(frame["artifact"]["object_key"].is_string());
        assert_eq!(frame["frame"]["digest"], frame["artifact"]["sha256"]);
    }
    let event_types = stream["data"]["events"]
        .as_array()
        .expect("stream events")
        .iter()
        .map(|event| event["type"].as_str().expect("event type"))
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        [
            "stream.started",
            "stream.frame_sampled",
            "stream.frame_sampled",
            "stream.completed"
        ]
    );
    assert_eq!(state.captures.load(Ordering::Acquire), 2);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    assert!(!legacy_session_root.exists());

    let reconnected = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        ["--json", "session", "status"],
    );
    assert_eq!(reconnected["data"]["running"], true);

    let (fresh_exit, fresh_error) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "stream",
            "--max-frames",
            "2",
            "--require-fresh",
        ],
    );
    assert_eq!(fresh_exit, 2);
    assert_eq!(fresh_error["error"]["code"], "validation_failed");
    assert!(
        fresh_error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("not supported"))
    );
    assert_eq!(state.captures.load(Ordering::Acquire), 2);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime remains discoverable");
    client.health().expect("Runtime remains alive");
    drop(client);
    host.close().expect("close host");
}

#[test]
fn runtime_backed_session_clients_fail_visibly_when_runtime_is_unavailable() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("missing-runtime");
    let local_app_data = root.path().join("local-app-data");
    let legacy_session_root = local_app_data.join("ActingCommand/actinglab/session");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");

    let failures = [
        run_actinglab_failure_json(
            &config_path,
            &runtime_root,
            &local_app_data,
            ["--json", "session", "status"],
        ),
        run_actinglab_failure_json(
            &config_path,
            &runtime_root,
            &local_app_data,
            [
                "--json",
                "--instance",
                "node.a",
                "session",
                "monitor-policy",
                "status",
            ],
        ),
        run_actinglab_failure_json(
            &config_path,
            &runtime_root,
            &local_app_data,
            [
                "--json",
                "--instance",
                "node.a",
                "session",
                "stream",
                "--max-frames",
                "2",
            ],
        ),
    ];
    for (exit_code, failure) in failures {
        assert_eq!(exit_code, 5);
        assert_eq!(failure["ok"], false);
        assert_eq!(failure["error"]["code"], "runtime_not_running");
        assert!(failure["data"].is_null());
    }
    assert!(!legacy_session_root.exists());
}

fn run_actinglab_json<const N: usize>(
    config_path: &Path,
    runtime_root: &Path,
    local_app_data: &Path,
    arguments: [&str; N],
) -> Value {
    let output = run_actinglab_output(config_path, runtime_root, local_app_data, arguments);
    assert!(
        output.status.success(),
        "actinglab failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("ActingLab JSON")
}

fn run_actinglab_failure_json<const N: usize>(
    config_path: &Path,
    runtime_root: &Path,
    local_app_data: &Path,
    arguments: [&str; N],
) -> (i32, Value) {
    let output = run_actinglab_output(config_path, runtime_root, local_app_data, arguments);
    assert!(
        !output.status.success(),
        "actinglab unexpectedly succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let exit_code = output.status.code().expect("actinglab exit code");
    let envelope = serde_json::from_slice(&output.stdout).expect("ActingLab error JSON");
    (exit_code, envelope)
}

fn run_actinglab_output<const N: usize>(
    config_path: &Path,
    runtime_root: &Path,
    local_app_data: &Path,
    arguments: [&str; N],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(arguments)
        .env("ACTINGLAB_CONFIG_PATH", config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", runtime_root)
        .env("LOCALAPPDATA", local_app_data)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .output()
        .expect("run actinglab")
}

#[test]
fn lab_package_debug_is_a_correlated_runtime_request_without_device_authority() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let package = root.path().join("debug-package.zip");
    write_runtime_owned_lab_package(&package);
    let expected_sha256 = format!("{:x}", Sha256::digest(fs::read(&package).expect("package")));
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-package-debug-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let output = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "debug-package",
            "--zip",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(output["data"]["authority"], "runtime");
    assert_eq!(output["data"]["summary"]["task_id"], "task");
    assert_eq!(
        output["data"]["summary"]["verified_sha256"],
        expected_sha256
    );
    assert_eq!(
        output["data"]["terminal_receipt"]["correlation_id"],
        output["data"]["correlation_id"]
    );
    assert!(
        output["data"]["events"]
            .as_array()
            .is_some_and(|events| !events.is_empty())
    );
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    let correlation_id = output["data"]["correlation_id"]
        .as_str()
        .expect("debug correlation");
    let watch = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "watch",
            "--req",
            correlation_id,
            "--after",
            "0",
            "--wait-ms",
            "50",
            "--max-events",
            "16",
        ],
    );
    assert_eq!(watch["data"]["authority"], "runtime_global_ledger");
    assert_eq!(watch["data"]["progress"]["state"], "advanced");
    assert!(
        watch["data"]["progress"]["event_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert!(watch["data"]["progress"].get("percent").is_none());
    assert!(watch["data"]["progress"].get("completed").is_none());
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    let (_, failure) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "debug-package",
            "--zip",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &"0".repeat(64),
        ],
    );
    assert_eq!(failure["ok"], false);
    assert_eq!(host.runtime_info().pid(), std::process::id());
    host.close().expect("close host");
}

#[test]
fn runtime_owned_evidence_export_has_a_sealed_offline_replay_path() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    let package = root.path().join("debug-package.zip");
    let evidence = root.path().join("runtime-evidence.zip");
    fs::write(&config_path, "{}").expect("write config");
    write_runtime_owned_lab_package(&package);
    let expected_sha256 = format!("{:x}", Sha256::digest(fs::read(&package).expect("package")));
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-evidence-export-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let export_output = run_actinglab_output(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "export-evidence",
            "--zip",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
            "--out",
            evidence.to_str().expect("evidence path"),
            "--outcome",
            "success",
        ],
    );
    if !export_output.status.success() {
        let fatal = host.fatal_error().expect("Runtime fatal state");
        panic!(
            "evidence export failed: stdout={} stderr={} fatal={fatal:#?}",
            String::from_utf8_lossy(&export_output.stdout),
            String::from_utf8_lossy(&export_output.stderr),
        );
    }
    let export = serde_json::from_slice::<Value>(&export_output.stdout).expect("export JSON");
    assert_eq!(export["data"]["authority"], "runtime");
    assert_eq!(export["data"]["summary"]["task_outcome"], "success");
    assert_eq!(
        export["data"]["summary"]["evidence_completeness"],
        "complete"
    );
    assert_eq!(
        export["data"]["summary"]["screenshot_counts"]["persisted"],
        0
    );
    assert_eq!(
        export["data"]["terminal_receipt"]["correlation_id"],
        export["data"]["correlation_id"]
    );
    assert!(evidence.is_file());
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    let exported_bytes = fs::read(&evidence).expect("evidence bytes");

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::TaskCompleted)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::ArtifactExportCompleted)
    );
    drop(client);

    let (_, collision) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "export-evidence",
            "--zip",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
            "--out",
            evidence.to_str().expect("evidence path"),
            "--outcome",
            "success",
        ],
    );
    assert_eq!(collision["ok"], false);
    assert_eq!(
        fs::read(&evidence).expect("evidence after collision"),
        exported_bytes
    );
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("Runtime fatal state").is_none());
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client after collision");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events after collision");
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::ArtifactExportFailed)
    );
    drop(client);

    let zip_sha256 = export["data"]["summary"]["zip_sha256"]
        .as_str()
        .expect("ZIP digest")
        .to_string();
    host.close().expect("close host");
    let replay = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "replay-evidence",
            "--zip",
            evidence.to_str().expect("evidence path"),
            "--expected-sha256",
            &zip_sha256,
        ],
    );
    assert_eq!(replay["data"]["authority"], "sealed_offline_verifier");
    assert_eq!(replay["data"]["zip_sha256"], zip_sha256);
    assert_eq!(replay["data"]["manifest"]["task_outcome"], "success");
    assert_eq!(
        replay["data"]["manifest_sha256"],
        export["data"]["summary"]["manifest_sha256"]
    );

    let (_, mismatch) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "replay-evidence",
            "--zip",
            evidence.to_str().expect("evidence path"),
            "--expected-sha256",
            &"0".repeat(64),
        ],
    );
    assert_eq!(mismatch["ok"], false);
}

#[test]
fn runtime_debug_session_exports_verified_debug_full_capture_evidence() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let package = root.path().join("debug-package.zip");
    let evidence = root.path().join("captured-evidence.zip");
    write_runtime_owned_lab_package(&package);
    let expected_sha256 = format!("{:x}", Sha256::digest(fs::read(&package).expect("package")));
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-captured-evidence-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let session = client.begin_debug_session().expect("debug session");
    session
        .debug_package(
            PackageDebugRequest::new(package.to_string_lossy().into_owned(), expected_sha256)
                .expect("debug package request"),
        )
        .expect("debug package");
    session
        .capture_sequence(
            "node.a",
            CaptureSequenceSpec::new(1, 0).expect("capture spec"),
        )
        .expect("Runtime capture sequence");
    let receipt = session
        .export_evidence(
            RuntimeEvidenceExportRequest::new(
                evidence.to_string_lossy().into_owned(),
                TaskOutcome::Success,
            )
            .expect("evidence request"),
        )
        .expect("evidence export");
    let summary = match receipt.result() {
        Some(RuntimeResult::EvidenceExportCompleted { summary }) => summary,
        other => panic!("unexpected evidence result: {other:?}"),
    };
    assert_eq!(summary.screenshot_counts().captured, 1);
    assert_eq!(summary.screenshot_counts().persisted, 1);
    assert_eq!(summary.archive().retention_class, RetentionClass::DebugFull);
    let events = session
        .query_events(ProjectionProfile::Forensic)
        .expect("debug events");
    let captures = events
        .iter()
        .flat_map(|event| &event.artifacts)
        .filter(|artifact| artifact.kind() == actingcommand_contract::ArtifactKind::CaptureFrame)
        .collect::<Vec<_>>();
    assert!(!captures.is_empty());
    assert!(captures.iter().all(|artifact| {
        artifact.run_id == Some(summary.run_id())
            && artifact.retention_class == RetentionClass::DebugFull
    }));
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn production_do_uses_runtime_capture_and_fenced_input() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let resources = root.path().join("resources");
    let semantic_package = root.path().join("semantic.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_semantic_resources(&resources);
    write_semantic_package(&semantic_package, &resources);
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
    );
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-drive-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args([
            "--json",
            "--instance",
            "node.a",
            "do",
            "home_button",
            "--capture",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--expected-sha256",
            &expected_sha256,
        ])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .output()
        .expect("run actinglab do");

    assert!(
        output.status.success(),
        "actinglab failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = serde_json::from_slice::<Value>(&output.stdout).expect("CLI JSON");
    assert_eq!(
        envelope
            .pointer("/data/device/backend")
            .and_then(Value::as_str),
        Some("runtime_proxy")
    );
    assert!(envelope.pointer("/data/needs_detection").is_none());
    assert_eq!(state.captures.load(Ordering::Acquire), 2);
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    host.close().expect("close host");
}

#[test]
fn online_lab2_observe_and_do_share_runtime_authority_without_local_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let resources = root.path().join("resources");
    let semantic_package = root.path().join("semantic.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_semantic_resources(&resources);
    write_semantic_package(&semantic_package, &resources);
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
    );
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-lab2-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let observe = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "observe",
            "--capture",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(
        observe
            .pointer("/data/arbitration/authority")
            .and_then(Value::as_str),
        Some("runtime_scheduler")
    );

    let action = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "home_button",
            "--capture",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(
        action.pointer("/data/executed").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        action
            .pointer("/data/device/authority")
            .and_then(Value::as_str),
        Some("runtime_execution_kernel")
    );
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert!(state.captures.load(Ordering::Acquire) >= 3);
    assert!(!local_app_data.join("ActingCommand/actinglab/lab2").exists());

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    let input = events
        .iter()
        .find(|event| event.event_type == EventType::InputCommitted)
        .expect("input committed");
    let correlation = input.links.correlation_id().copied().expect("correlation");
    let correlated = events
        .iter()
        .filter(|event| event.links.correlation_id().copied() == Some(correlation))
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_event_order(
        &correlated,
        &[
            EventType::CaptureCompleted,
            EventType::LeaseGranted,
            EventType::InputCommitted,
            EventType::LeaseReleased,
            EventType::CaptureCompleted,
        ],
    );

    drop(client);
    host.close().expect("close host");
}

#[test]
fn online_lab2_do_guard_failure_records_observation_without_runtime_input() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let resources = root.path().join("resources");
    let semantic_package = root.path().join("semantic.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_semantic_resources(&resources);
    let pack_path = resources.join("recognition/arknights.cn.pack.json");
    let pack = fs::read_to_string(&pack_path).expect("recognition pack");
    fs::write(&pack_path, pack.replace("[255,0,0]", "[0,0,255]"))
        .expect("mismatched recognition pack");
    write_semantic_package(&semantic_package, &resources);
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
    );
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-lab2-guard-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let (exit_code, failure) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "home_button",
            "--capture",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(exit_code, 3, "{failure}");
    assert_eq!(failure["error"]["code"], "target_not_visible");
    assert_eq!(
        failure["error"]["details"]["needs_detection"],
        serde_json::json!({
            "status": "needs_detection",
            "reason": "resource_drift",
            "command": "do",
            "subject": "home_button",
            "detector_ids": [],
            "keys": [],
            "recommended_action": "run_detect"
        })
    );
    assert_eq!(
        failure["error"]["details"]["ledger"]["authority"],
        "runtime_global_ledger"
    );
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::CaptureCompleted)
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != EventType::InputCommitted)
    );

    drop(client);
    host.close().expect("close host");
}

#[test]
fn online_lab2_ensure_and_wait_use_runtime_authority_without_local_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let resources = root.path().join("resources");
    let semantic_package = root.path().join("navigation.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_navigation_resources(&resources);
    write_semantic_package(&semantic_package, &resources);
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
    );
    let state = Arc::new(FakeState::default());
    state.transition_after_tap.store(true, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-lab2-route-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let wait_home = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "wait",
            "--capture",
            "--page",
            "home",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(wait_home["data"]["state"], "arrived");
    assert_eq!(
        wait_home["data"]["arbitration"]["authority"],
        "runtime_scheduler"
    );

    let ensure = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "ensure",
            "--capture",
            "--to",
            "target",
            "--step-timeout-ms",
            "100",
            "--poll-ms",
            "1",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(ensure["data"]["state"], "arrived");
    assert_eq!(ensure["data"]["page"], "arknights/target");
    assert_eq!(ensure["data"]["executed"], true);
    assert_eq!(
        ensure["data"]["arbitration"]["authority"],
        "runtime_scheduler"
    );

    let wait_stable = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "wait",
            "--capture",
            "--stable",
            "target_anchor",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(wait_stable["data"]["state"], "stable");
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert!(state.captures.load(Ordering::Acquire) >= 6);
    assert!(!local_app_data.join("ActingCommand/actinglab/lab2").exists());

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    let input = events
        .iter()
        .find(|event| event.event_type == EventType::InputCommitted)
        .expect("input committed");
    let correlation = input.links.correlation_id().copied().expect("correlation");
    let correlated = events
        .iter()
        .filter(|event| event.links.correlation_id().copied() == Some(correlation))
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_event_order(
        &correlated,
        &[
            EventType::CaptureCompleted,
            EventType::LeaseGranted,
            EventType::InputCommitted,
            EventType::LeaseReleased,
            EventType::CaptureCompleted,
        ],
    );

    drop(client);
    host.close().expect("close host");
}

fn write_semantic_resources(root: &std::path::Path) {
    let recognition = root.join("recognition");
    let navigation = root.join("navigation");
    fs::create_dir_all(&recognition).expect("recognition dir");
    fs::create_dir_all(&navigation).expect("navigation dir");
    fs::write(
        recognition.join("arknights.cn.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":0,"y":0,"width":1,"height":1}}
            ]
        }"#,
    )
    .expect("recognition pack");
    fs::write(
        recognition.join("arknights.cn.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[{"id":"arknights/home","required":["home_button"]}]
        }"#,
    )
    .expect("page set");
    fs::write(
        navigation.join("arknights.cn.navigation.json"),
        r#"{"schema_version":"0.3","game":"arknights","server":"cn","navigation":[],"destructive_actions":[]}"#,
    )
    .expect("navigation graph");
}

fn write_navigation_resources(root: &std::path::Path) {
    let recognition = root.join("recognition");
    let navigation = root.join("navigation");
    fs::create_dir_all(&recognition).expect("recognition dir");
    fs::create_dir_all(&navigation).expect("navigation dir");
    fs::write(
        recognition.join("arknights.cn.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"target_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
            ]
        }"#,
    )
    .expect("recognition pack");
    fs::write(
        recognition.join("arknights.cn.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[
                {"id":"arknights/home","required":["home_anchor"]},
                {"id":"arknights/target","required":["target_anchor"]}
            ]
        }"#,
    )
    .expect("page set");
    fs::write(
        navigation.join("arknights.cn.navigation.json"),
        r#"{
            "schema_version":"0.3",
            "game":"arknights",
            "server":"cn",
            "navigation":[{
                "id":"home_to_target",
                "from_page":"arknights/home",
                "to_page":"arknights/target",
                "click":{"kind":"point","x":0,"y":0}
            }],
            "destructive_actions":[]
        }"#,
    )
    .expect("navigation graph");
}

fn write_semantic_package(path: &Path, root: &Path) {
    let pack = fs::read(root.join("recognition/arknights.cn.pack.json")).expect("pack");
    let pages = fs::read(root.join("recognition/arknights.cn.pages.json")).expect("pages");
    let navigation =
        fs::read(root.join("navigation/arknights.cn.navigation.json")).expect("navigation");
    let navigation_value: Value = serde_json::from_slice(&navigation).expect("navigation JSON");
    let pack_value: Value = serde_json::from_slice(&pack).expect("pack JSON");
    let game = navigation_value["game"].as_str().expect("navigation game");
    let server = navigation_value["server"]
        .as_str()
        .expect("navigation server");
    let operations = navigation_value["navigation"]
        .as_array()
        .expect("navigation routes")
        .iter()
        .map(|route| {
            json!({
                "id": route["id"],
                "purpose": "runtime semantic closure",
                "from": route["from_page"],
                "to": route["to_page"],
                "click": route["click"],
                "unguarded_trusted_coordinate": true
            })
        })
        .collect::<Vec<_>>();
    let width = pack_value["coordinate_space"]["width"]
        .as_u64()
        .expect("pack width");
    let height = pack_value["coordinate_space"]["height"]
        .as_u64()
        .expect("pack height");
    let navigable = !operations.is_empty();
    let control = serde_json::to_vec(&json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "runtime.semantic.fixture",
        "execution_mode": if navigable { "navigable_route" } else { "recognize_only" },
        "game": game,
        "server": server,
        "resolution": {"width": width, "height": height},
        "entry_task_id": "task",
        "allow_placeholder_coords": true
    }))
    .expect("control JSON");
    let operation = serde_json::to_vec(&json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": game,
        "server_scope": [server],
        "goal": "runtime semantic closure",
        "coordinate_space": {"width": width, "height": height},
        "operations": operations
    }))
    .expect("operation JSON");
    write_zip(
        path,
        &[
            ("control.json", &control),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            ("resources/operations/task/task.json", &operation),
            ("resources/recognition/arknights.cn.pack.json", &pack),
            ("resources/recognition/arknights.cn.pages.json", &pages),
            (
                "resources/navigation/arknights.cn.navigation.json",
                &navigation,
            ),
        ],
    );
}

#[test]
fn production_tap_uses_runtime_proxy_without_local_adb_configuration() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-proxy-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(["--instance", "node.a", "tap", "10", "20"])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .output()
        .expect("run actinglab tap");

    assert!(
        output.status.success(),
        "actinglab failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = serde_json::from_slice::<Value>(&output.stdout).expect("CLI JSON");
    assert_eq!(
        envelope.pointer("/data/backend").and_then(Value::as_str),
        Some("runtime_proxy")
    );
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

#[test]
fn production_lab_run_routes_device_effects_through_runtime_only() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let config_path = root.path().join("actinglab.json");
    let package_path = root.path().join("runtime-owned-lab.zip");
    let result_path = root.path().join("result.zip");
    let adb_marker = root.path().join("forbidden-adb-invoked");
    fs::write(&config_path, "{}").expect("write config");
    write_runtime_owned_lab_package(&package_path);
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&package_path).expect("read package"))
    );
    let forbidden_adb = write_forbidden_adb(root.path(), &adb_marker);
    let state = Arc::new(FakeState::default());
    state.transition_after_tap.store(true, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-run-test"),
        Arc::new(FakeProvider {
            instance_alias: "neutral.instance",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args([
            "--json",
            "--instance",
            "neutral.instance",
            "--game",
            "neutral",
            "--server",
            "test",
            "lab",
            "run",
            "--zip",
            package_path.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
            "--out",
            result_path.to_str().expect("result path"),
        ])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env("ACTINGCOMMAND_ADB_PATH", &forbidden_adb)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .output()
        .expect("run actinglab lab run");

    assert!(output.status.success(), "Runtime-owned task must complete");
    let envelope = serde_json::from_slice::<Value>(&output.stdout).expect("CLI JSON");
    assert_eq!(
        envelope
            .pointer("/data/runtime_flow/receipt/result/kind")
            .and_then(Value::as_str),
        Some("contained_task_completed"),
        "unexpected Lab run response: {envelope}"
    );
    assert_eq!(
        envelope
            .pointer("/data/runtime_flow/receipt/result/final_page")
            .and_then(Value::as_str),
        Some("neutral/terminal")
    );
    assert!(result_path.is_file());
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert!(state.captures.load(Ordering::Acquire) >= 2);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    assert!(
        !adb_marker.exists(),
        "ActingLab invoked a local ADB backend"
    );

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    let terminal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::TaskCompleted
                && matches!(
                    &event.payload,
                    ProjectionPayload::Full(payload)
                        if payload.as_ref().action() == EventAction::RuntimeTaskRun
                )
        })
        .expect("Runtime Lab run terminal");
    let correlation = *terminal
        .links
        .correlation_id()
        .expect("Runtime Lab run correlation");
    let correlated = events
        .iter()
        .filter(|event| event.links.correlation_id() == Some(&correlation))
        .collect::<Vec<_>>();
    let event_types = correlated
        .iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_event_order(
        &event_types,
        &[
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::LeaseRequested,
            EventType::LeaseGranted,
            EventType::TaskRequested,
            EventType::TaskStarted,
            EventType::CaptureRequested,
            EventType::CaptureCompleted,
            EventType::RecognitionCompleted,
            EventType::TaskStepStarted,
            EventType::InputIntent,
            EventType::InputCommitted,
            EventType::TaskStepFinished,
            EventType::TaskTerminalIntent,
            EventType::TaskCompleted,
            EventType::LeaseReleased,
        ],
    );
    let actions = correlated
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => Some(payload.as_ref().action()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(actions.contains(&EventAction::RuntimeTaskRun));

    drop(client);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

#[test]
fn runtime_finishes_and_rebuilds_lab_run_after_actinglab_client_is_killed() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    let package_path = root.path().join("runtime-owned-lab.zip");
    let result_path = root.path().join("client-result.zip");
    let adb_marker = root.path().join("forbidden-adb-invoked");
    fs::write(&config_path, "{}").expect("write config");
    write_runtime_owned_lab_package(&package_path);
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&package_path).expect("read package"))
    );
    let forbidden_adb = write_forbidden_adb(root.path(), &adb_marker);
    let state = Arc::new(FakeState::default());
    state.transition_after_tap.store(true, Ordering::Release);
    state.tap_delay_ms.store(500, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-killed-client-test"),
        Arc::new(FakeProvider {
            instance_alias: "neutral.instance",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let mut client_process = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args([
            "--json",
            "--instance",
            "neutral.instance",
            "--game",
            "neutral",
            "--server",
            "test",
            "lab",
            "run",
            "--zip",
            package_path.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
            "--out",
            result_path.to_str().expect("result path"),
        ])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env("LOCALAPPDATA", &local_app_data)
        .env("ACTINGCOMMAND_ADB_PATH", &forbidden_adb)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ActingLab client");
    wait_until(Duration::from_secs(5), || {
        state.tap_started.load(Ordering::Acquire)
    });
    client_process.kill().expect("kill ActingLab client");
    let status = client_process.wait().expect("wait killed ActingLab client");
    assert!(!status.success(), "killed ActingLab client succeeded");

    wait_until(Duration::from_secs(5), || {
        state.taps.load(Ordering::Acquire) == 1 && state.captures.load(Ordering::Acquire) >= 2
    });
    assert!(
        !result_path.exists(),
        "killed client unexpectedly published its local result projection"
    );
    assert!(
        !adb_marker.exists(),
        "ActingLab invoked a local ADB backend"
    );

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("fresh Runtime client");
    let events = wait_for_runtime_task_terminal(&client);
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
            "missing Runtime semantic fact {required}: {facts:#?}"
        );
    }
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::TerminalCommitted {
            outcome: TaskOutcome::Success,
            final_page: Some(page),
            executed_steps: 1,
            failure_code: None,
        } if page == "neutral/terminal"
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskCompleted)
            .count(),
        1
    );

    drop(client);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for condition"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_runtime_task_terminal(
    client: &RuntimeClient,
) -> Vec<actingcommand_contract::ProjectedEvent> {
    let started = Instant::now();
    loop {
        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query Runtime ledger after ActingLab client kill");
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
            "Runtime task terminal did not become durable after ActingLab client kill"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn task_fact_kind(fact: &TaskSemanticFact) -> &'static str {
    match fact {
        TaskSemanticFact::PackageAdmitted { .. } => "package_admitted",
        TaskSemanticFact::RunStarted => "run_started",
        TaskSemanticFact::EvidenceIndexed { .. } => "evidence_indexed",
        TaskSemanticFact::RecognitionStarted { .. } => "recognition_started",
        TaskSemanticFact::RecognitionCompleted { .. } => "recognition_completed",
        TaskSemanticFact::StepStarted { .. } => "step_started",
        TaskSemanticFact::EffectIntent { .. } => "effect_intent",
        TaskSemanticFact::EffectCompleted { .. } => "effect_completed",
        TaskSemanticFact::StepFinished { .. } => "step_finished",
        TaskSemanticFact::Finalizing { .. } => "finalizing",
        TaskSemanticFact::TerminalCommitted { .. } => "terminal_committed",
        TaskSemanticFact::TerminalRejected { .. } => "terminal_rejected",
    }
}

fn assert_event_order(actual: &[EventType], expected: &[EventType]) {
    let mut cursor = 0;
    for expected_type in expected {
        let offset = actual[cursor..]
            .iter()
            .position(|actual_type| actual_type == expected_type)
            .unwrap_or_else(|| panic!("missing Runtime event {expected_type:?} in {actual:?}"));
        cursor += offset + 1;
    }
}

fn write_runtime_owned_lab_package(path: &Path) {
    write_zip(
        path,
        &[
            (
                "control.json",
                br#"{
                    "schema_version":"Lab-1y.control.v1",
                    "package_id":"neutral.runtime-owned.recovery",
                    "execution_mode":"navigable_route",
                    "game":"neutral",
                    "server":"test",
                    "resolution":{"width":2,"height":2},
                    "entry_task_id":"task",
                    "capture_interval_ms":1,
                    "step_timeout_ms":1,
                    "max_steps":3
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
                    "coordinate_space":{"width":2,"height":2},
                    "defaults":{"timeout_ms":1,"max_attempts":1,"retry_interval_ms":1,"post_wait_freezes_ms":0},
                    "entry_page":"home",
                    "target_page":"terminal",
                    "recovery":{"kind":"return_home","task_id":"return_home"},
                    "max_task_retries":1,
                    "on_exhausted":"pause",
                    "operations":[{
                        "id":"open_terminal",
                        "purpose":"force a sealed recovery suggestion",
                        "from":"home",
                        "to":"terminal",
                        "click":{"kind":"point","x":1,"y":1},
                        "retryable":true,
                        "effect":"navigation_only",
                        "unguarded_trusted_coordinate":true
                    }]
                }"#,
            ),
            (
                "resources/operations/return_home/task.json",
                br#"{
                    "schema_version":"0.6",
                    "task_id":"return_home",
                    "game":"neutral",
                    "server_scope":["test"],
                    "coordinate_space":{"width":2,"height":2},
                    "target_page":"home",
                    "operations":[{
                        "id":"return_home_action",
                        "purpose":"sealed successor fixture",
                        "from":"any",
                        "to":"home",
                        "click":{"kind":"point","x":1,"y":1},
                        "effect":"navigation_only",
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
                    "coordinate_space":{"width":2,"height":2},
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
            (
                "resources/navigation/neutral.test.navigation.json",
                br#"{
                    "schema_version":"0.3",
                    "game":"neutral",
                    "server":"test",
                    "navigation":[{
                        "id":"open_terminal",
                        "from_page":"neutral/home",
                        "to_page":"neutral/terminal",
                        "click":{"kind":"point","x":1,"y":1}
                    }],
                    "destructive_actions":[]
                }"#,
            ),
        ],
    );
}

fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
    let file = File::create(path).expect("zip file");
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in files {
        zip.start_file(*name, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip");
}

fn write_forbidden_adb(root: &Path, marker: &Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let path = root.join("forbidden-adb.cmd");
        fs::write(
            &path,
            format!(
                "@echo off\r\necho invoked>\"{}\"\r\nexit /b 99\r\n",
                marker.display()
            ),
        )
        .expect("write forbidden adb");
        path
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("forbidden-adb");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho invoked > \"{}\"\nexit 99\n",
                marker.display()
            ),
        )
        .expect("write forbidden adb");
        let mut permissions = fs::metadata(&path).expect("adb metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("adb permissions");
        path
    }
}
