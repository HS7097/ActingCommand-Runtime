// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{EventActor, EventSource, IdentifierIssuer, RUNTIME_INFO_FILE};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _kill_result = self.0.kill();
            let _wait_result = self.0.wait();
        }
    }
}

#[test]
fn actingd_outlives_disposable_clients_and_accepts_reconnection() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    write_config(&config_path, root.path());
    let child = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start actingd");
    let mut child = ChildGuard(child);
    wait_for_runtime_info(&mut child.0, root.path());

    let first = connect(root.path());
    let owner_epoch = first.health().expect("first client health");
    drop(first);

    let second = connect(root.path());
    assert_eq!(second.health().expect("second client health"), owner_epoch);
    assert!(child.0.try_wait().expect("process state").is_none());
    drop(second);

    child.0.kill().expect("kill actingd");
    assert!(!child.0.wait().expect("wait actingd").success());
}

#[test]
fn invalid_startup_returns_nonzero() {
    let output = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", "missing-actingd-config.json"])
        .output()
        .expect("run actingd");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("FATAL actingd"));
}

fn connect(state_root: &Path) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(state_root, EventActor::Cli, EventSource::Cli)
            .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("connect runtime")
}

fn write_config(path: &Path, state_root: &Path) {
    let instance_id = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id");
    let value = json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "actingd-process-test-salt",
        "instances": [{
            "alias": "node.a",
            "instance_id": instance_id.transport(),
            "application_id": "neutral.application",
            "adb_path": "adb",
            "touch_backend": "maatouch",
            "capture_backend": "adb",
            "push_touch_tool": false
        }]
    });
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("config json"),
    )
    .expect("write config");
}

fn wait_for_runtime_info(child: &mut Child, state_root: &Path) {
    let started = Instant::now();
    loop {
        if state_root.join(RUNTIME_INFO_FILE).is_file() {
            return;
        }
        if let Some(status) = child.try_wait().expect("process state") {
            panic!("actingd exited before ready with {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "actingd readiness timed out"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
