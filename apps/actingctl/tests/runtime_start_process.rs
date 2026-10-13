// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #389 MCP-10 (command table v1.1 §4.11): `actingctl start` and `actingctl restart` on
//! temporary roots. The live Runtime is the sealed C4 Runtime child on the root's state root;
//! the fixed entry is a stand-in (actingctl itself) that exits at once, so a start through it
//! ends before readiness. No real actingd starts.

#![cfg(windows)]

#[allow(dead_code)]
#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_contract::{EventActor, EventSource};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;
use tempfile::TempDir;

const ENTRY: &str = "actingcommand-actingd.exe";

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

/// `<base>\install`: the fixed entry is the stand-in, and the state root is `<root>\state`.
struct Root {
    _temp: TempDir,
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    config: PathBuf,
}

impl Root {
    /// The root with its config at `config` (relative to the root); also the config's bytes.
    fn new(config: &Path) -> (Self, Vec<u8>) {
        let temp = TempDir::new().expect("tempdir");
        let base = temp.path().to_path_buf();
        let root = base.join("install");
        let state = root.join("state");
        let runtime = root.join("runtime");
        let config = root.join(config);
        let config_directory = config.parent().expect("config directory").to_path_buf();
        for directory in [&state, &runtime, &config_directory] {
            fs::create_dir_all(directory).expect("create fixture directory");
        }
        fs::copy(env!("CARGO_BIN_EXE_actingctl"), runtime.join(ENTRY))
            .expect("place the stand-in entry");
        let config_bytes = serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": state,
            "bind_host": "127.0.0.1",
            "bind_port": 0,
            "secret_fingerprint_salt": "runtime-start-process-test-salt",
            "instances": []
        }))
        .expect("config JSON");
        fs::write(&config, &config_bytes).expect("write config");
        let fixture = Self {
            _temp: temp,
            base,
            root,
            state,
            config,
        };
        (fixture, config_bytes)
    }

    /// An A/B root with slot A and generation 1 (the watchdog's fixture layout).
    fn installation() -> Self {
        let generation = Path::new("install").join("generations").join("1");
        let (fixture, config_bytes) = Self::new(&generation.join("actingd.config.json"));
        let root = &fixture.root;
        fs::create_dir_all(root.join("A")).expect("slot directory");
        let members = br#"{"fixture":"runtime-start"}"#;
        fs::write(root.join("A").join("MEMBERS.json"), members).expect("write members");
        fs::write(root.join("install").join("slot-A.lock"), b"").expect("write slot lock");
        fs::write(root.join("install").join("writer.lock"), b"").expect("write writer lock");
        let selection = json!({
            "schema_version": "actingcommand.install-selection.v1",
            "slot": "A",
            "generation": 1,
            "members": {
                "path": "A/MEMBERS.json",
                "sha256": Sha256Hash::digest(members).to_string()
            },
            "config": {
                "path": "install/generations/1/actingd.config.json",
                "sha256": Sha256Hash::digest(&config_bytes).to_string()
            }
        });
        fs::write(
            root.join("install").join("active.json"),
            serde_json::to_vec(&selection).expect("selection JSON"),
        )
        .expect("write selection");
        fixture
    }

    /// A verification root: no selection, a config the command names with `--config`.
    fn verification() -> Self {
        Self::new(&Path::new("config").join("actingd.config.json")).0
    }

    /// Runs actingctl with stdout and stderr in files, so a process it starts holds no pipe of
    /// this test.
    fn actingctl(&self, name: &str, arguments: &[&str]) -> (i32, String, String) {
        let stdout = self.base.join(format!("{name}.out"));
        let stderr = self.base.join(format!("{name}.err"));
        let status = Command::new(env!("CARGO_BIN_EXE_actingctl"))
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(File::create(&stdout).expect("stdout file"))
            .stderr(File::create(&stderr).expect("stderr file"))
            .status()
            .expect("run actingctl");
        (
            status.code().expect("exit code"),
            fs::read_to_string(&stdout).expect("read stdout"),
            fs::read_to_string(&stderr).expect("read stderr"),
        )
    }

    fn text(path: &Path) -> &str {
        path.to_str().expect("UTF-8 fixture path")
    }

    /// The `actingd-*.log` files a start left in the root.
    fn start_logs(&self) -> usize {
        fs::read_dir(&self.root)
            .expect("read the root")
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("actingd-") && name.ends_with(".log")
            })
            .count()
    }

    /// Every path under the root with its length and modification time, sorted.
    fn tree(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut entries = Vec::new();
        let mut pending = vec![self.root.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).expect("read a fixture directory") {
                let path = entry.expect("fixture entry").path();
                let metadata = fs::metadata(&path).expect("fixture metadata");
                if metadata.is_dir() {
                    pending.push(path.clone());
                }
                let modified = metadata.modified().expect("modification time");
                entries.push((path, metadata.len(), modified));
            }
        }
        entries.sort();
        entries
    }
}

/// The developer line of a failed command: `FATAL actingctl: <code>: <detail>`, exit 1.
fn fatal_line(result: &(i32, String, String)) -> &str {
    let (exit, stdout, stderr) = result;
    assert_eq!(*exit, 1, "stdout: {stdout}; stderr: {stderr}");
    assert!(
        stdout.is_empty(),
        "a failure prints nothing on stdout: {stdout}"
    );
    stderr
        .lines()
        .find(|line| line.starts_with("FATAL actingctl: "))
        .unwrap_or_else(|| panic!("no FATAL line: {stderr}"))
}

/// §4.11 `start` step 1: a state root whose owner lock a live process holds is refused with
/// `start_runtime_already_running` (error, value `path` = the state root) and nothing changes:
/// no run lock, no log, the Runtime still runs. After its formal close, steps 2-4 take the
/// watchdog's run lock and start through the watchdog's manual start; the stand-in entry exits
/// before readiness, which is `exited_during_startup`.
#[test]
fn start_refuses_a_live_state_root_and_otherwise_starts_through_the_watchdog_start() {
    let fixture = Root::installation();
    let mut runtime = support::RuntimeChild::spawn(&fixture.state, "c4_runtime_child_process");
    runtime.wait_ready(&fixture.state);
    let root = Root::text(&fixture.root);

    let refused = fixture.actingctl("start-live", &["start", "--root", root]);
    let line = fatal_line(&refused);
    assert!(
        line.starts_with("FATAL actingctl: start_runtime_already_running: "),
        "{line}"
    );
    assert!(
        line.contains(&format!("path={}", fixture.state.display())),
        "{line}"
    );
    assert!(
        !fixture.root.join("watchdog").exists(),
        "a run lock was taken"
    );
    assert_eq!(fixture.start_logs(), 0, "a start log was created");
    runtime.assert_alive();

    runtime.stop_clean();
    let started = fixture.actingctl("start-closed", &["start", "--root", root]);
    let line = fatal_line(&started);
    assert!(
        line.starts_with("FATAL actingctl: exited_during_startup: "),
        "{line}"
    );
    assert!(
        fixture.root.join("watchdog").join("run.lock").is_file(),
        "the watchdog's run lock was not used"
    );
    assert_eq!(fixture.start_logs(), 1);
}

/// §4.11 `restart` step 1: while an instance holds a lease past `--wait-idle`, restart is
/// refused with `start_wait_idle_expired` (error, values `timeout_ms` and `aliases`) and nothing
/// is stopped. The verification-root form: `--config` and a matching `--state-root`.
#[test]
fn restart_refuses_while_an_instance_is_busy_and_stops_nothing() {
    let fixture = Root::verification();
    let mut runtime = support::RuntimeChild::spawn(&fixture.state, "c4_runtime_child_process");
    let before = runtime.wait_ready(&fixture.state);
    let holder = RuntimeClient::connect(RuntimeClientConfig::new(
        &fixture.state,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("lease holder");
    let token = holder.acquire_lease("node.a").expect("hold the instance");

    let refused = fixture.actingctl(
        "restart-busy",
        &[
            "restart",
            "--root",
            Root::text(&fixture.root),
            "--config",
            Root::text(&fixture.config),
            "--state-root",
            Root::text(&fixture.state),
            "--wait-idle",
            "1",
        ],
    );
    let line = fatal_line(&refused);
    assert!(
        line.starts_with("FATAL actingctl: start_wait_idle_expired: "),
        "{line}"
    );
    assert!(line.contains("timeout_ms=1000"), "{line}");
    assert!(line.contains("aliases=node.a"), "{line}");
    runtime.assert_alive();
    assert_eq!(
        holder.health().expect("the Runtime still answers"),
        before.owner_epoch(),
        "the owner changed"
    );
    assert_eq!(fixture.start_logs(), 0, "a start log was created");

    holder.release_lease(&token).expect("release the instance");
    drop(holder);
    runtime.stop_clean();
}

/// Coordinator ruling on MCP-10 (review of deviation 6): `start` refuses while acsetup holds
/// `install\writer.lock`, exactly as the watchdog refuses a start (row 7a), with its
/// `watchdog_installer_busy`. Nothing under the root changes: no run lock, and no start log,
/// which every launch of the fixed entry creates first, so no Runtime was launched.
#[test]
fn start_refuses_while_the_installer_holds_its_writer_lock_and_touches_nothing() {
    let fixture = Root::installation();
    let writer = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.root.join("install").join("writer.lock"))
        .expect("open writer lock");
    writer.try_lock().expect("hold the writer lock");
    let before = fixture.tree();

    let refused = fixture.actingctl(
        "start-installer-busy",
        &["start", "--root", Root::text(&fixture.root)],
    );
    let line = fatal_line(&refused);
    assert!(
        line.starts_with("FATAL actingctl: watchdog_installer_busy: "),
        "{line}"
    );
    assert_eq!(fixture.tree(), before, "the refused start changed the root");
    assert!(
        !fixture.root.join("watchdog").exists(),
        "a run lock was taken"
    );
    assert_eq!(fixture.start_logs(), 0, "a Runtime was launched");
    drop(writer);
}
