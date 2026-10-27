// SPDX-License-Identifier: AGPL-3.0-only

// Test-only: the neutral builds, release folders and roots of the INST-1 gates, and the fake host
// they run `actingctl install` on. Every name is neutral; no file is a real program.

use super::host::{Host, Installer, NotRan, Ran, RunningImage};
use super::manifest::{Speaks, parse_declaration};
use super::runtime::{
    Closed, ConfigSeen, Drained, InstanceSeen, PauseSeen, PlanSeen, RuntimeLink, RuntimeSeen,
};
use super::{Answer, execute};
use crate::codes::ActingctlCode;
use crate::watchdog::decide::LiveOwner;
use crate::watchdog::start::{StartedProcess, StartedState, Target};
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub(super) const C1: &str = "1111111111111111111111111111111111111111";
pub(super) const C2: &str = "2222222222222222222222222222222222222222";
pub(super) const REPOSITORY: &str = "neutral/runtime";
pub(super) const TAG: &str = "v0.0.1-test";

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The interfaces both manifests of a neutral build declare: `actingd-config` up to revision 17,
/// whose check-config takes `--install-root` (INST-1b-F fix round 1).
pub(super) fn speaks() -> Value {
    json!({
        "actingd-config": [5, 17],
        "install-selection": [1, 1],
        "install-control": [0, 1],
        "ledger": [3, 4],
        "runtime-client": [2, 2],
        "package": [3, 5],
        "runtime-outcomes": [1, 1]
    })
}

/// One half of a neutral build: its manifest members and its files.
#[derive(Clone)]
pub(super) struct HalfSpec {
    pub(super) layout_key: &'static str,
    pub(super) layout: String,
    pub(super) repository: String,
    pub(super) commit: String,
    pub(super) speaks: Value,
    /// The listed files.
    pub(super) files: Vec<(String, Vec<u8>)>,
    /// Files the zip or folder carries that the manifest does not list.
    pub(super) unlisted: Vec<(String, Vec<u8>)>,
    /// Listed files written with other bytes than the manifest's.
    pub(super) altered: Vec<String>,
}

impl HalfSpec {
    pub(super) fn runtime(commit: &str) -> Self {
        Self {
            layout_key: "runtime_payload_layout",
            layout: "distribution-v1".to_owned(),
            repository: REPOSITORY.to_owned(),
            commit: commit.to_owned(),
            speaks: speaks(),
            files: vec![
                (
                    "actingcommand-actingd.exe".to_owned(),
                    format!("neutral daemon {commit}").into_bytes(),
                ),
                (
                    "actingctl.exe".to_owned(),
                    format!("neutral control {commit}").into_bytes(),
                ),
                (
                    "actingd.defaults.json".to_owned(),
                    br#"{"ledger_observer":{"open_timeout_ms":600000}}"#.to_vec(),
                ),
            ],
            unlisted: Vec::new(),
            altered: Vec::new(),
        }
    }

    pub(super) fn tools(commit: &str) -> Self {
        Self {
            layout_key: "tools_payload_layout",
            layout: "platform-tools-v3".to_owned(),
            repository: REPOSITORY.to_owned(),
            commit: commit.to_owned(),
            speaks: speaks(),
            files: vec![
                (
                    "actingledger.exe".to_owned(),
                    format!("neutral ledger {commit}").into_bytes(),
                ),
                ("actingwatch.exe".to_owned(), b"neutral launcher".to_vec()),
                (
                    "platform-tools/adb.exe".to_owned(),
                    b"neutral bridge".to_vec(),
                ),
            ],
            unlisted: Vec::new(),
            altered: Vec::new(),
        }
    }

    pub(super) fn manifest(&self) -> Value {
        let files = self
            .files
            .iter()
            .map(|(path, bytes)| {
                json!({"path": path, "size_bytes": bytes.len(), "sha256": sha256_hex(bytes)})
            })
            .collect::<Vec<_>>();
        let mut manifest = json!({
            "repository": self.repository,
            "commit_sha": self.commit,
            "tree_sha": self.commit,
            "files": files,
            "interfaces": {
                "schema_version": "actingcommand.component-interfaces.v1",
                "speaks": self.speaks,
            },
        });
        manifest[self.layout_key] = Value::String(self.layout.clone());
        manifest
    }

    /// Every entry as written: the manifest, the listed files (altered ones changed) and the
    /// unlisted ones.
    pub(super) fn entries(&self) -> Vec<(String, Vec<u8>)> {
        let mut entries = vec![(
            "BUILD-MANIFEST.json".to_owned(),
            serde_json::to_vec_pretty(&self.manifest()).expect("a manifest"),
        )];
        for (path, bytes) in &self.files {
            let mut bytes = bytes.clone();
            if self.altered.contains(path) {
                bytes.extend_from_slice(b" altered");
            }
            entries.push((path.clone(), bytes));
        }
        entries.extend(self.unlisted.iter().cloned());
        entries
    }

    pub(super) fn write_dir(&self, dir: &Path) {
        for (path, bytes) in self.entries() {
            let target = path
                .split('/')
                .fold(dir.to_path_buf(), |path, part| path.join(part));
            fs::create_dir_all(target.parent().expect("a parent")).expect("create a folder");
            fs::write(&target, bytes).expect("write a file");
        }
    }

    pub(super) fn write_zip(&self, path: &Path) {
        let mut writer = zip::ZipWriter::new(File::create(path).expect("create a zip"));
        for (name, bytes) in self.entries() {
            writer
                .start_file(name, zip::write::FileOptions::default())
                .expect("start a zip entry");
            writer.write_all(&bytes).expect("write a zip entry");
        }
        writer.finish().expect("finish a zip");
    }
}

/// A neutral build: its commit and both halves.
#[derive(Clone)]
pub(super) struct BuildSpec {
    pub(super) commit: String,
    pub(super) runtime: HalfSpec,
    pub(super) tools: HalfSpec,
}

impl BuildSpec {
    pub(super) fn new(commit: &str) -> Self {
        Self {
            commit: commit.to_owned(),
            runtime: HalfSpec::runtime(commit),
            tools: HalfSpec::tools(commit),
        }
    }

    /// Writes `runtime\` and `tools\` below `base`.
    pub(super) fn write_build(&self, base: &Path) {
        self.runtime.write_dir(&base.join("runtime"));
        self.tools.write_dir(&base.join("tools"));
    }

    pub(super) fn runtime_zip_name(&self) -> String {
        format!("actingcommand-runtime-{}.zip", self.commit)
    }

    pub(super) fn tools_zip_name(&self) -> String {
        format!("actingcommand-tools-{}.zip", self.commit)
    }

    /// A release folder: both zips and `SHA256SUMS`.
    pub(super) fn write_release(&self, dir: &Path) {
        fs::create_dir_all(dir).expect("create the release folder");
        self.runtime.write_zip(&dir.join(self.runtime_zip_name()));
        self.tools.write_zip(&dir.join(self.tools_zip_name()));
        let sums = [self.runtime_zip_name(), self.tools_zip_name()]
            .iter()
            .map(|name| {
                let bytes = fs::read(dir.join(name)).expect("read a zip");
                format!("{}  {name}\n", sha256_hex(&bytes))
            })
            .collect::<String>();
        fs::write(dir.join("SHA256SUMS"), sums).expect("write SHA256SUMS");
    }

    /// The release-facts document of a release folder as it is now: its files' digests.
    pub(super) fn facts(&self, dir: &Path) -> Value {
        let assets = [
            self.runtime_zip_name(),
            self.tools_zip_name(),
            "SHA256SUMS".to_owned(),
        ]
        .iter()
        .map(|name| {
            let bytes = fs::read(dir.join(name)).expect("read a release file");
            json!({"name": name, "size": bytes.len(), "digest": format!("sha256:{}", sha256_hex(&bytes))})
        })
        .collect::<Vec<_>>();
        json!({
            "repository": REPOSITORY,
            "tag": TAG,
            "commit": self.commit,
            "prerelease": true,
            "assets": assets,
        })
    }
}

/// Writes a facts document to `path`.
pub(super) fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("write JSON");
}

/// A single-location root whose live build is `build`: `runtime\`, `tools\` and a configuration
/// naming a state root outside the root.
pub(super) fn live_root(base: &Path, name: &str, build: &BuildSpec) -> PathBuf {
    let root = base.join(name);
    build.write_build(&root);
    let state = base.join(format!("{name}-state"));
    fs::create_dir_all(&state).expect("create the state root");
    write_json(
        &root.join("actingd.config.json"),
        &json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": state,
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": []
        }),
    );
    root
}

/// Every file below `dir` with its bytes, by its path relative to `dir` (`/` separators).
pub(super) fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("list a folder") {
            let entry = entry.expect("a folder entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().expect("an entry type").is_dir() {
                pending.push((entry.path(), relative));
            } else {
                files.insert(relative, fs::read(entry.path()).expect("read a file"));
            }
        }
    }
    files
}

/// A host on a fixed clock that moves 1 ms per call, its work folder in the test's temporary
/// folder; it stops an operation after the step `crash_after` names, between the two renames of
/// the swap step `crash_between_renames` names, or before `current.json` is retired when
/// `crash_before_retire` is set, each once, and calls `on_step` after each step first. Its
/// processes are `processes`; its programs answer as `programs` says; its start logs lie in the
/// root itself.
pub(super) struct FakeHost {
    clock: Rc<Cell<u64>>,
    work: PathBuf,
    /// Workflow #421 INST-2: the root's fake Runtime, the available memory and the detached
    /// command lines.
    pub(super) runtime: FakeRuntime,
    pub(super) memory_mb: Cell<u64>,
    pub(super) detached: RefCell<Vec<Vec<String>>>,
    pub(super) crash_after: Cell<Option<usize>>,
    pub(super) crash_between_renames: Cell<Option<usize>>,
    pub(super) crash_before_retire: Cell<bool>,
    pub(super) stopped: RefCell<Vec<(String, usize)>>,
    pub(super) processes: RefCell<Vec<RunningImage>>,
    pub(super) programs: Programs,
    #[allow(clippy::type_complexity)]
    pub(super) on_step: RefCell<Option<Box<dyn Fn(usize)>>>,
    /// Every program run and every step, in order.
    pub(super) log: RefCell<Vec<String>>,
}

/// How the fake programs answer.
pub(super) struct Programs {
    /// The builds (`live`, or a commit) whose check-config refuses the configuration.
    pub(super) refuse_check_config: RefCell<Vec<String>>,
    /// The builds whose check-config refuses `--install-root` as a build before
    /// `actingd-config` revision 17 does: `check_config_usage_invalid`, no value, no report.
    pub(super) refuse_install_root: RefCell<Vec<String>>,
    /// The ledger head the head read and `events` read.
    pub(super) ledger_head: Cell<u64>,
    /// A build whose reader refuses a read that reaches this sequence (every read also reads
    /// the head row, as the real range read does).
    pub(super) unreadable: RefCell<Option<(String, u64)>>,
    /// The exit code of `ledger-maintenance verify`.
    pub(super) verify_exit: Cell<i32>,
    /// The programs that are still running at their deadline, by the word that names what they
    /// do (`check-config`, `ledger-maintenance`, `export`, `events`): `pid` 4242, left running.
    pub(super) stall: RefCell<Vec<&'static str>>,
    /// Every program run, by that word, with the deadline it was given.
    pub(super) deadlines: RefCell<Vec<(String, u64)>>,
}

/// The process id of a fake program left running at its deadline.
pub(super) const STALLED_PID: u32 = 4242;

impl FakeHost {
    pub(super) fn new(work: &Path) -> Self {
        fs::create_dir_all(work).expect("create the work folder");
        let clock = Rc::new(Cell::new(1_800_000_000_000));
        Self {
            runtime: FakeRuntime::new(Rc::clone(&clock)),
            memory_mb: Cell::new(64_000),
            detached: RefCell::new(Vec::new()),
            clock,
            work: work.to_path_buf(),
            crash_after: Cell::new(None),
            crash_between_renames: Cell::new(None),
            crash_before_retire: Cell::new(false),
            stopped: RefCell::new(Vec::new()),
            processes: RefCell::new(Vec::new()),
            programs: Programs {
                refuse_check_config: RefCell::new(Vec::new()),
                refuse_install_root: RefCell::new(Vec::new()),
                ledger_head: Cell::new(100),
                unreadable: RefCell::new(None),
                verify_exit: Cell::new(0),
                stall: RefCell::new(Vec::new()),
                deadlines: RefCell::new(Vec::new()),
            },
            on_step: RefCell::new(None),
            log: RefCell::new(Vec::new()),
        }
    }
}

/// The failure of a crash at a test hook: the operation stops there as if the process died.
fn crash() -> Outcome {
    Outcome::from_io_error(
        &std::io::Error::from(std::io::ErrorKind::Interrupted),
        actingcommand_contract::outcome::IoOp::Write,
    )
}

/// The value after `flag` among `words`, as a number.
fn number_after(words: &[String], flag: &str) -> Option<u64> {
    words
        .iter()
        .position(|word| word == flag)
        .and_then(|index| words.get(index + 1))
        .and_then(|value| value.parse::<u64>().ok())
}

/// The build a program belongs to: the commit after `builds`, else `live`.
fn build_of(program: &Path) -> String {
    let parts = program
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    parts
        .iter()
        .position(|part| part == "builds")
        .and_then(|index| parts.get(index + 1).cloned())
        .unwrap_or_else(|| "live".to_owned())
}

fn ran(exit_code: i32, stdout: Value) -> Ran {
    Ran {
        exit_code: Some(exit_code),
        stdout: serde_json::to_vec(&stdout).expect("JSON"),
        stderr: if exit_code == 0 {
            Vec::new()
        } else {
            b"refused by the neutral program".to_vec()
        },
    }
}

impl Host for FakeHost {
    fn now_unix_ms(&self) -> u64 {
        let now = self.clock.get();
        self.clock.set(now + 1);
        now
    }

    fn installer(&self) -> Installer {
        Installer {
            commit: Some(C2.to_owned()),
            path: "neutral-installer".to_owned(),
        }
    }

    fn installer_speaks(&self) -> Option<Speaks> {
        parse_declaration(&json!({
            "schema_version": "actingcommand.component-interfaces.v1",
            "speaks": speaks(),
        }))
    }

    fn work_dir(&self) -> PathBuf {
        self.work.clone()
    }

    fn processes_under(&self, root: &Path) -> Result<Vec<RunningImage>, Outcome> {
        let mut images = self.processes.borrow().clone();
        // Workflow #421 INST-2: a running fake Runtime is an actingd under the root.
        if self.runtime.running.get() {
            images.push(RunningImage {
                pid: self.runtime.owner.borrow().pid,
                path: format!(
                    "{}\\runtime\\actingcommand-actingd.exe",
                    super::files::plain(root)
                ),
                mcp_serve: false,
            });
        }
        Ok(images)
    }

    fn run_program(
        &self,
        program: &Path,
        arguments: &[OsString],
        timeout_ms: u64,
    ) -> Result<Ran, NotRan> {
        let build = build_of(program);
        let name = program
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let words = arguments
            .iter()
            .map(|word| word.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.log
            .borrow_mut()
            .push(format!("{build} {name} {}", words.join(" ")));
        // What the program does: actingd's first word, actingledger's after `--state-root <s>`.
        let doing = words
            .get(if name == "actingledger.exe" { 2 } else { 0 })
            .cloned()
            .unwrap_or_default();
        self.programs
            .deadlines
            .borrow_mut()
            .push((doing.clone(), timeout_ms));
        if self
            .programs
            .stall
            .borrow()
            .iter()
            .any(|stalled| *stalled == doing)
        {
            return Err(NotRan::StillRunning { pid: STALLED_PID });
        }
        let head = self.programs.ledger_head.get();
        Ok(match (name.as_str(), words.first().map(String::as_str)) {
            ("actingcommand-actingd.exe", Some("check-config"))
                if words.iter().any(|word| word == "--install-root")
                    && self.programs.refuse_install_root.borrow().contains(&build) =>
            {
                ran(
                    1,
                    json!({
                        "schema_version": "actingcommand.actingd.check-config.v2",
                        "outcome": {"schema_version": "actingcommand.outcome.v1",
                            "code": "check_config_usage_invalid", "category": "error",
                            "values": {}},
                    }),
                )
            }
            ("actingcommand-actingd.exe", Some("check-config")) => {
                if self.programs.refuse_check_config.borrow().contains(&build) {
                    ran(
                        1,
                        json!({
                            "schema_version": "actingcommand.actingd.check-config.v2",
                            "outcome": {"schema_version": "actingcommand.outcome.v1",
                                "code": "config_decode_failed", "category": "error", "values": {}},
                        }),
                    )
                } else {
                    ran(
                        0,
                        json!({
                            "schema_version": "actingcommand.actingd.check-config.v2",
                            "outcome": {"schema_version": "actingcommand.outcome.v1",
                                "code": "check_config_passed", "category": "success", "values": {}},
                            "report": {"policy_plan": {"plan": null, "approvals": null},
                                "standard_packs": {"games": []}},
                        }),
                    )
                }
            }
            ("actingcommand-actingd.exe", Some("ledger-maintenance")) => {
                ran(self.programs.verify_exit.get(), json!({}))
            }
            // The bounded head read (S0): no row but the head's.
            ("actingledger.exe", _)
                if doing == "export"
                    && words.get(3).map(String::as_str) == Some("--performance")
                    && number_after(&words, "--through") == Some(0) =>
            {
                ran(
                    0,
                    json!({"command": "performance", "data": {"after_sequence": 0,
                        "through_sequence": 0, "limit": 1, "snapshot_latest_sequence": head,
                        "rows": [], "next_after_sequence": null}}),
                )
            }
            // A range page, as the real one reads: the rows after `--after` through `--through`
            // (else one more than the limit, up to the head), then the head row.
            ("actingledger.exe", _) if doing == "events" => {
                let after = number_after(&words, "--after").expect("an --after page");
                let limit = number_after(&words, "--limit").expect("a --limit page");
                let through = number_after(&words, "--through");
                let bound =
                    through.map_or(head.min(after + limit + 1), |through| through.min(head));
                let refused =
                    self.programs
                        .unreadable
                        .borrow()
                        .as_ref()
                        .is_some_and(|(reader, sequence)| {
                            *reader == build
                                && ((after < *sequence && *sequence <= bound) || *sequence == head)
                        });
                if refused {
                    ran(1, json!({}))
                } else {
                    let next = (bound > after + limit).then_some(after + limit);
                    ran(
                        0,
                        json!({"command": "events", "data": {"after_sequence": after,
                            "through_sequence": through.unwrap_or(head), "limit": limit,
                            "events": [], "artifact_evictions": [],
                            "next_after_sequence": next}}),
                    )
                }
            }
            _ => panic!("an unexpected program: {} {words:?}", program.display()),
        })
    }

    fn log_dirs(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.to_path_buf(), root.join("watchdog")]
    }

    fn after_step(&self, operation_id: &str, step: usize) -> Result<(), Outcome> {
        self.log.borrow_mut().push(format!("step {step}"));
        if let Some(on_step) = self.on_step.borrow().as_ref() {
            on_step(step);
        }
        if self.crash_after.get() == Some(step) {
            self.crash_after.set(None);
            self.stopped
                .borrow_mut()
                .push((operation_id.to_owned(), step));
            return Err(crash());
        }
        Ok(())
    }

    fn between_renames(&self, operation_id: &str, step: usize) -> Result<(), Outcome> {
        if self.crash_between_renames.get() == Some(step) {
            self.crash_between_renames.set(None);
            self.stopped
                .borrow_mut()
                .push((operation_id.to_owned(), step));
            return Err(crash());
        }
        Ok(())
    }

    fn before_retire(&self, operation_id: &str) -> Result<(), Outcome> {
        if self.crash_before_retire.replace(false) {
            self.stopped.borrow_mut().push((operation_id.to_owned(), 0));
            return Err(crash());
        }
        Ok(())
    }

    fn runtime(&self) -> &dyn RuntimeLink {
        &self.runtime
    }

    /// A poll moves the fake clock; nothing sleeps.
    fn pause_ms(&self, ms: u64) {
        self.clock.set(self.clock.get() + ms);
    }

    fn available_memory_mb(&self) -> Result<u64, Outcome> {
        Ok(self.memory_mb.get())
    }

    fn runtime_read_bytes(&self, _root: &Path) -> Option<u64> {
        None
    }

    fn detach(&self, arguments: &[String], _directory: &Path, _log: &Path) -> Result<u32, Outcome> {
        self.detached.borrow_mut().push(arguments.to_vec());
        Ok(9_999)
    }
}

/// Workflow #421 INST-2: how one fake start behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FakeStart {
    /// Answers this many milliseconds after its spawn, on the fake clock.
    Ready { after_ms: u64 },
    /// Exits before it answers, with this exit code.
    Exits { code: i32 },
    /// Neither answers nor exits: left running at its deadline.
    Hangs,
}

/// The fake Runtime's owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FakeOwner {
    pub(super) pid: u32,
    pub(super) epoch: String,
    pub(super) started_at_unix_ms: u64,
}

/// Workflow #421 INST-2: the root's fake Runtime. It is stopped until a test boots it; its
/// pauses persist across a close and a start, as the Runtime's own persisted pauses do. A spawn
/// reads which build it starts from the fixed entry's bytes (`neutral daemon <commit>`), writes
/// its start log with an open line, and answers as the next of `starts` says (by default ready
/// after 2 s). Every pause, resume, close and spawn is listed in `calls`.
pub(super) struct FakeRuntime {
    clock: Rc<Cell<u64>>,
    pub(super) running: Cell<bool>,
    /// The build the running Runtime is (`live` for the Runtime a test booted).
    pub(super) build: RefCell<String>,
    pub(super) owner: RefCell<FakeOwner>,
    epochs: Cell<u64>,
    pub(super) instances: RefCell<Vec<InstanceSeen>>,
    pub(super) global_pause: RefCell<Option<PauseSeen>>,
    pub(super) instructions: RefCell<Vec<(String, String)>>,
    pub(super) sources: RefCell<BTreeMap<String, String>>,
    pub(super) plan: RefCell<PlanSeen>,
    pub(super) drains: RefCell<BTreeMap<String, Drained>>,
    pub(super) starts: RefCell<VecDeque<FakeStart>>,
    /// By build: members that replace the default `StartReport` answer's.
    pub(super) report_patch: RefCell<BTreeMap<String, Value>>,
    /// The builds that do not know `StartReport`.
    pub(super) unsupported: RefCell<Vec<String>>,
    /// The close does not complete (its wait passes; nothing is killed).
    pub(super) refuse_shutdown: Cell<bool>,
    pub(super) calls: RefCell<Vec<String>>,
    /// The spawn under way: when, how it behaves and its build.
    pending: RefCell<Option<(u64, FakeStart, String)>>,
    revisions: Cell<u64>,
}

impl FakeRuntime {
    fn new(clock: Rc<Cell<u64>>) -> Self {
        Self {
            clock,
            running: Cell::new(false),
            build: RefCell::new("live".to_owned()),
            owner: RefCell::new(FakeOwner {
                pid: 7_000,
                epoch: "epoch-0".to_owned(),
                started_at_unix_ms: 1_700_000_000_000,
            }),
            epochs: Cell::new(0),
            instances: RefCell::new(Vec::new()),
            global_pause: RefCell::new(None),
            instructions: RefCell::new(Vec::new()),
            sources: RefCell::new(BTreeMap::new()),
            plan: RefCell::new(PlanSeen::default()),
            drains: RefCell::new(BTreeMap::new()),
            starts: RefCell::new(VecDeque::new()),
            report_patch: RefCell::new(BTreeMap::new()),
            unsupported: RefCell::new(Vec::new()),
            refuse_shutdown: Cell::new(false),
            calls: RefCell::new(Vec::new()),
            pending: RefCell::new(None),
            revisions: Cell::new(0),
        }
    }

    /// The Runtime a test boots: running as the root's live build, with `aliases` registered
    /// and unpaused.
    pub(super) fn boot(&self, aliases: &[&str]) {
        self.running.set(true);
        self.next_owner(1_700_000_000_000);
        *self.instances.borrow_mut() = aliases
            .iter()
            .map(|alias| InstanceSeen {
                alias: (*alias).to_owned(),
                pause: None,
                lease_active: false,
                queued_requests: 0,
            })
            .collect();
    }

    fn next_owner(&self, started_at_unix_ms: u64) {
        let epoch = self.epochs.get() + 1;
        self.epochs.set(epoch);
        *self.owner.borrow_mut() = FakeOwner {
            pid: 7_000 + u32::try_from(epoch).unwrap_or(0),
            epoch: format!("epoch-{epoch}"),
            started_at_unix_ms,
        };
    }

    /// Sets the pause of `alias` (another party's, say).
    pub(super) fn pause_as(&self, alias: &str, reason: &str) {
        let revision = self.revisions.get() + 1;
        self.revisions.set(revision);
        if let Some(instance) = self
            .instances
            .borrow_mut()
            .iter_mut()
            .find(|instance| instance.alias == alias)
        {
            instance.pause = Some(PauseSeen {
                reason: reason.to_owned(),
                revision,
            });
        }
    }

    /// The pause reason of `alias`, if paused.
    pub(super) fn pause_of(&self, alias: &str) -> Option<String> {
        self.instances
            .borrow()
            .iter()
            .find(|instance| instance.alias == alias)
            .and_then(|instance| instance.pause.as_ref())
            .map(|pause| pause.reason.clone())
    }

    /// The `StartReport` answer of the running Runtime: every registered instance bound, the
    /// tagged state loaded, an open well within its deadline, nothing dropped or dispatched,
    /// with the members of its build's patch.
    fn report(&self) -> Value {
        let owner = self.owner.borrow().clone();
        let mut report = json!({
            "owner_epoch": owner.epoch,
            "started_at_unix_ms": owner.started_at_unix_ms,
            "ready_at_unix_ms": owner.started_at_unix_ms,
            "open": {
                "phases": {"sql_read_ms": 0, "verify_ms": 0, "material_ms": 0, "restore_ms": 0,
                    "meta_ms": 10, "tail_ms": 20, "index_ms": 1_000, "import_ms": 0},
                "sum_ms": 1_030,
                "deadline_ms": 600_000,
                "slow": false,
            },
            "tagged_state": "load",
            "recovery_ms": 5,
            "pauses_restored": 0,
            "instructions_restored": self.instructions.borrow().len(),
            "instances_bound": self
                .instances
                .borrow()
                .iter()
                .map(|instance| instance.alias.clone())
                .collect::<Vec<_>>(),
        });
        if let Some(Value::Object(patch)) = self.report_patch.borrow().get(&*self.build.borrow()) {
            for (member, value) in patch {
                report[member] = value.clone();
            }
        }
        report
    }
}

/// A fake started process: it exits at once, or runs.
struct FakeStarted(FakeStart);

impl StartedProcess for FakeStarted {
    fn state(&mut self) -> Result<StartedState, Outcome> {
        Ok(match self.0 {
            FakeStart::Exits { code } => StartedState::Exited(Some(code)),
            FakeStart::Ready { .. } | FakeStart::Hangs => StartedState::Running,
        })
    }
}

/// The fake Runtime refusing: the code the real client's failure would carry.
fn refused() -> Outcome {
    Outcome::new(ActingctlCode::ShutdownWaitTimeout)
        .pid(7_000_u32)
        .timeout_ms(std::time::Duration::from_millis(60_000))
}

impl RuntimeLink for FakeRuntime {
    fn status(&self, _state_root: &Path) -> Result<Option<RuntimeSeen>, Outcome> {
        if !self.running.get() {
            return Ok(None);
        }
        let owner = self.owner.borrow().clone();
        Ok(Some(RuntimeSeen {
            owner_epoch: owner.epoch,
            pid: owner.pid,
            started_at_unix_ms: owner.started_at_unix_ms,
            global_pause: self.global_pause.borrow().clone(),
            instances: self.instances.borrow().clone(),
        }))
    }

    fn config(&self, _state_root: &Path) -> Result<ConfigSeen, Outcome> {
        if !self.running.get() {
            return Err(refused());
        }
        Ok(ConfigSeen {
            instructions: self.instructions.borrow().clone(),
            sources: self.sources.borrow().clone(),
        })
    }

    fn plan(&self, _state_root: &Path) -> Result<PlanSeen, Outcome> {
        if !self.running.get() {
            return Err(refused());
        }
        Ok(self.plan.borrow().clone())
    }

    fn pause(
        &self,
        _state_root: &Path,
        alias: &str,
        reason: &str,
        _drain_timeout_ms: u64,
    ) -> Result<Drained, Outcome> {
        if !self.running.get() {
            return Err(refused());
        }
        self.calls
            .borrow_mut()
            .push(format!("pause {alias} {reason}"));
        self.pause_as(alias, reason);
        Ok(self.drains.borrow().get(alias).copied().unwrap_or_default())
    }

    fn resume(
        &self,
        _state_root: &Path,
        alias: &str,
        owner_epoch: &str,
        revision: u64,
    ) -> Result<(), Outcome> {
        if !self.running.get() || self.owner.borrow().epoch != owner_epoch {
            return Err(refused());
        }
        self.calls.borrow_mut().push(format!("resume {alias}"));
        let mut instances = self.instances.borrow_mut();
        let instance = instances
            .iter_mut()
            .find(|instance| instance.alias == alias)
            .ok_or_else(refused)?;
        if instance.pause.as_ref().map(|pause| pause.revision) != Some(revision) {
            return Err(refused());
        }
        instance.pause = None;
        Ok(())
    }

    fn shutdown(&self, _root: &Path, _state_root: &Path, _wait_ms: u64) -> Result<Closed, Outcome> {
        self.calls.borrow_mut().push("shutdown".to_owned());
        if self.refuse_shutdown.get() {
            return Err(refused());
        }
        self.running.set(false);
        Ok(Closed {
            wait: json!({"state": "completed"}),
            terminal: Some(("runtime_stopped".to_owned(), "neutral.log".to_owned())),
        })
    }

    fn start_report(&self, _state_root: &Path) -> Result<Option<Value>, Outcome> {
        if !self.running.get() {
            return Err(refused());
        }
        if self.unsupported.borrow().contains(&*self.build.borrow()) {
            return Ok(None);
        }
        Ok(Some(self.report()))
    }

    fn spawn(&self, target: &Target<'_>, log: &Path) -> Result<Box<dyn StartedProcess>, Outcome> {
        let entry = fs::read_to_string(target.entry()).unwrap_or_default();
        let build = entry
            .strip_prefix("neutral daemon ")
            .unwrap_or("live")
            .to_owned();
        let behaviour = self
            .starts
            .borrow_mut()
            .pop_front()
            .unwrap_or(FakeStart::Ready { after_ms: 2_000 });
        self.calls.borrow_mut().push(format!("spawn {build}"));
        let open = json!({"schema_version": "actingcommand.outcome.v1", "code": "ledger_opened",
            "category": "info", "values": {"sql_read_ms": 0, "verify_ms": 0, "material_ms": 0,
            "restore_ms": 0, "meta_ms": 10, "tail_ms": 20, "index_ms": 1_000, "import_ms": 0,
            "timeout_ms": 600_000}});
        fs::write(log, format!("OUTCOME actingd: {open}\n")).expect("write the start log");
        *self.pending.borrow_mut() = Some((self.clock.get(), behaviour, build));
        Ok(Box::new(FakeStarted(behaviour)))
    }

    fn answering_owner(&self, _state_root: &Path) -> Option<LiveOwner> {
        let pending = self.pending.borrow().clone();
        if let Some((at, FakeStart::Ready { after_ms }, build)) = pending
            && self.clock.get() >= at + after_ms
        {
            self.pending.borrow_mut().take();
            self.running.set(true);
            *self.build.borrow_mut() = build;
            self.next_owner(at);
        }
        if !self.running.get() {
            return None;
        }
        let owner = self.owner.borrow().clone();
        Some(LiveOwner {
            pid: owner.pid,
            owner_epoch: owner.epoch,
            started_at_unix_ms: owner.started_at_unix_ms,
        })
    }
}

/// Runs `actingctl install <words>` on `host`.
pub(super) fn run(host: &dyn Host, words: &[&str]) -> Answer {
    let arguments = words.iter().map(OsString::from).collect::<Vec<_>>();
    execute(&arguments, host)
}

/// A path as a command-line word.
pub(super) fn word(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// The answer's outcome code.
pub(super) fn code(answer: &Answer) -> &str {
    answer.outcome.code().as_str()
}
