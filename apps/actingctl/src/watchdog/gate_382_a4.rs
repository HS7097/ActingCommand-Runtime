// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-4, test plan v2.2 APPS-G12 (inventory-apps.md key 25, "Tick orchestration"), on
// the APPS-I3 seams (`start::TickSeams`): one whole `run-once --from-task` tick over a real A/B
// root, with a fake machine in place of the CIM probe, the spawn, the started process, the
// owner's health probe and the clock. Every expected value comes from the requirement:
// - contracts/runtime-watchdog.md, "Observation" item 5 (W tried shared; a free lock stays held
//   shared until the tick ends), "Decision" rows 7a and 7f, and "Start" (readiness polled every
//   500 ms for 180 s; an exit of the started process first is `exited_during_startup`; no answer
//   in time is `start_timeout` and the process is left running; the start record is saved before
//   the spawn);
// - rd-1007/v0113/model-374.md §2.3 steps 1 and 6 (:198, :217-220) and row 7a;
// - req-basis-ha.md A-4 G12 (the start record, `outcome=pending`, is on disk before the spawn;
//   the result and `last_tick` are saved at the tick's end);
// - state.json v2 (Workflow #378 A5, model v2.2 §7.5): `starts[].start_result` is `{code,
//   values}`, `watchdog_start_pending` until the start ends.
// The seams offer no way to end a started process, so a start that times out leaves it running
// by construction.

use super::decide::{LiveOwner, READY_TIMEOUT_MS};
use super::observe::HostProbe;
use super::start::{Attempt, StartedProcess, StartedState, Target, TickSeams};
use super::{Report, run_once_with};
use actingcommand_contract::outcome::{Detail, Outcome};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::fs::{self, File, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

// The tick's wall clock when it begins.
const T0: u64 = 1_800_000_000_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;
// The owner the started Runtime answers as.
const STARTED_PID: u32 = 4242;
const STARTED_EPOCH: &str = "epoch_started";

// Row 7f, a start that answers: the owner answers at the third health poll (after one second).
// The start record, `watchdog_start_pending`, was on disk before the spawn, and the free writer
// lock was held shared during the start; at the tick's end the record holds
// `watchdog_started_runtime` with the owner, and `last_tick` and the task tick's end are saved.
#[test]
fn gate_382_a4_g12_an_answering_start_is_started_and_its_record_is_saved_before_the_spawn_and_at_the_end()
 {
    let base = TempDir::new().expect("tempdir");
    let root = install_root(base.path());
    let machine = FakeMachine::new(Behaviour::AnswersAt { poll: 3 });

    let (report, state) = tick(&root, &machine);

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_started_runtime",
        "{}",
        describe(&report)
    );
    assert_eq!(
        report.outcome.value("owner_epoch"),
        Some(&json!(STARTED_EPOCH))
    );
    assert_eq!(report.outcome.value("pid"), Some(&json!(STARTED_PID)));
    assert_start_bookkeeping(&machine, &state, "watchdog_started_runtime");
    assert_eq!(state["starts"][0]["actingd_pid"], json!(STARTED_PID));
    assert_eq!(state["starts"][0]["owner_epoch"], json!(STARTED_EPOCH));
    assert_eq!(state["starts"][0]["method"], json!("breakaway"));
}

// Row 7f, a start whose process exits first: `watchdog_start_failed` caused by
// `exited_during_startup` with the process's exit code, kept as the start's result.
#[test]
fn gate_382_a4_g12_a_start_whose_process_exits_first_is_start_failed_exited_during_startup() {
    let base = TempDir::new().expect("tempdir");
    let root = install_root(base.path());
    let machine = FakeMachine::new(Behaviour::ExitsAt {
        check: 2,
        exit_code: 1,
    });

    let (report, state) = tick(&root, &machine);

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_start_failed",
        "{}",
        describe(&report)
    );
    assert_eq!(
        report.outcome.value("failure_code"),
        Some(&json!("exited_during_startup")),
        "{}",
        describe(&report)
    );
    let failure = cause(&report);
    assert_eq!(failure.code().as_str(), "exited_during_startup");
    assert_eq!(failure.value("exit_code"), Some(&json!(1)));
    assert_start_bookkeeping(&machine, &state, "exited_during_startup");
}

// Row 7f, a start that never answers: after the 180 s readiness window (polled every 500 ms), the
// tick ends `watchdog_start_failed` caused by `start_timeout` with that window, kept as the
// start's result; no owner is recorded for it.
#[test]
fn gate_382_a4_g12_a_start_that_never_answers_is_start_timeout_after_180_s() {
    let base = TempDir::new().expect("tempdir");
    let root = install_root(base.path());
    let machine = FakeMachine::new(Behaviour::Silent);

    let (report, state) = tick(&root, &machine);

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_start_failed",
        "{}",
        describe(&report)
    );
    assert_eq!(
        report.outcome.value("failure_code"),
        Some(&json!("start_timeout")),
        "{}",
        describe(&report)
    );
    let failure = cause(&report);
    assert_eq!(failure.code().as_str(), "start_timeout");
    assert_eq!(failure.value("timeout_ms"), Some(&json!(READY_TIMEOUT_MS)));
    let window = Duration::from_millis(READY_TIMEOUT_MS);
    assert!(
        machine.elapsed.get() >= window
            && machine.elapsed.get() < window + Duration::from_millis(500),
        "the readiness wait ended after {:?}",
        machine.elapsed.get()
    );
    assert_start_bookkeeping(&machine, &state, "start_timeout");
    assert!(state["starts"][0].get("actingd_pid").is_none(), "{state}");
}

// Row 7a: while acsetup holds its writer lock exclusively, the tick is `watchdog_installer_busy`:
// nothing is spawned and no start is recorded, and `last_tick` is saved.
#[test]
fn gate_382_a4_g12_a_held_writer_lock_is_installer_busy_and_nothing_starts() {
    let base = TempDir::new().expect("tempdir");
    let root = install_root(base.path());
    let acsetup = File::options()
        .read(true)
        .write(true)
        .open(root.join("install").join("writer.lock"))
        .expect("open acsetup's writer lock");
    acsetup.lock().expect("acsetup holds its writer lock");
    let machine = FakeMachine::new(Behaviour::AnswersAt { poll: 1 });

    let (report, state) = tick(&root, &machine);
    drop(acsetup);

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_installer_busy",
        "{}",
        describe(&report)
    );
    assert!(machine.spawns.borrow().is_empty(), "nothing is spawned");
    assert_eq!(state["starts"], json!([]), "{state}");
    assert_eq!(state["last_tick_unix_ms"], json!(T0));
}

// --- the tick ---

// Runs one task tick on `machine` and reads the state it saved.
fn tick(root: &Path, machine: &FakeMachine) -> (Report, Value) {
    let report = run_once_with(machine, root, true);
    let state = fs::read(root.join("watchdog").join("state.json")).unwrap_or_else(|error| {
        panic!("state.json after the tick: {error}; {}", describe(&report))
    });
    let state = serde_json::from_slice(&state).expect("state.json is JSON");
    (report, state)
}

// The start's bookkeeping of row 7f: at the spawn, the start record was saved pending and the
// free writer lock was held shared (an exclusive lock was refused); at the tick's end the record
// holds `result`, and `last_tick` and the task tick's end are saved.
fn assert_start_bookkeeping(machine: &FakeMachine, state: &Value, result: &str) {
    assert_eq!(
        *machine.spawns.borrow(),
        [Spawn {
            from_task: true,
            saved_result: Some("watchdog_start_pending".to_owned()),
            writer_exclusive_refused: true,
        }]
    );
    let starts = state["starts"].as_array().expect("the start records");
    assert_eq!(starts.len(), 1, "{state}");
    assert_eq!(starts[0]["at_unix_ms"], json!(T0));
    assert_eq!(starts[0]["start_result"]["code"], json!(result), "{state}");
    assert_eq!(state["last_tick_unix_ms"], json!(T0));
    assert_eq!(
        state["last_task_tick_end_unix_ms"],
        json!(machine.now_unix_ms())
    );
}

// The start's failure: the decision's `caused_by` link.
fn cause(report: &Report) -> &Outcome {
    report
        .outcome
        .causes()
        .first()
        .map(|link| link.outcome())
        .unwrap_or_else(|| panic!("the failure is linked: {}", describe(report)))
}

fn describe(report: &Report) -> String {
    report
        .line(Detail::Full)
        .map_or_else(|error| error.to_string(), |line| line.to_string())
}

// An A/B root with slot A and generation 1 whose zero-instance config names `<root>\state`: its
// fixed entry exists, acsetup's writer lock is free, and the owner journal's last record is
// active while nothing holds the owner lock (the Runtime crashed), so the tick considers a start.
fn install_root(base: &Path) -> PathBuf {
    let root = base.join("install");
    let generation = root.join("install").join("generations").join("1");
    for directory in [
        root.join("A"),
        generation.clone(),
        root.join("state"),
        root.join("runtime"),
    ] {
        fs::create_dir_all(directory).expect("create a fixture directory");
    }
    let members = br#"{"fixture":"watchdog-tick"}"#;
    fs::write(root.join("A").join("MEMBERS.json"), members).expect("write the members");
    let config = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": root.join("state"),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "watchdog-tick-test-salt",
        "instances": []
    }))
    .expect("the config JSON");
    fs::write(generation.join("actingd.config.json"), &config).expect("write the config");
    fs::write(root.join("install").join("slot-A.lock"), b"").expect("write the slot lock");
    let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let selection = json!({
        "schema_version": "actingcommand.install-selection.v1",
        "slot": "A",
        "generation": 1,
        "members": { "path": "A/MEMBERS.json", "sha256": digest(members) },
        "config": {
            "path": "install/generations/1/actingd.config.json",
            "sha256": digest(&config)
        }
    });
    fs::write(
        root.join("install").join("active.json"),
        serde_json::to_vec(&selection).expect("the selection JSON"),
    )
    .expect("write the selection");
    fs::write(root.join("runtime").join("actingcommand-actingd.exe"), b"")
        .expect("write the fixed entry");
    fs::write(root.join("install").join("writer.lock"), b"").expect("write the writer lock");
    let record = json!({
        "schema_version": "actingcommand.runtime-owner.v2",
        "revision": 1,
        "owner_epoch": "epoch_crashed",
        "pid": 100,
        "started_at_unix_ms": T0 - HOUR_MS,
        "active": true,
        "active_instances": [],
        "closed_at_unix_ms": null,
        "resource_disposition": "in_use"
    });
    fs::write(root.join("state").join("owner.lock"), format!("{record}\n"))
        .expect("write the owner journal");
    root
}

// --- the fake machine (APPS-I3) ---

#[derive(Debug, Clone, Copy)]
enum Behaviour {
    // The started process exits with `exit_code` at its `check`-th state check.
    ExitsAt { check: u32, exit_code: i32 },
    // The started owner answers from the `poll`-th health poll on.
    AnswersAt { poll: u32 },
    // The started process runs on and never answers.
    Silent,
}

// What the machine saw at a spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spawn {
    from_task: bool,
    // The result of the newest start record in state.json when the spawn came.
    saved_result: Option<String>,
    // Whether an exclusive lock on acsetup's writer lock was refused at the spawn.
    writer_exclusive_refused: bool,
}

// A machine with no Runtime process under the root, booted a day before the tick. Its clock
// moves only when the readiness wait pauses.
struct FakeMachine {
    behaviour: Behaviour,
    wall_unix_ms: Cell<u64>,
    elapsed: Cell<Duration>,
    health_polls: Cell<u32>,
    spawns: RefCell<Vec<Spawn>>,
}

impl FakeMachine {
    fn new(behaviour: Behaviour) -> Self {
        Self {
            behaviour,
            wall_unix_ms: Cell::new(T0),
            elapsed: Cell::new(Duration::ZERO),
            health_polls: Cell::new(0),
            spawns: RefCell::new(Vec::new()),
        }
    }
}

impl TickSeams for FakeMachine {
    fn now_unix_ms(&self) -> u64 {
        self.wall_unix_ms.get()
    }

    fn monotonic(&self) -> Duration {
        self.elapsed.get()
    }

    fn pause(&self, interval: Duration) {
        self.elapsed.set(self.elapsed.get() + interval);
        let interval_ms = u64::try_from(interval.as_millis()).expect("a poll interval");
        self.wall_unix_ms.set(self.wall_unix_ms.get() + interval_ms);
    }

    fn host_probe(&self, _root_plain: &Path) -> Result<HostProbe, Outcome> {
        Ok(HostProbe {
            processes: Vec::new(),
            boot_unix_ms: Some(T0 - DAY_MS),
        })
    }

    fn spawn(
        &self,
        target: &Target<'_>,
        _entry: &Path,
        log_file: File,
        log: &Path,
        from_task: bool,
    ) -> Result<(&'static str, Box<dyn StartedProcess>), Attempt> {
        drop(log_file);
        let saved_result = log
            .parent()
            .and_then(|directory| fs::read(directory.join("state.json")).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|state| {
                state["starts"].as_array()?.last()?["start_result"]["code"]
                    .as_str()
                    .map(str::to_owned)
            });
        let writer = File::open(target.root_plain.join("install").join("writer.lock"))
            .expect("open acsetup's writer lock");
        let writer_exclusive_refused = matches!(writer.try_lock(), Err(TryLockError::WouldBlock));
        drop(writer);
        self.spawns.borrow_mut().push(Spawn {
            from_task,
            saved_result,
            writer_exclusive_refused,
        });
        let process: Box<dyn StartedProcess> = Box::new(FakeProcess {
            behaviour: self.behaviour,
            checks: 0,
        });
        Ok((if from_task { "breakaway" } else { "wmi" }, process))
    }

    fn answering_owner(&self, _state_root: &Path) -> Option<LiveOwner> {
        let poll = self.health_polls.get() + 1;
        self.health_polls.set(poll);
        match self.behaviour {
            Behaviour::AnswersAt { poll: from } if poll >= from => Some(LiveOwner {
                pid: STARTED_PID,
                owner_epoch: STARTED_EPOCH.to_owned(),
                // The owner started after the spawn.
                started_at_unix_ms: T0 + 500,
            }),
            Behaviour::AnswersAt { .. } | Behaviour::ExitsAt { .. } | Behaviour::Silent => None,
        }
    }
}

// The started process, as the readiness wait sees it.
struct FakeProcess {
    behaviour: Behaviour,
    checks: u32,
}

impl StartedProcess for FakeProcess {
    fn state(&mut self) -> Result<StartedState, Outcome> {
        self.checks += 1;
        Ok(match self.behaviour {
            Behaviour::ExitsAt { check, exit_code } if self.checks >= check => {
                StartedState::Exited(Some(exit_code))
            }
            Behaviour::ExitsAt { .. } | Behaviour::AnswersAt { .. } | Behaviour::Silent => {
                StartedState::Running
            }
        })
    }
}
