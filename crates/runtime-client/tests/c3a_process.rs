// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    EventActor, EventSource, IdentifierIssuer, InputAction, InstanceId, OwnerEpoch,
    RUNTIME_INFO_FILE, RuntimeErrorCode, RuntimeInfo,
};
use actingcommand_device::{CaptureBackend, DeviceError, DeviceResult, InputBackend};
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

    fn close(&mut self) -> DeviceResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.record("close")
    }
}

struct FileProvider {
    instance_id: InstanceId,
    events_path: PathBuf,
}

impl ExecutionBackendProvider for FileProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec!["ak.cn".to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "ak.cn")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<sealed-process-test>"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        if instance_alias != "ak.cn" {
            return Err(DeviceError::fatal("sealed process-test instance mismatch"));
        }
        let backend = FileBackend {
            events_path: self.events_path.clone(),
            closed: false,
        };
        backend.record("open")?;
        Ok(Box::new(backend))
    }

    fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        Err(DeviceError::fatal(
            "sealed process test does not configure capture",
        ))
    }
}

struct RuntimeChild {
    child: Option<Child>,
    stop_path: PathBuf,
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
        Self {
            child: Some(child),
            stop_path,
        }
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

    fn stop_clean(&mut self) {
        fs::write(&self.stop_path, b"stop").expect("write Runtime stop signal");
        let started = Instant::now();
        loop {
            if let Some(status) = self.try_wait().expect("read child process state") {
                let output = self.output();
                assert!(status.success(), "Runtime clean stop failed: {output}");
                self.child = None;
                return;
            }
            if started.elapsed() >= Duration::from_secs(5) {
                let child = self.child.as_mut().expect("live Runtime child");
                child.kill().expect("kill timed-out Runtime child");
                child.wait().expect("wait timed-out Runtime child");
                self.child = None;
                panic!("Runtime clean stop timed out");
            }
            thread::sleep(Duration::from_millis(10));
        }
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
fn hard_kill_restart_fences_every_old_input_and_enforces_takeover_cooldown() {
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
    let first_status = first_client.status().expect("first Runtime status");
    assert_eq!(first_status.owner_epoch(), first_info.owner_epoch());
    assert_eq!(first_status.instances().len(), 1);
    assert_eq!(first_status.instances()[0].instance_alias(), "ak.cn");
    assert!(!first_status.instances()[0].takeover_cooldown_active());
    let old_token = first_client.acquire_lease("ak.cn").expect("old lease");
    first_client
        .input(&old_token, InputAction::Reset)
        .expect("old Runtime input");
    assert_eq!(backend_events(root.path()), vec!["open", "reset"]);

    first.kill_hard();
    drop(first_client);

    let mut second = RuntimeChild::spawn(root.path(), instance_id, 2);
    let second_info = second.wait_for_runtime_info(root.path(), Some(first_info.owner_epoch()));
    assert_ne!(second_info.owner_epoch(), first_info.owner_epoch());
    let second_client = client(root.path());
    let takeover_status = second_client.status().expect("takeover Runtime status");
    assert_eq!(takeover_status.owner_epoch(), second_info.owner_epoch());
    assert!(takeover_status.instances()[0].takeover_cooldown_active());
    let cooldown = second_client
        .acquire_lease("ak.cn")
        .expect_err("takeover cooldown must reject acquisition");
    let cooldown = cooldown.projection().expect("cooldown projection");
    assert_eq!(cooldown.code, RuntimeErrorCode::LeaseCooldown);
    let retry_after_ms = cooldown.retry_after_ms.expect("cooldown retry delay");

    let before_stale_inputs = backend_events(root.path());
    for action in all_input_actions() {
        let error = second_client
            .input(&old_token, action)
            .expect_err("old token input must be rejected");
        assert_eq!(
            error.projection().expect("stale-token projection").code,
            RuntimeErrorCode::StaleOwnerEpoch
        );
    }
    assert_eq!(backend_events(root.path()), before_stale_inputs);

    thread::sleep(Duration::from_millis(retry_after_ms.saturating_add(100)));
    let new_token = second_client
        .acquire_lease("ak.cn")
        .expect("lease after takeover cooldown");
    let active_status = second_client.status().expect("active Runtime status");
    assert!(active_status.instances()[0].lease_active());
    assert!(!active_status.instances()[0].takeover_cooldown_active());
    second_client
        .input(&new_token, InputAction::Tap { x: 10, y: 20 })
        .expect("new epoch input");
    second_client
        .release_lease(&new_token)
        .expect("new epoch release");
    drop(second_client);
    second.stop_clean();

    assert_eq!(
        backend_events(root.path()),
        vec!["open", "reset", "open", "tap", "close"]
    );
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

fn all_input_actions() -> Vec<InputAction> {
    vec![
        InputAction::Tap { x: 10, y: 20 },
        InputAction::LongTap {
            x: 10,
            y: 20,
            duration_ms: 10,
        },
        InputAction::Swipe {
            x1: 10,
            y1: 20,
            x2: 30,
            y2: 40,
            duration_ms: 10,
        },
        InputAction::Key {
            key: "BACK".to_string(),
        },
        InputAction::Text {
            text: "sealed-stale-input".to_string(),
        },
        InputAction::Reset,
    ]
}
