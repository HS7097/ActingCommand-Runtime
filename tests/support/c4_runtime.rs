// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{IdentifierIssuer, InstanceId, RUNTIME_INFO_FILE, RuntimeInfo};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
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

const CHILD_MODE_ENV: &str = "ACTINGCOMMAND_C4_TEST_CHILD";
const CHILD_ROOT_ENV: &str = "ACTINGCOMMAND_C4_TEST_ROOT";
const CHILD_INSTANCE_ENV: &str = "ACTINGCOMMAND_C4_TEST_INSTANCE";
const CHILD_STOP_ENV: &str = "ACTINGCOMMAND_C4_TEST_STOP";
const BACKEND_EVENTS_FILE: &str = "sealed-c4-backend.log";

struct FileBackend {
    events_path: PathBuf,
    closed: bool,
}

impl FileBackend {
    fn record(&self, event: &str) -> DeviceResult<()> {
        record_event(&self.events_path, event)
    }
}

struct FileCaptureBackend {
    frame_path: PathBuf,
    events_path: PathBuf,
    closed: bool,
}

impl CaptureBackend for FileCaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        record_event(&self.events_path, "capture")?;
        let png = fs::read(&self.frame_path)
            .map_err(|error| DeviceError::fatal(format!("read sealed frame: {error}")))?;
        Frame::from_png(png, CaptureBackendName::AdbScreencap)
    }
}

impl Drop for FileCaptureBackend {
    fn drop(&mut self) {
        if !self.closed {
            self.closed = true;
            record_event(&self.events_path, "capture_close").expect("record sealed capture close");
        }
    }
}

impl InputBackend for FileBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.record("unexpected_tap")
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        self.record("unexpected_long_tap")
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        self.record("unexpected_swipe")
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        self.record("unexpected_key")
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        self.record("unexpected_text")
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
    frame_path: PathBuf,
}

impl ExecutionBackendProvider for FileProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec!["ak.cn".to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "ak.cn")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<sealed-c4-process>"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        if instance_alias != "ak.cn" {
            return Err(DeviceError::fatal("sealed C4 instance mismatch"));
        }
        let backend = FileBackend {
            events_path: self.events_path.clone(),
            closed: false,
        };
        backend.record("open")?;
        Ok(Box::new(backend))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        if instance_alias != "ak.cn" {
            return Err(DeviceError::fatal("sealed C4 instance mismatch"));
        }
        record_event(&self.events_path, "capture_open")?;
        Ok(Box::new(FileCaptureBackend {
            frame_path: self.frame_path.clone(),
            events_path: self.events_path.clone(),
            closed: false,
        }))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: actingcommand_contract::ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "sealed C4 process does not expose application control",
        ))
    }
}

fn record_event(path: &Path, event: &str) -> DeviceResult<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| DeviceError::fatal(format!("open sealed backend journal: {error}")))?;
    writeln!(file, "{event}")
        .map_err(|error| DeviceError::fatal(format!("write sealed backend journal: {error}")))?;
    file.sync_data()
        .map_err(|error| DeviceError::fatal(format!("sync sealed backend journal: {error}")))
}

pub struct RuntimeChild {
    child: Option<Child>,
    stop_path: PathBuf,
}

impl RuntimeChild {
    pub fn spawn(root: &Path, child_test_name: &str) -> Self {
        let stop_path = root.join("stop-runtime");
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        let child = Command::new(env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                child_test_name,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_MODE_ENV, "1")
            .env(CHILD_ROOT_ENV, root)
            .env(
                CHILD_INSTANCE_ENV,
                serde_json::to_string(&instance_id).expect("instance JSON"),
            )
            .env(CHILD_STOP_ENV, &stop_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sealed Runtime process");
        Self {
            child: Some(child),
            stop_path,
        }
    }

    pub fn wait_ready(&mut self, root: &Path) -> RuntimeInfo {
        let started = Instant::now();
        loop {
            if let Ok(encoded) = fs::read(root.join(RUNTIME_INFO_FILE))
                && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&encoded)
                && info.validate().is_ok()
            {
                return info;
            }
            if let Some(status) = self.try_wait().expect("child process state") {
                panic!(
                    "Runtime exited before ready with {status}: {}",
                    self.output()
                );
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "Runtime readiness timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn assert_alive(&mut self) {
        assert!(
            self.try_wait().expect("child process state").is_none(),
            "Runtime exited with client"
        );
    }

    pub fn stop_clean(&mut self) {
        fs::write(&self.stop_path, b"stop").expect("write Runtime stop signal");
        let started = Instant::now();
        loop {
            if let Some(status) = self.try_wait().expect("child process state") {
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
            stdout.read_to_string(&mut output).expect("child stdout");
        }
        if let Some(stderr) = child.stderr.as_mut() {
            stderr.read_to_string(&mut output).expect("child stderr");
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
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn run_child_if_requested() -> bool {
    if env::var_os(CHILD_MODE_ENV).is_none() {
        return false;
    }
    let root = PathBuf::from(env::var_os(CHILD_ROOT_ENV).expect("child state root"));
    let instance_id = serde_json::from_str::<InstanceId>(
        &env::var(CHILD_INSTANCE_ENV).expect("child instance id"),
    )
    .expect("parse child instance id");
    let stop_path = PathBuf::from(env::var_os(CHILD_STOP_ENV).expect("child stop path"));
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&root, b"c4-process-acceptance-salt")
            .with_io_timeout(Duration::from_millis(500))
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 100,
                takeover_cooldown_ms: 200,
                lease_ttl_ms: 10_000,
                ..SchedulerConfig::default()
            }),
        Arc::new(FileProvider {
            instance_id,
            events_path: root.join(BACKEND_EVENTS_FILE),
            frame_path: root.join("sealed.png"),
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
    true
}

pub fn write_sealed_frame(path: &Path) {
    let frame = Frame::from_pixels(
        2,
        1,
        vec![255, 0, 0, 0, 255, 0],
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .expect("sealed frame");
    fs::write(path, frame.encode_png_fast().expect("sealed frame PNG"))
        .expect("write sealed frame");
}

pub fn backend_events(root: &Path) -> Vec<String> {
    match fs::read_to_string(root.join(BACKEND_EVENTS_FILE)) {
        Ok(content) => content.lines().map(str::to_string).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("read backend events: {error}"),
    }
}
