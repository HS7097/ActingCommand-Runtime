// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 WD-1 (deploy plan Q1 option (b); Alice 2026-10-16; register §1.96; apps gate 70):
// the watchdog supervises a single-location root, one without `install\active.json` whose
// `runtime\BUILD-MANIFEST.json` is a file and whose configuration is `<root>\actingd.config.json`.
// Every expected value comes from the slice's requirement lines and
// contracts/runtime-watchdog.md ("Layout", "Observation" item 5, "Decision" rows 2, 6, 7a, 7a′
// and 7f, "Start", "Task", "Exit codes"):
// - req. 1: an A/B selection wins; otherwise a root with `runtime\BUILD-MANIFEST.json` is single,
//   configured by `<root>\actingd.config.json` and its absolute `state_root`; a root with neither
//   stays `watchdog_layout_unknown`;
// - req. 2: `install\active.json` appearing before the start is `watchdog_selection_changed`;
// - req. 3: the decisions are unchanged; a start runs `<root>\runtime\actingcommand-actingd.exe
//   --config <root>\actingd.config.json` in `<root>`, and its record names no generation;
// - req. 4: report v2 carries `layout` and `config`, with null `generation` and `slot`;
// - req. 5: `install` on a single root passes the layout precondition and keeps the launcher's;
// - req. 6: `actingctl start` / `restart` without `--config` still need an A/B selection;
// - coordinator decision 5 of 2026-10-18: a deploy that holds `install\writer.lock` exclusively
//   while it replaces `runtime\` is row 7a: nothing starts.
// The machine is a fake on the APPS-I3 seams (`start::TickSeams`), as in `gate_382_a4`; a live
// owner is a Runtime host in this process. Nothing here reads `status()`, which runs the real CIM
// probe and the task query.

use super::decide::{self, LiveOwner, Stage};
use super::observe::{self, HostProbe, Installation, Layout};
use super::start::{Attempt, StartedProcess, StartedState, Target, TickSeams};
use super::state::WatchdogState;
use super::{Report, Tick, install, run_once_with};
use crate::held_runtime::{HELD_SALT, NoInstances};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_runtime_host::{RuntimeHost, RuntimeHostConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

// The tick's wall clock when it begins.
const T0: u64 = 1_800_000_000_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;
// The owner the started Runtime answers as.
const STARTED_PID: u32 = 4242;
const STARTED_EPOCH: &str = "epoch_started";
// `actingctl start`'s developer hint for a root without a selection.
const NO_SELECTION_HINT: &str = "pass --config <file> to start a root without one";

// Req. 1 and 4: the root resolves as single, with `<root>\actingd.config.json` and the state root
// that config names; the report of a tick on it says `layout` single and that `config`, and its
// `generation` and `slot` are null.
#[test]
fn wd1_a_single_location_root_resolves_with_its_root_config_and_state_root() {
    let single = SingleRoot::new();
    let config = single.plain.join("actingd.config.json");

    let installation = Installation::resolve(&single.root)
        .unwrap_or_else(|failure| panic!("the single-location root resolves: {failure}"));
    assert!(matches!(installation.layout, Layout::Single));
    assert_eq!(installation.layout_name(), "single");
    assert_eq!(installation.config_plain, config);
    assert_eq!(installation.state_root, single.state);
    assert_eq!(installation.root_plain, single.plain);
    assert_eq!(
        (installation.generation(), installation.slot()),
        (None, None)
    );

    // No owner journal yet: the tick decides without a start, and its report names the layout.
    let machine = FakeMachine::new();
    let (report, _) = tick(&single.root, &machine);
    let line = report.line(Detail::Full).expect("the report line");
    assert_eq!(line["outcome"]["code"], "watchdog_never_started", "{line}");
    assert_eq!(line["layout"], "single", "{line}");
    assert_eq!(line["config"], json!(text(&config)), "{line}");
    assert_eq!(line["root"], json!(text(&single.plain)), "{line}");
    assert_eq!(line["state_root"], json!(text(&single.state)), "{line}");
    for member in ["generation", "slot"] {
        assert_eq!(line.get(member), Some(&Value::Null), "{member}: {line}");
    }
}

// Req. 3, row 7f: the journal's last record is active and nothing holds the owner lock (a crash).
// The tick starts the fixed entry with `--config <root>\actingd.config.json` in `<root>`, the
// started owner answers, and the start record and its log line name no generation.
#[test]
fn wd1_a_crashed_single_location_runtime_is_started_with_its_root_config() {
    let single = SingleRoot::new();
    single.owner_journal(Owner::Crashed);
    let machine = FakeMachine::new();

    let (report, state) = tick(&single.root, &machine);

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
    assert_eq!(
        *machine.spawns.borrow(),
        [Spawn {
            entry: single
                .plain
                .join("runtime")
                .join("actingcommand-actingd.exe"),
            config: single.plain.join("actingd.config.json"),
            working_directory: single.plain.clone(),
            from_task: true,
        }]
    );
    let starts = state["starts"].as_array().expect("the start records");
    assert_eq!(starts.len(), 1, "{state}");
    assert_eq!(
        starts[0]["start_result"]["code"],
        json!("watchdog_started_runtime"),
        "{state}"
    );
    assert!(starts[0].get("generation").is_none(), "{state}");
    let log = fs::read_to_string(single.root.join("watchdog").join("watchdog.log"))
        .expect("the watchdog log");
    let started = log
        .lines()
        .find(|line| line.contains(" WARN watchdog_started_runtime "))
        .unwrap_or_else(|| panic!("the start's log line: {log}"));
    assert!(!started.contains(" generation="), "{started}");
}

// Req. 3, row 6: the journal's last record is a close (a formal close no log covers, and no
// earlier owner): `watchdog_formal_close`, and nothing is spawned or recorded.
#[test]
fn wd1_a_formally_closed_single_location_runtime_is_not_started() {
    let single = SingleRoot::new();
    single.owner_journal(Owner::Closed);
    let machine = FakeMachine::new();

    let (report, state) = tick(&single.root, &machine);

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_formal_close",
        "{}",
        describe(&report)
    );
    assert!(machine.spawns.borrow().is_empty(), "nothing is spawned");
    assert_eq!(state["starts"], json!([]), "{state}");
}

// Req. 3, row 2: a Runtime holds the owner lock of the config's state root and answers:
// `watchdog_alive`, and nothing is spawned or recorded.
#[test]
fn wd1_a_live_single_location_runtime_is_left_alone() {
    let single = SingleRoot::new();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(single.state.clone(), HELD_SALT),
        Arc::new(NoInstances),
    )
    .expect("a Runtime starts on the root's state root");
    let machine = FakeMachine::new();

    let (report, state) = tick(&single.root, &machine);
    host.close().expect("the Runtime closes");

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_alive",
        "{}",
        describe(&report)
    );
    assert!(machine.spawns.borrow().is_empty(), "nothing is spawned");
    assert_eq!(state["starts"], json!([]), "{state}");
}

// Req. 2, row 7a′: the tick observed a single-location root and considers a start; before the
// start an A/B selection appears in `install\active.json` (an installer converted the root):
// `watchdog_selection_changed`, the same as a changed A/B selection.
#[test]
fn wd1_active_json_appearing_before_the_start_is_a_selection_change() {
    let single = SingleRoot::new();
    single.owner_journal(Owner::Crashed);
    let machine = FakeMachine::new();
    let state = WatchdogState::default();
    let mut tick = Tick::observe(&single.root, &single.root.join("watchdog"))
        .unwrap_or_else(|failure| panic!("the tick observes the root: {failure}"));
    assert!(
        matches!(
            decide::decide_observed(&tick.observed(), &state),
            Stage::ConsiderStart
        ),
        "the crashed Runtime's start is considered"
    );

    single.add_ab_selection(&single.state);
    let (probes, guard) = tick
        .probe_start(&machine)
        .unwrap_or_else(|decision| panic!("the start probes: {decision:?}"));
    drop(guard);

    let decision = decide::decide_start(&probes, &state, T0, true);
    assert_eq!(
        decision.outcome().code().as_str(),
        "watchdog_selection_changed",
        "{decision:?}"
    );
    assert!(machine.spawns.borrow().is_empty(), "nothing is spawned");
}

// Req. 5 (contract "Task", preconditions): `install` on a single-location root passes the layout
// precondition and refuses on the next one, the missing Tools launcher, with
// `watchdog_launcher_missing` before it creates `<root>\watchdog\` or reaches schtasks.
#[test]
fn wd1_install_on_a_single_location_root_checks_the_launcher_not_the_layout() {
    let single = SingleRoot::new();
    // The Runtime's fixed entry is deployed; the Tools that carry the launcher are not.
    fs::write(single.root.join("runtime").join("actingctl.exe"), b"")
        .expect("write the Runtime's fixed entry");

    let report = install(&single.root);

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_launcher_missing",
        "{}",
        describe(&report)
    );
    assert_eq!(
        report.outcome.value("path"),
        Some(&json!(text(
            &single.plain.join("tools").join("actingwatch.exe")
        )))
    );
    assert_eq!(report.exit_code(), 1);
    assert!(
        !single.root.join("watchdog").exists(),
        "install refused before it created the watchdog directory or registered a task"
    );
}

// Req. 1: a root with neither marker (no `install\active.json`, no `runtime\BUILD-MANIFEST.json`,
// though it holds a root config) stays `watchdog_layout_unknown` with its `path`; a root with both
// markers is A/B: the selection's generation, slot, config and state root, not the root config's.
#[test]
fn wd1_a_root_with_neither_marker_stays_layout_unknown_and_ab_wins_over_single() {
    let temp = TempDir::new().expect("tempdir");
    let neither = temp.path().join("neither");
    fs::create_dir_all(neither.join("runtime")).expect("create the runtime directory");
    write_config(
        &neither.join("actingd.config.json"),
        &temp.path().join("neither-state"),
    );
    let failure = Installation::resolve(&neither)
        .err()
        .expect("a root with neither marker is refused");
    assert_eq!(failure.code().as_str(), "watchdog_layout_unknown");
    assert_eq!(failure.value("path"), Some(&json!(text(&plain(&neither)))));

    let both = SingleRoot::new();
    let selected_state = both.base().join("selected-state");
    fs::create_dir_all(&selected_state).expect("create the selected state root");
    both.add_ab_selection(&selected_state);
    let installation = Installation::resolve(&both.root)
        .unwrap_or_else(|failure| panic!("the root with both markers resolves: {failure}"));
    assert_eq!(installation.layout_name(), "ab");
    assert_eq!(
        (installation.generation(), installation.slot()),
        (Some(1), Some("A"))
    );
    assert_eq!(
        installation.config_plain,
        both.plain
            .join("install")
            .join("generations")
            .join("1")
            .join("actingd.config.json")
    );
    assert_eq!(installation.state_root, selected_state);
}

// Req. 6: `actingctl start` (and `restart`) without `--config` on a single-location root keep
// today's refusal: `watchdog_layout_unknown` with the root as `path` and the hint to pass
// `--config`.
#[test]
fn wd1_start_without_config_still_needs_an_ab_selection() {
    let single = SingleRoot::new();

    let failed = crate::runtime_start::ab_installation(&single.root)
        .err()
        .expect("start without --config refuses a single-location root");

    assert_eq!(failed.outcome.code().as_str(), "watchdog_layout_unknown");
    assert_eq!(
        failed.outcome.value("path"),
        Some(&json!(text(&single.plain)))
    );
    assert_eq!(failed.text.as_deref(), Some(NO_SELECTION_HINT));
}

// Coordinator decision 5 of 2026-10-18 (contract "Observation" item 5, row 7a): a deploy script
// that holds `install\writer.lock` exclusively while it replaces `runtime\` of a single-location
// root keeps the tick from starting a crashed Runtime: `watchdog_installer_busy`, nothing spawned.
#[test]
fn wd1_a_deploy_holding_the_writer_lock_of_a_single_location_root_starts_nothing() {
    let single = SingleRoot::new();
    single.owner_journal(Owner::Crashed);
    fs::create_dir_all(single.root.join("install")).expect("create the install directory");
    let deploy = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(single.root.join("install").join("writer.lock"))
        .expect("open the writer lock");
    deploy.lock().expect("the deploy holds the writer lock");
    let machine = FakeMachine::new();

    let (report, state) = tick(&single.root, &machine);
    drop(deploy);

    assert_eq!(
        report.outcome.code().as_str(),
        "watchdog_installer_busy",
        "{}",
        describe(&report)
    );
    assert!(machine.spawns.borrow().is_empty(), "nothing is spawned");
    assert_eq!(state["starts"], json!([]), "{state}");
}

// --- the single-location root ---

// A single-location root: `runtime\BUILD-MANIFEST.json`, a stand-in fixed entry and
// `actingd.config.json` naming an absolute state root outside the root; no `install\`.
struct SingleRoot {
    temp: TempDir,
    root: PathBuf,
    // The plain spelling of the canonical root, as the watchdog hands it to children.
    plain: PathBuf,
    state: PathBuf,
}

#[derive(Debug, Clone, Copy)]
enum Owner {
    // The last record is active: the Runtime crashed.
    Crashed,
    // The owner's start record and its close record.
    Closed,
}

impl SingleRoot {
    fn new() -> Self {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().join("single");
        let state = temp.path().join("single-state");
        for directory in [root.join("runtime"), state.clone()] {
            fs::create_dir_all(directory).expect("create a fixture directory");
        }
        fs::write(
            root.join("runtime").join("BUILD-MANIFEST.json"),
            br#"{"fixture":"watchdog-single"}"#,
        )
        .expect("write the build manifest");
        fs::write(root.join("runtime").join("actingcommand-actingd.exe"), b"")
            .expect("write the fixed entry");
        write_config(&root.join("actingd.config.json"), &state);
        let plain = plain(&root);
        Self {
            temp,
            root,
            plain,
            state,
        }
    }

    fn base(&self) -> &Path {
        self.temp.path()
    }

    // The owner journal of the config's state root.
    fn owner_journal(&self, owner: Owner) {
        let record = |revision: u64, active: bool, closed_at: Option<u64>| {
            json!({
                "schema_version": "actingcommand.runtime-owner.v2",
                "revision": revision,
                "owner_epoch": "epoch_single",
                "pid": 100,
                "started_at_unix_ms": T0 - HOUR_MS,
                "active": active,
                "active_instances": [],
                "closed_at_unix_ms": closed_at,
                "resource_disposition": if active { "in_use" } else { "none" },
            })
        };
        let mut journal = format!("{}\n", record(1, true, None));
        if matches!(owner, Owner::Closed) {
            journal.push_str(&format!(
                "{}\n",
                record(2, false, Some(T0 - HOUR_MS + 60_000))
            ));
        }
        fs::write(self.state.join("owner.lock"), journal).expect("write the owner journal");
    }

    // Makes the root an A/B installation too: slot A, generation 1, whose config names
    // `state_root`.
    fn add_ab_selection(&self, state_root: &Path) {
        let generation = self.root.join("install").join("generations").join("1");
        for directory in [self.root.join("A"), generation.clone()] {
            fs::create_dir_all(directory).expect("create an A/B directory");
        }
        let members = br#"{"fixture":"watchdog-single-and-ab"}"#;
        fs::write(self.root.join("A").join("MEMBERS.json"), members).expect("write the members");
        let config = write_config(&generation.join("actingd.config.json"), state_root);
        fs::write(self.root.join("install").join("slot-A.lock"), b"").expect("write the slot lock");
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
            self.root.join("install").join("active.json"),
            serde_json::to_vec(&selection).expect("the selection JSON"),
        )
        .expect("write the selection");
    }
}

// A zero-instance configuration naming `state_root`; returns its bytes.
fn write_config(path: &Path, state_root: &Path) -> Vec<u8> {
    let config = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "instances": []
    }))
    .expect("the config JSON");
    fs::write(path, &config).expect("write the config");
    config
}

// The plain spelling of a fixture path's canonical form.
fn plain(path: &Path) -> PathBuf {
    observe::plain_path(&fs::canonicalize(path).expect("canonicalize a fixture path"))
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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

fn describe(report: &Report) -> String {
    report
        .line(Detail::Full)
        .map_or_else(|error| error.to_string(), |line| line.to_string())
}

// --- the fake machine (APPS-I3) ---

// What the machine was asked to start.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spawn {
    entry: PathBuf,
    config: PathBuf,
    working_directory: PathBuf,
    from_task: bool,
}

// A machine with no Runtime process under the root, booted a day before the tick, whose started
// Runtime answers at once. Its clock moves only when the readiness wait pauses.
struct FakeMachine {
    wall_unix_ms: Cell<u64>,
    elapsed: Cell<Duration>,
    spawns: RefCell<Vec<Spawn>>,
}

impl FakeMachine {
    fn new() -> Self {
        Self {
            wall_unix_ms: Cell::new(T0),
            elapsed: Cell::new(Duration::ZERO),
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
        entry: &Path,
        log_file: File,
        _log: &Path,
        from_task: bool,
    ) -> Result<(&'static str, Box<dyn StartedProcess>), Attempt> {
        drop(log_file);
        self.spawns.borrow_mut().push(Spawn {
            entry: entry.to_path_buf(),
            config: target.config_plain.to_path_buf(),
            working_directory: target.root_plain.to_path_buf(),
            from_task,
        });
        let process: Box<dyn StartedProcess> = Box::new(Running);
        Ok((if from_task { "breakaway" } else { "wmi" }, process))
    }

    fn answering_owner(&self, _state_root: &Path) -> Option<LiveOwner> {
        // The started Runtime answers once it was spawned, as an owner that started after it.
        (!self.spawns.borrow().is_empty()).then(|| LiveOwner {
            pid: STARTED_PID,
            owner_epoch: STARTED_EPOCH.to_owned(),
            started_at_unix_ms: T0 + 500,
        })
    }
}

// The started process runs on.
struct Running;

impl StartedProcess for Running {
    fn state(&mut self) -> Result<StartedState, Outcome> {
        Ok(StartedState::Running)
    }
}
