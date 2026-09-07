// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    EventActor, EventSource, IdentifierIssuer, InputAction, InstanceId, OwnerEpoch,
    RUNTIME_INFO_FILE, RuntimeInfo,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use actingcommand_scheduler::SchedulerConfig;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const CHILD_MODE_ENV: &str = "ACTINGCOMMAND_C3A_TEST_CHILD";
const CHILD_ROOT_ENV: &str = "ACTINGCOMMAND_C3A_TEST_ROOT";
const CHILD_INSTANCE_ENV: &str = "ACTINGCOMMAND_C3A_TEST_INSTANCE";
const CHILD_STOP_ENV: &str = "ACTINGCOMMAND_C3A_TEST_STOP";
const BACKEND_EVENTS_FILE: &str = "sealed-backend-events.log";

struct FileBackend {
    events_path: PathBuf,
    closed: bool,
}

impl FileBackend {
    fn record(&self, event: &str) -> DeviceResult<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events_path)
            .map_err(|error| DeviceError::fatal(format!("open test backend journal: {error}")))?;
        writeln!(file, "{event}")
            .map_err(|error| DeviceError::fatal(format!("write test backend journal: {error}")))?;
        file.sync_data()
            .map_err(|error| DeviceError::fatal(format!("sync test backend journal: {error}")))
    }
}

impl InputBackend for FileBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.record("tap")
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        self.record("long_tap")
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        self.record("swipe")
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        self.record("key")
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        self.record("text")
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.record("reset")
    }

    fn close_once(
        &mut self,
        _authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        if self.closed {
            return Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
                0,
            ));
        }
        self.closed = true;
        self.record("close")?;
        Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
            1,
        ))
    }
}

struct FileProvider {
    instance_id: InstanceId,
    events_path: PathBuf,
}

struct FileCapture;

impl CaptureBackend for FileCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        Frame::from_pixels(
            1,
            1,
            vec![0, 0, 0, 255],
            PixelFormat::Rgba8,
            CaptureBackendName::AdbScreencap,
        )
    }
    fn close_once(
        &mut self,
        _authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
            0,
        ))
    }
}

impl ExecutionBackendProvider for FileProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec!["node.a".to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "node.a")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<sealed-process-test>"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        if instance_alias != "node.a" {
            return Err(DeviceError::fatal("sealed process-test instance mismatch"));
        }
        let backend = FileBackend {
            events_path: self.events_path.clone(),
            closed: false,
        };
        backend.record("open")?;
        Ok(Box::new(backend))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        if instance_alias != "node.a" {
            return Err(DeviceError::fatal("sealed process-test instance mismatch"));
        }
        Ok(Box::new(FileCapture))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: actingcommand_contract::ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "sealed process test does not expose application control",
        ))
    }
}

struct RuntimeChild {
    child: Option<Child>,
}

impl RuntimeChild {
    fn spawn(root: &Path, instance_id: InstanceId, generation: u8) -> Self {
        let stop_path = root.join(format!("stop-{generation}"));
        let child = Command::new(env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "c3a_runtime_host_child_process",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_MODE_ENV, "1")
            .env(CHILD_ROOT_ENV, root)
            .env(
                CHILD_INSTANCE_ENV,
                serde_json::to_string(&instance_id).expect("instance id JSON"),
            )
            .env(CHILD_STOP_ENV, &stop_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn Runtime test process");
        Self { child: Some(child) }
    }

    fn wait_for_runtime_info(
        &mut self,
        state_root: &Path,
        previous_epoch: Option<OwnerEpoch>,
    ) -> RuntimeInfo {
        let started = Instant::now();
        loop {
            if let Ok(encoded) = fs::read(state_root.join(RUNTIME_INFO_FILE))
                && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&encoded)
                && info.validate().is_ok()
                && previous_epoch.is_none_or(|previous| info.owner_epoch() != previous)
            {
                return info;
            }
            if let Some(status) = self.try_wait().expect("read child process state") {
                panic!(
                    "Runtime test process exited before ready with {status}: {}",
                    self.output()
                );
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "Runtime test process readiness timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill_hard(&mut self) {
        let child = self.child.as_mut().expect("live Runtime child");
        child.kill().expect("hard-kill Runtime child");
        let status = child.wait().expect("wait for hard-killed Runtime child");
        assert!(
            !status.success(),
            "hard-killed Runtime unexpectedly succeeded"
        );
        self.child = None;
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.as_mut().expect("live Runtime child").try_wait()
    }

    fn output(&mut self) -> String {
        let child = self.child.as_mut().expect("Runtime child");
        let mut output = String::new();
        if let Some(stdout) = child.stdout.as_mut() {
            stdout
                .read_to_string(&mut output)
                .expect("read child stdout");
        }
        if let Some(stderr) = child.stderr.as_mut() {
            stderr
                .read_to_string(&mut output)
                .expect("read child stderr");
        }
        output
    }
}

impl Drop for RuntimeChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _kill_result = child.kill();
            let _wait_result = child.wait();
        }
    }
}

#[test]
fn c3a_runtime_host_child_process() {
    if env::var_os(CHILD_MODE_ENV).is_none() {
        return;
    }
    let root = PathBuf::from(env::var_os(CHILD_ROOT_ENV).expect("child state root"));
    let instance_id = serde_json::from_str::<InstanceId>(
        &env::var(CHILD_INSTANCE_ENV).expect("child instance id"),
    )
    .expect("parse child instance id");
    let stop_path = PathBuf::from(env::var_os(CHILD_STOP_ENV).expect("child stop path"));
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&root, b"c3a-process-acceptance-salt")
            .with_io_timeout(Duration::from_millis(500))
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 100,
                takeover_cooldown_ms: 1_000,
                lease_ttl_ms: 10_000,
                ..SchedulerConfig::default()
            }),
        Arc::new(FileProvider {
            instance_id,
            events_path: root.join(BACKEND_EVENTS_FILE),
        }),
    )
    .expect("start child Runtime host");
    loop {
        if stop_path.is_file() {
            break;
        }
        if let Some(error) = host.fatal_error().expect("child Runtime health") {
            panic!("child Runtime became fatal: {error}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    host.close().expect("close child Runtime host");
}

#[test]
fn hard_kill_with_unconfirmed_owner_rejects_automatic_restart() {
    let root = TempDir::new().expect("tempdir");
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let mut first = RuntimeChild::spawn(root.path(), instance_id, 1);
    let first_info = first.wait_for_runtime_info(root.path(), None);
    let first_client = client(root.path());
    assert_eq!(
        first_client.health().expect("first Runtime health"),
        first_info.owner_epoch()
    );
    let first_runtime_info =
        fs::read(root.path().join(RUNTIME_INFO_FILE)).expect("read first runtime-info.json");
    let old_token = first_client.acquire_lease("node.a").expect("old lease");
    first_client
        .input(&old_token, InputAction::Reset)
        .expect("old Runtime input");
    assert_eq!(backend_events(root.path()), vec!["open", "reset"]);

    first.kill_hard();
    drop(first_client);

    let mut second = RuntimeChild::spawn(root.path(), instance_id, 2);
    let started = Instant::now();
    let second_status = loop {
        if let Some(status) = second.try_wait().expect("read second child process state") {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "second Runtime rejection timed out"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        !second_status.success(),
        "second Runtime unexpectedly accepted unconfirmed resources"
    );
    let second_output = second.output();
    assert!(
        second_output.contains("owner_resource_unconfirmed"),
        "second Runtime output omitted owner_resource_unconfirmed: {second_output}"
    );
    assert!(
        second_output.contains("acquire_owner_file"),
        "second Runtime output omitted acquire_owner_file: {second_output}"
    );

    let retained_runtime_info =
        fs::read(root.path().join(RUNTIME_INFO_FILE)).expect("read retained runtime-info.json");
    assert_eq!(retained_runtime_info, first_runtime_info);
    let retained_info =
        serde_json::from_slice::<RuntimeInfo>(&retained_runtime_info).expect("parse runtime info");
    assert_eq!(retained_info.owner_epoch(), first_info.owner_epoch());
    assert_eq!(backend_events(root.path()), vec!["open", "reset"]);
}

fn client(state_root: &Path) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(state_root, EventActor::Cli, EventSource::Cli)
            .with_io_timeout(Duration::from_millis(500))
            .with_backend_open_timeout(Duration::from_secs(2)),
    )
    .expect("connect Runtime client")
}

fn backend_events(state_root: &Path) -> Vec<String> {
    fs::read_to_string(state_root.join(BACKEND_EVENTS_FILE))
        .expect("read backend events")
        .lines()
        .map(str::to_string)
        .collect()
}
