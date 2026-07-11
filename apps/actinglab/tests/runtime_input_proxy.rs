// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{IdentifierIssuer, InstanceId};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use serde_json::Value;
use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

#[derive(Default)]
struct FakeState {
    taps: AtomicUsize,
    captures: AtomicUsize,
    closes: AtomicUsize,
}

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

impl InputBackend for FakeBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
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
    instance_id: InstanceId,
    state: Arc<FakeState>,
}

struct FakeCapture {
    state: Arc<FakeState>,
}

impl CaptureBackend for FakeCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        self.state.captures.fetch_add(1, Ordering::AcqRel);
        Frame::from_pixels(
            1,
            1,
            vec![255, 0, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
}

impl ExecutionBackendProvider for FakeProvider {
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "ak.cn")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<sealed-test>"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        assert_eq!(instance_alias, "ak.cn");
        Ok(Box::new(FakeBackend {
            state: Arc::clone(&self.state),
            closed: false,
        }))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        assert_eq!(instance_alias, "ak.cn");
        Ok(Box::new(FakeCapture {
            state: Arc::clone(&self.state),
        }))
    }
}

#[test]
fn production_tap_target_uses_runtime_capture_and_fenced_input() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let resources = root.path().join("resources");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_semantic_resources(&resources);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-drive-test"),
        Arc::new(FakeProvider {
            instance_id,
            state: Arc::clone(&state),
        }),
    )
    .expect("runtime host");

    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args([
            "--json",
            "--instance",
            "ak.cn",
            "--resource-root",
            resources.to_str().expect("resource root"),
            "--game",
            "ark",
            "--server",
            "cn",
            "tap-target",
            "home_button",
            "--capture",
        ])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .output()
        .expect("run actinglab tap-target");

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
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
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
                {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
            ]
        }"#,
    )
    .expect("recognition pack");
    fs::write(
        recognition.join("arknights.cn.pages.json"),
        r#"{"schema_version":"0.3","pages":[]}"#,
    )
    .expect("page set");
    fs::write(
        navigation.join("arknights.cn.navigation.json"),
        r#"{"schema_version":"0.3","game":"arknights","server":"cn","navigation":[],"destructive_actions":[]}"#,
    )
    .expect("navigation graph");
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
            instance_id,
            state: Arc::clone(&state),
        }),
    )
    .expect("runtime host");

    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(["--instance", "ak.cn", "tap", "10", "20"])
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
