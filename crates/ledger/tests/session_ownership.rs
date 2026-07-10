// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_ledger::{LabLedger, SessionHeader};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const CHILD_ROOT_ENV: &str = "ACTINGCOMMAND_LEDGER_OWNER_TEST_ROOT";
const CHILD_READY_ENV: &str = "ACTINGCOMMAND_LEDGER_OWNER_TEST_READY";
const CHILD_RELEASE_ENV: &str = "ACTINGCOMMAND_LEDGER_OWNER_TEST_RELEASE";
const SESSION_NAME: &str = "shared-runtime-session";

fn header() -> SessionHeader {
    SessionHeader::new("runtime", "arknights", "cn", "ak")
}

fn owner_path(root: &Path) -> PathBuf {
    root.join("sessions")
        .join(SESSION_NAME)
        .join("ledger.owner.json")
}

fn spawn_owner(root: &Path, ready: &Path, release: &Path) -> Child {
    Command::new(env::current_exe().expect("current test binary"))
        .args(["--exact", "ledger_owner_child_process", "--nocapture"])
        .env(CHILD_ROOT_ENV, root)
        .env(CHILD_READY_ENV, ready)
        .env(CHILD_RELEASE_ENV, release)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ledger owner")
}

fn wait_for_ready(ready: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if ready.is_file() {
            return;
        }
        if let Some(status) = child.try_wait().expect("child status") {
            panic!("ledger owner exited before ready: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for ledger owner");
}

#[test]
fn ledger_owner_child_process() {
    let Some(root) = env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let ready = PathBuf::from(env::var_os(CHILD_READY_ENV).expect("ready path"));
    let release = PathBuf::from(env::var_os(CHILD_RELEASE_ENV).expect("release path"));
    let ledger = LabLedger::open_or_create(&root, SESSION_NAME, header()).expect("own ledger");
    fs::write(&ready, b"ready").expect("write ready");
    while !release.exists() {
        thread::sleep(Duration::from_millis(10));
    }
    std::hint::black_box(ledger.ledger_path());
}

#[test]
fn another_live_process_cannot_open_the_same_runtime_ledger_session() {
    let temp = TempDir::new().expect("tempdir");
    let ready = temp.path().join("ready");
    let release = temp.path().join("release");
    let mut owner = spawn_owner(temp.path(), &ready, &release);
    wait_for_ready(&ready, &mut owner);

    let second = LabLedger::open_or_create(temp.path(), SESSION_NAME, header());
    fs::write(&release, b"release").expect("release owner");
    assert!(owner.wait().expect("owner exit").success());
    let error = match second {
        Ok(_) => panic!("second live process must be rejected"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("runtime ledger session is owned by live process"),
        "{error}"
    );
}

#[test]
fn stale_runtime_ledger_owner_is_recovered_after_process_death() {
    let temp = TempDir::new().expect("tempdir");
    let ready = temp.path().join("ready");
    let release = temp.path().join("release");
    let mut owner = spawn_owner(temp.path(), &ready, &release);
    wait_for_ready(&ready, &mut owner);
    let owner_record_exists = owner_path(temp.path()).is_file();

    owner.kill().expect("hard kill owner");
    owner.wait().expect("reap owner");
    assert!(owner_record_exists, "owner process must publish ownership");
    let ledger = LabLedger::open_or_create(temp.path(), SESSION_NAME, header())
        .expect("stale owner should be recovered");

    assert_eq!(ledger.ledger_path().file_name().unwrap(), "ledger.jsonl");
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(owner_path(temp.path())).expect("owner metadata"))
            .expect("owner metadata JSON");
    assert_eq!(
        metadata
            .get("owner_pid")
            .and_then(serde_json::Value::as_u64),
        Some(u64::from(std::process::id()))
    );
}
