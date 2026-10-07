// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #374: `actingctl watchdog` on an A/B fixture root. A live owner (the sealed C4
//! Runtime child), a held acsetup writer lock, start attempts through a stand-in fixed entry
//! and the budget, a FATAL hold, a formal start and close; the real start path with the
//! actingd that CI exports as `ACTINGCOMMAND_TEST_ACTINGD_EXE`; and the task's registration
//! and removal. Without a registered task `status` adds attention 14.

#![cfg(windows)]

#[allow(dead_code)]
#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_pack_containment::Sha256Hash;
use serde_json::{Value, json};
use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tempfile::TempDir;

const ENTRY: &str = "actingcommand-actingd.exe";

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

/// `<base>\install` is an A/B root with slot A, generation 1 and a zero-instance config whose
/// state root is `<root>\state`.
struct Fixture {
    _temp: TempDir,
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    config: PathBuf,
    runs: Cell<u32>,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("tempdir");
        let base = temp.path().to_path_buf();
        let root = base.join("install");
        let state = root.join("state");
        let generation = root.join("install").join("generations").join("1");
        for directory in [
            root.join("A"),
            generation.clone(),
            state.clone(),
            root.join("runtime"),
        ] {
            fs::create_dir_all(directory).expect("create fixture directory");
        }
        let members = br#"{"fixture":"watchdog"}"#;
        fs::write(root.join("A").join("MEMBERS.json"), members).expect("write members");
        let config = generation.join("actingd.config.json");
        let config_bytes = serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": state,
            "bind_host": "127.0.0.1",
            "bind_port": 0,
            "secret_fingerprint_salt": "watchdog-process-test-salt",
            "instances": []
        }))
        .expect("config JSON");
        fs::write(&config, &config_bytes).expect("write config");
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
        Self {
            _temp: temp,
            base,
            root,
            state,
            config,
            runs: Cell::new(0),
        }
    }

    /// Runs `actingctl watchdog <arguments> --root <root>` with stdout and stderr in files, so
    /// a Runtime the tick starts holds no pipe of this test.
    fn watchdog(&self, arguments: &[&str]) -> (i32, Value) {
        let run = self.runs.get() + 1;
        self.runs.set(run);
        let stdout = self.base.join(format!("watchdog-{run}.out"));
        let stderr = self.base.join(format!("watchdog-{run}.err"));
        let status = Command::new(env!("CARGO_BIN_EXE_actingctl"))
            .arg("watchdog")
            .args(arguments)
            .arg("--root")
            .arg(&self.root)
            .stdin(Stdio::null())
            .stdout(File::create(&stdout).expect("stdout file"))
            .stderr(File::create(&stderr).expect("stderr file"))
            .status()
            .expect("run actingctl watchdog");
        let output = fs::read_to_string(&stdout).expect("read stdout");
        let report = serde_json::from_str(output.trim()).unwrap_or_else(|error| {
            panic!(
                "watchdog {arguments:?} printed no report ({error}): {output}; stderr: {}",
                fs::read_to_string(&stderr).unwrap_or_default()
            )
        });
        (status.code().expect("exit code"), report)
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("watchdog").join("watchdog.log"))
            .expect("read watchdog log")
    }

    fn start_logs(&self) -> usize {
        fs::read_dir(self.root.join("watchdog"))
            .expect("read watchdog directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("actingd-") && name.ends_with(".log")
            })
            .count()
    }

    fn wait_alive(&self) {
        let started = Instant::now();
        loop {
            let (_, report) = self.watchdog(&["status"]);
            if report["decision"] == "alive" {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "the Runtime did not come up: {report}"
            );
            thread::sleep(Duration::from_millis(500));
        }
    }
}

fn attention(report: &Value) -> Vec<String> {
    report["attention"]
        .as_array()
        .expect("attention list")
        .iter()
        .map(|item| item["code"].as_str().expect("attention code").to_owned())
        .collect()
}

fn assert_decision(result: (i32, Value), exit: i32, decision: &str) -> Value {
    let (code, report) = result;
    assert_eq!(
        (code, report["decision"].as_str()),
        (exit, Some(decision)),
        "{report}"
    );
    report
}

#[test]
fn watchdog_follows_a_live_owner_a_kill_the_budget_a_fatal_and_a_formal_close() {
    let fixture = Fixture::new();
    // A stand-in fixed entry that exits at once without taking the owner lock.
    fs::copy(
        env!("CARGO_BIN_EXE_actingctl"),
        fixture.root.join("runtime").join(ENTRY),
    )
    .expect("place the stand-in entry");
    let mut runtime = support::RuntimeChild::spawn(&fixture.state, "c4_runtime_child_process");
    runtime.wait_ready(&fixture.state);

    let report = assert_decision(fixture.watchdog(&["status"]), 14, "alive");
    assert_eq!(report["detail"]["started_by_watchdog"], false);
    assert_eq!(attention(&report), ["task_missing"]);
    assert_decision(fixture.watchdog(&["run-once"]), 0, "alive");
    assert!(
        fixture
            .log()
            .contains(" INFO watchdog_formal_start_observed ")
    );

    // An unexpected end while acsetup holds its writer lock: the watchdog stands aside.
    let writer = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.root.join("install").join("writer.lock"))
        .expect("open writer lock");
    writer.try_lock().expect("hold the writer lock");
    drop(runtime);
    let report = assert_decision(fixture.watchdog(&["run-once"]), 0, "installer_busy");
    assert_eq!(report["journal"]["active"], true);
    drop(writer);

    // Three start attempts through the stand-in entry fail at once; then the budget holds.
    for attempt in 1..=3 {
        let report = assert_decision(fixture.watchdog(&["run-once"]), 12, "start_failed");
        assert_eq!(
            report["detail"]["code"], "exited_during_startup",
            "attempt {attempt}"
        );
        assert_eq!(report["last_start"]["method"], "wmi");
    }
    for _ in 0..2 {
        assert_decision(fixture.watchdog(&["run-once"]), 11, "budget_exhausted");
    }
    assert_eq!(fixture.start_logs(), 3);
    assert_decision(fixture.watchdog(&["status"]), 11, "budget_exhausted");

    // A FATAL newer than the journal holds before every start row.
    let fatal = fixture.root.join("actingd-1.log");
    fs::write(&fatal, "FATAL actingd: config: bind_port_invalid\n").expect("write FATAL log");
    let report = assert_decision(fixture.watchdog(&["run-once"]), 10, "fatal_hold");
    assert_eq!(
        report["detail"]["line"],
        "FATAL actingd: config: bind_port_invalid"
    );
    File::options()
        .write(true)
        .open(&fatal)
        .and_then(|file| file.set_modified(SystemTime::now() - Duration::from_secs(3600)))
        .expect("age the FATAL log");

    // A formal start clears the exhaustion; after a formal close the watchdog stays down.
    // The killed owner's runtime-info is still there, so readiness is the watchdog's own
    // `alive`: the new owner holds the lock and answers (review M1); until then it is
    // `owner_lock_held`.
    let mut runtime = support::RuntimeChild::spawn(&fixture.state, "c4_runtime_child_process");
    fixture.wait_alive();
    let report = assert_decision(fixture.watchdog(&["run-once"]), 0, "alive");
    assert_eq!(report["budget"]["exhausted_since_unix_ms"], Value::Null);
    runtime.stop_clean();
    assert_decision(fixture.watchdog(&["run-once"]), 0, "formal_close");
    assert_eq!(fixture.start_logs(), 3);
    // No Runtime log covers the closed epoch: `status` flags it (review M2).
    let report = assert_decision(fixture.watchdog(&["status"]), 14, "formal_close");
    assert_eq!(report["close_evidence"], "unlogged");
    assert_eq!(
        attention(&report),
        ["formal_close_unlogged", "task_missing"]
    );
}

/// Ends a Runtime the test could not stop formally.
struct RuntimeCleanup(PathBuf);

impl Drop for RuntimeCleanup {
    fn drop(&mut self) {
        let Ok(bytes) = fs::read(self.0.join("runtime-info.json")) else {
            return;
        };
        if let Some(pid) = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|info| info["pid"].as_u64())
        {
            let ended = Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
            eprintln!("cleanup taskkill {pid}: {ended:?}");
        }
    }
}

#[test]
fn watchdog_restarts_a_killed_runtime_through_the_fixed_entry() {
    let actingd = std::env::var_os("ACTINGCOMMAND_TEST_ACTINGD_EXE")
        .expect("ACTINGCOMMAND_TEST_ACTINGD_EXE names the actingd executable (CI exports it)");
    let fixture = Fixture::new();
    let entry = fixture.root.join("runtime").join(ENTRY);
    fs::copy(&actingd, &entry).expect("place the fixed entry");
    let _cleanup = RuntimeCleanup(fixture.state.clone());

    // A formal start with a log, as the logon script or acsetup does it.
    let log = File::create(fixture.root.join("actingd-formal.log")).expect("formal log");
    let mut formal = Command::new(&entry)
        .arg("--config")
        .arg(&fixture.config)
        .current_dir(&fixture.root)
        .stdin(Stdio::null())
        .stdout(log.try_clone().expect("formal log handle"))
        .stderr(log)
        .spawn()
        .expect("start actingd formally");
    fixture.wait_alive();

    // An unexpected end, as in Task Manager: one task tick starts it again.
    formal.kill().expect("end actingd");
    formal.wait().expect("wait for actingd");
    let report = assert_decision(fixture.watchdog(&["run-once", "--from-task"]), 0, "started");
    let method = report["detail"]["method"]
        .as_str()
        .expect("start method")
        .to_owned();
    assert!(matches!(method.as_str(), "breakaway" | "wmi"), "{report}");
    let line = fixture
        .log()
        .lines()
        .find(|line| line.contains(" WARN watchdog_started_runtime "))
        .map(str::to_owned)
        .expect("start line");
    assert!(
        line.contains(&format!(" method={method} ")) && line.contains(" generation=1 "),
        "{line}"
    );
    let report = assert_decision(fixture.watchdog(&["status"]), 14, "alive");
    assert_eq!(report["detail"]["started_by_watchdog"], true);
    assert_eq!(attention(&report), ["task_missing"]);

    // A formal close: the watchdog stays down, and its own log covers the closed epoch.
    let shutdown = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .arg("request-shutdown")
        .arg("--state-root")
        .arg(&fixture.state)
        .args(["--wait", "60"])
        .output()
        .expect("request shutdown");
    assert!(
        shutdown.status.success(),
        "request-shutdown failed: {}",
        String::from_utf8_lossy(&shutdown.stderr)
    );
    assert_decision(
        fixture.watchdog(&["run-once", "--from-task"]),
        0,
        "formal_close",
    );
    let report = assert_decision(fixture.watchdog(&["status"]), 14, "formal_close");
    assert_eq!(report["close_evidence"], "logged");
    assert_eq!(attention(&report), ["task_missing"]);
    assert_eq!(fixture.start_logs(), 1);
}

/// Deletes the test's task whatever the test did.
struct TaskCleanup(String);

impl Drop for TaskCleanup {
    fn drop(&mut self) {
        let deleted = Command::new("schtasks")
            .args(["/Delete", "/TN", &self.0, "/F"])
            .output();
        eprintln!("cleanup schtasks /Delete {}: {deleted:?}", self.0);
    }
}

#[test]
fn watchdog_install_registers_the_task_and_uninstall_removes_it() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("tools")).expect("tools directory");
    fs::copy(
        env!("CARGO_BIN_EXE_actingwatch"),
        fixture.root.join("tools").join("actingwatch.exe"),
    )
    .expect("place the launcher");
    fs::copy(
        env!("CARGO_BIN_EXE_actingctl"),
        fixture.root.join("runtime").join("actingctl.exe"),
    )
    .expect("place the fixed entry");

    let report = assert_decision(fixture.watchdog(&["install"]), 0, "installed");
    let name = report["task_name"].as_str().expect("task name").to_owned();
    let _cleanup = TaskCleanup(name.clone());
    assert!(
        name.starts_with("ActingCommand Runtime watchdog "),
        "{name}"
    );
    let canonical = fs::canonicalize(&fixture.root).expect("canonical root");
    let plain = canonical.to_string_lossy();
    let launcher = format!(
        "{}\\tools\\actingwatch.exe",
        plain.strip_prefix("\\\\?\\").unwrap_or(plain.as_ref())
    );
    assert_eq!(
        report["command"].as_str().map(str::to_lowercase),
        Some(launcher.to_lowercase()),
        "{report}"
    );
    assert_eq!(report["interval"], "PT1M");
    assert_eq!(report["logon_type"], "InteractiveToken");
    assert_eq!(report["enabled"], true);
    let xml = fs::read(fixture.root.join("watchdog").join("task.xml")).expect("task.xml");
    assert!(
        xml.starts_with(&[0xff, 0xfe]),
        "task.xml is not UTF-16LE with a BOM"
    );

    // Task Scheduler's own copy of the definition.
    let queried = Command::new("schtasks")
        .args(["/Query", "/TN", &name, "/XML"])
        .output()
        .expect("schtasks /Query");
    assert!(queried.status.success(), "{queried:?}");
    let text = String::from_utf8_lossy(&queried.stdout);
    for setting in [
        "<Interval>PT1M</Interval>",
        "<LogonType>InteractiveToken</LogonType>",
        "<ExecutionTimeLimit>PT5M</ExecutionTimeLimit>",
        "actingwatch.exe</Command>",
    ] {
        assert!(text.contains(setting), "{setting} missing from {text}");
    }
    assert!(!text.contains("<Enabled>false</Enabled>"), "{text}");
    let report = assert_decision(fixture.watchdog(&["status"]), 0, "never_started");
    assert_eq!(report["task"]["registered"], true, "{report}");
    assert_eq!(report["task"]["mismatches"], json!([]), "{report}");
    assert!(
        fixture
            .log()
            .contains(&format!(" INFO watchdog_installed task=\"{name}\" "))
    );

    // Uninstall removes the task and keeps every file; a second one finds none.
    let report = assert_decision(fixture.watchdog(&["uninstall"]), 0, "uninstalled");
    assert_eq!(report["was_registered"], true);
    let queried = Command::new("schtasks")
        .args(["/Query", "/TN", &name])
        .output()
        .expect("schtasks /Query");
    assert!(!queried.status.success(), "the task is still registered");
    let report = assert_decision(fixture.watchdog(&["uninstall"]), 0, "uninstalled");
    assert_eq!(report["was_registered"], false);
    assert!(fixture.root.join("watchdog").join("task.xml").is_file());
    let report = assert_decision(fixture.watchdog(&["status"]), 14, "never_started");
    assert_eq!(attention(&report), ["task_missing"]);
}
