// SPDX-License-Identifier: AGPL-3.0-only

// #503 HALT-1 (0.12.87; register §1.530; contracts/runtime-watchdog.md, "Observation" item 5,
// "Decision" rows 7b and 7f, "Files"): the watchdog tries again. On production (11-27) one tick
// could not read its process probe within 60 s and started nothing, and the 73 ticks after it
// started nothing and wrote nothing: each probe failed the same way, and the log-on-change kept
// every repeat out of `watchdog.log`. Now P has a second way that uses no WMI (the operating
// system's process list), a tick is `process_probe_failed` only when both ways fail, and every
// tick whose probe fails writes its line.
//
// Whole `run-once --from-task` ticks over the real A/B root of `gate_382_a4` (its owner journal's
// last record is active while nothing holds the owner lock, so a tick considers a start), on a
// scripted machine: each tick's probe is a pair of answers, the first way's and the second
// way's, put together by the product's own `observe::probe_with_second_way`; each spawn exits at
// once or answers at the first health poll. The machine's clock moves only when the test moves it
// (one minute a tick, a task tick's period) or a readiness poll pauses. No clock is read and
// nothing sleeps.

use super::decide::LiveOwner;
use super::gate_382_a4::install_root;
use super::observe::{self, HostProbe};
use super::start::{Attempt, StartedProcess, StartedState, Target, TickSeams};
use super::{Report, run_once_with};
use crate::codes::ActingctlCode;
use crate::process_probe::NamedProcess;
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

// The first tick's wall clock.
const T0: u64 = 1_800_000_000_000;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
// A task tick's period.
const TICK_MS: u64 = 60_000;
// A gap after which a task tick opens the gap grace (contracts/runtime-watchdog.md, "Gap grace":
// more than 90 s).
const GAP_MS: u64 = 5 * 60 * 1000;
const STARTED_PID: u32 = 4242;
const STARTED_EPOCH: &str = "epoch_started";
const START_FAILED_LINE: &str = " ERROR watchdog_start_failed ";
const STARTED_LINE: &str = " WARN watchdog_started_runtime ";

// --- the tests ---

// Ruling B1 (the second way), on the product's composition alone: a first way that fails is
// followed by the process list. A process with the daemon's name whose path cannot be read and
// one under the root are Runtime processes of this root; one under another root is not; the
// second way reads no boot time. An empty list is no process. Both ways failing is the first
// way's failure, `process_probe_failed` with its `timeout_ms`, with the second way's as its
// `secondary` link. A first way that answers is the answer, and the list is not taken.
#[test]
fn halt1_b1_a_first_way_that_fails_is_followed_by_the_process_list_and_both_failing_fails() {
    let base = TempDir::new().expect("tempdir");
    let root = base.path().join("install");
    let own = format!(r"{}\runtime\actingcommand-actingd.exe", root.display());
    let listed = vec![
        named(7, None),
        named(8, Some(r"C:\Other\runtime\actingcommand-actingd.exe")),
        named(9, Some(own.as_str())),
    ];

    let seen = observe::probe_with_second_way(Err(probe_timed_out()), || Ok(listed), &root)
        .expect("the second way answers");

    let pids = seen
        .processes
        .iter()
        .map(|process| process.pid)
        .collect::<Vec<_>>();
    assert_eq!(
        pids,
        [7, 9],
        "the processes of this root: {:?}",
        seen.processes
    );
    assert_eq!(seen.processes[0].path, None);
    assert_eq!(seen.processes[1].path.as_deref(), Some(own.as_str()));
    assert_eq!(
        seen.boot_unix_ms, None,
        "the process list reads no boot time"
    );

    let none = observe::probe_with_second_way(Err(probe_timed_out()), || Ok(Vec::new()), &root)
        .expect("the second way answers");
    assert!(none.processes.is_empty());

    let Err(failed) =
        observe::probe_with_second_way(Err(probe_timed_out()), || Err(list_failed()), &root)
    else {
        panic!("both ways fail, so the probe fails");
    };
    assert_eq!(failed.code().as_str(), "process_probe_failed");
    assert_eq!(failed.value("timeout_ms"), Some(&serde_json::json!(60_000)));
    let secondary = failed
        .causes()
        .iter()
        .filter(|link| *link.relation() == CauseRelation::Secondary)
        .map(|link| link.outcome().code().as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(secondary, ["process_probe_failed"], "{failed:?}");

    let first = observe::probe_with_second_way(
        Ok(HostProbe {
            processes: Vec::new(),
            boot_unix_ms: Some(T0 - DAY_MS),
        }),
        || panic!("the process list is not taken when the first way answers"),
        &root,
    )
    .expect("the first way answers");
    assert_eq!(first.boot_unix_ms, Some(T0 - DAY_MS));
}

// Ruling B1 on a whole tick: the first way fails and the process list shows a process with the
// daemon's name whose path cannot be read. It counts as running under the root: the tick is
// `watchdog_runtime_process_present` and nothing is spawned.
#[test]
fn halt1_b1_a_daemon_the_second_way_sees_is_running_and_nothing_starts_beside_it() {
    let base = TempDir::new().expect("tempdir");
    let root = install_root(base.path());
    let machine = ScriptedMachine::new([(Err(probe_timed_out()), Ok(vec![named(77, None)]))], []);

    let report = run_once_with(&machine, &root, true);

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_runtime_process_present",
        "{}",
        describe(&report)
    );
    assert_eq!(machine.spawns.get(), 0, "nothing is spawned");
}

// Ruling 3 and B2: a tick that cannot probe (both ways fail), then a second one that cannot
// either, then one that can and sees no Runtime. The first two are `watchdog_start_failed` caused
// by `process_probe_failed` and spawn nothing, and each writes its ERROR line (until 0.12.87 the
// second wrote none); the third starts the Runtime and writes its WARN line.
#[test]
fn halt1_b2_every_tick_that_cannot_probe_writes_its_line_and_the_first_that_can_starts() {
    let base = TempDir::new().expect("tempdir");
    let root = install_root(base.path());
    let machine = ScriptedMachine::new(
        [
            (Err(probe_timed_out()), Err(list_failed())),
            (Err(probe_timed_out()), Err(list_failed())),
            (Ok(no_runtime()), Err(list_failed())),
        ],
        [Spawned::Answers],
    );

    let first = run_once_with(&machine, &root, true);
    assert_probe_failed(&first);
    assert_eq!(log_lines(&root, START_FAILED_LINE), 1);
    machine.next_tick(TICK_MS);
    let second = run_once_with(&machine, &root, true);
    assert_probe_failed(&second);
    assert_eq!(
        log_lines(&root, START_FAILED_LINE),
        2,
        "the second tick that could not probe writes its line too"
    );
    assert_eq!(
        machine.spawns.get(),
        0,
        "nothing is spawned while P is unknown"
    );
    machine.next_tick(TICK_MS);
    let third = run_once_with(&machine, &root, true);

    assert_eq!(
        third.outcome.code().as_str(),
        "watchdog_started_runtime",
        "{}",
        describe(&third)
    );
    assert_eq!(machine.spawns.get(), 1);
    assert_eq!(log_lines(&root, STARTED_LINE), 1);
}

// Ruling 3: the same after a start that failed (its process exited first): the next tick, which
// sees no Runtime, starts it.
#[test]
fn halt1_after_a_start_that_failed_the_next_tick_that_sees_no_runtime_starts_it() {
    let base = TempDir::new().expect("tempdir");
    let root = install_root(base.path());
    let machine = ScriptedMachine::new(
        [
            (Ok(no_runtime()), Err(list_failed())),
            (Ok(no_runtime()), Err(list_failed())),
        ],
        [Spawned::Exits, Spawned::Answers],
    );

    let first = run_once_with(&machine, &root, true);
    assert_eq!(
        first.outcome.code().as_str(),
        "watchdog_start_failed",
        "{}",
        describe(&first)
    );
    assert_eq!(
        first.outcome.value("failure_code"),
        Some(&serde_json::json!("exited_during_startup")),
        "{}",
        describe(&first)
    );
    machine.next_tick(TICK_MS);
    let second = run_once_with(&machine, &root, true);

    assert_eq!(
        second.outcome.code().as_str(),
        "watchdog_started_runtime",
        "{}",
        describe(&second)
    );
    assert_eq!(machine.spawns.get(), 2);
}

// Ruling 3: what forbids a start still forbids it after a tick that could not probe. The
// installer's writer lock held exclusively is `watchdog_installer_busy`; a task tick after a gap
// of five minutes, whose probe then answers with no Runtime, is `watchdog_grace`. Nothing is
// spawned.
#[test]
fn halt1_after_a_tick_that_could_not_probe_a_hold_or_the_grace_still_forbids_a_start() {
    let base = TempDir::new().expect("tempdir");
    let root = install_root(base.path());
    let machine = ScriptedMachine::new(
        [
            (Err(probe_timed_out()), Err(list_failed())),
            (Ok(no_runtime()), Err(list_failed())),
        ],
        [],
    );

    assert_probe_failed(&run_once_with(&machine, &root, true));
    machine.next_tick(TICK_MS);
    let installer = File::options()
        .read(true)
        .write(true)
        .open(root.join("install").join("writer.lock"))
        .expect("open the installer's writer lock");
    installer
        .lock()
        .expect("the installer holds its writer lock");
    let held = run_once_with(&machine, &root, true);
    drop(installer);
    assert_eq!(
        held.outcome.code().as_str(),
        "watchdog_installer_busy",
        "{}",
        describe(&held)
    );
    machine.next_tick(GAP_MS);
    let after_gap = run_once_with(&machine, &root, true);

    assert_eq!(
        after_gap.outcome.code().as_str(),
        "watchdog_grace",
        "{}",
        describe(&after_gap)
    );
    assert_eq!(machine.spawns.get(), 0, "nothing is spawned");
}

// --- helpers ---

fn named(pid: u32, image_path: Option<&str>) -> NamedProcess {
    NamedProcess {
        pid,
        image_path: image_path.map(str::to_owned),
    }
}

// The first way's failure as the production tick of 11-27 met it.
fn probe_timed_out() -> Outcome {
    Outcome::new(ActingctlCode::ProcessProbeFailed).timeout_ms(Duration::from_secs(60))
}

fn list_failed() -> Outcome {
    Outcome::new(ActingctlCode::ProcessProbeFailed)
}

// The first way's answer on a machine with no Runtime process, booted a day before.
fn no_runtime() -> HostProbe {
    HostProbe {
        processes: Vec::new(),
        boot_unix_ms: Some(T0 - DAY_MS),
    }
}

fn assert_probe_failed(report: &Report) {
    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_start_failed",
        "{}",
        describe(report)
    );
    assert_eq!(
        report.outcome.value("failure_code"),
        Some(&serde_json::json!("process_probe_failed")),
        "{}",
        describe(report)
    );
}

// The lines of `watchdog.log` that hold `marker`.
fn log_lines(root: &Path, marker: &str) -> usize {
    let log = fs::read_to_string(root.join("watchdog").join("watchdog.log")).unwrap_or_default();
    log.lines().filter(|line| line.contains(marker)).count()
}

fn describe(report: &Report) -> String {
    report
        .line(actingcommand_contract::outcome::Detail::Full)
        .map_or_else(|error| error.to_string(), |line| line.to_string())
}

// --- the scripted machine ---

// How a spawned process behaves.
#[derive(Debug, Clone, Copy)]
enum Spawned {
    // It exits at its first state check.
    Exits,
    // Its owner answers at the first health poll.
    Answers,
}

// A probe as a tick reads it: the first way's answer and the second way's.
type ScriptedProbe = (
    Result<HostProbe, Outcome>,
    Result<Vec<NamedProcess>, Outcome>,
);

struct ScriptedMachine {
    wall_unix_ms: Cell<u64>,
    elapsed: Cell<Duration>,
    probes: RefCell<VecDeque<ScriptedProbe>>,
    spawned: RefCell<VecDeque<Spawned>>,
    spawns: Cell<u32>,
    answering: Cell<bool>,
}

impl ScriptedMachine {
    fn new(
        probes: impl IntoIterator<Item = ScriptedProbe>,
        spawned: impl IntoIterator<Item = Spawned>,
    ) -> Self {
        Self {
            wall_unix_ms: Cell::new(T0),
            elapsed: Cell::new(Duration::ZERO),
            probes: RefCell::new(probes.into_iter().collect()),
            spawned: RefCell::new(spawned.into_iter().collect()),
            spawns: Cell::new(0),
            answering: Cell::new(false),
        }
    }

    // The next task tick, `after_ms` after this one.
    fn next_tick(&self, after_ms: u64) {
        self.wall_unix_ms.set(self.wall_unix_ms.get() + after_ms);
        self.answering.set(false);
    }
}

impl TickSeams for ScriptedMachine {
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

    fn host_probe(&self, root_plain: &Path) -> Result<HostProbe, Outcome> {
        let (first, second) = self
            .probes
            .borrow_mut()
            .pop_front()
            .expect("a scripted probe for this tick");
        observe::probe_with_second_way(first, move || second, root_plain)
    }

    fn spawn(
        &self,
        _target: &Target<'_>,
        _entry: &Path,
        log_file: File,
        _log: &Path,
        from_task: bool,
    ) -> Result<(&'static str, Box<dyn StartedProcess>), Attempt> {
        drop(log_file);
        self.spawns.set(self.spawns.get() + 1);
        let spawned = self
            .spawned
            .borrow_mut()
            .pop_front()
            .expect("a scripted spawn");
        self.answering.set(matches!(spawned, Spawned::Answers));
        let process: Box<dyn StartedProcess> = Box::new(ScriptedProcess { spawned });
        Ok((if from_task { "breakaway" } else { "wmi" }, process))
    }

    fn answering_owner(&self, _state_root: &Path) -> Option<LiveOwner> {
        self.answering.get().then(|| LiveOwner {
            pid: STARTED_PID,
            owner_epoch: STARTED_EPOCH.to_owned(),
            // The owner started after the spawn.
            started_at_unix_ms: self.wall_unix_ms.get() + 500,
        })
    }
}

struct ScriptedProcess {
    spawned: Spawned,
}

impl StartedProcess for ScriptedProcess {
    fn state(&mut self) -> Result<StartedState, Outcome> {
        Ok(match self.spawned {
            Spawned::Exits => StartedState::Exited(Some(1)),
            Spawned::Answers => StartedState::Running,
        })
    }
}
