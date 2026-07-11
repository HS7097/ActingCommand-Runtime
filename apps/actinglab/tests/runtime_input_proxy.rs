// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{IdentifierIssuer, InstanceId};
use actingcommand_device::{DeviceResult, InputBackend};
use actingcommand_runtime_host::{
    InputBackendProvider, ResolvedInputInstance, RuntimeHost, RuntimeHostConfig,
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

impl InputBackendProvider for FakeProvider {
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedInputInstance> {
        (instance_alias == "ak.cn")
            .then(|| ResolvedInputInstance::new(self.instance_id, "<sealed-test>"))
    }

    fn open(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        assert_eq!(instance_alias, "ak.cn");
        Ok(Box::new(FakeBackend {
            state: Arc::clone(&self.state),
            closed: false,
        }))
    }
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
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
    host.close().expect("close host");
}
