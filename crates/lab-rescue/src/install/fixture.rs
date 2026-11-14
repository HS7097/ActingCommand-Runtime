// SPDX-License-Identifier: AGPL-3.0-only

// Test-only: the neutral builds, release folders and roots of the INST-1 gates, and the fake host
// they run `actingctl install` on. Every name is neutral; no file is a real program.

use super::host::{Host, Installer, NotFinished, NotRan, Ran, RunningImage};
use super::manifest::{Speaks, parse_declaration};
use super::runtime::{
    Closed, ConfigSeen, Drained, InstanceSeen, PauseSeen, PlanSeen, Resumed, RuntimeLink,
    RuntimeSeen,
};
use super::{Answer, execute};
use crate::codes::ActingctlCode;
use crate::watchdog::decide::LiveOwner;
use crate::watchdog::start::{StartedProcess, StartedState, Target};
use actingcommand_contract::SchedulingPauseReason;
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::initiator;
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

/// The interfaces both manifests of a neutral build declare: `actingd-config` up to revision 18
/// (INST-2, v0.12.19), whose check-config takes `--install-root` (revision 17, INST-1b-F fix
/// round 1).
pub(super) fn speaks() -> Value {
    json!({
        "actingd-config": [5, 18],
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
    /// INST-2 fix round 1: the available memory of the next reads, one per read (then
    /// `memory_mb`), and the running Runtime's working set.
    pub(super) memory_reads: RefCell<VecDeque<u64>>,
    pub(super) working_set_mb: Cell<Option<u64>>,
    /// The reads a starting Runtime reports at each progress sample (none when empty).
    pub(super) read_bytes: Cell<Option<u64>>,
    pub(super) detached: RefCell<Vec<Vec<String>>>,
    pub(super) crash_after: Cell<Option<usize>>,
    pub(super) crash_between_renames: Cell<Option<usize>>,
    pub(super) crash_before_retire: Cell<bool>,
    pub(super) stopped: RefCell<Vec<(String, usize)>>,
    pub(super) processes: RefCell<Vec<RunningImage>>,
    /// Workflow #434 LAB-3 fix 1: the process id the host answers as its own (none by default).
    pub(super) own_pid: Cell<Option<u32>>,
    /// Workflow #444 NOTICES-2: the `package` range the installer declares instead of
    /// [`speaks`]'s (none by default: [`speaks`]'s).
    pub(super) installer_package: Cell<Option<[u64; 2]>>,
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
    /// Workflow #453 S6 fix 1: the notes every check-config that passes gives with its
    /// `check_config_passed`, as wire links (`code`, `category`, `relation` `note`, `values`), as
    /// the real one joins a problem of a game root or of its composition (none by default).
    pub(super) check_config_notes: RefCell<Vec<Value>>,
    /// Workflow #453 S6 fix 2: the `causes_total` of every check-config that passes, as the
    /// outcome's envelope sets it when it cut links (none by default).
    pub(super) check_config_causes_total: Cell<Option<u64>>,
    /// Workflow #453 S6 fix 2: members of the `report` of every check-config that passes, each
    /// replacing the default's member of its name (`standard_packs`, `standard_pack_schedule`;
    /// none by default).
    pub(super) check_config_report: RefCell<BTreeMap<&'static str, Value>>,
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
    /// Workflow #453 S10: the exit code of `actingd discover` and the line it prints (`null`, no
    /// report, until a test sets one).
    pub(super) discover: RefCell<(i32, Value)>,
    /// Workflow #453 S10 fix 1: every program run to be ended at its deadline
    /// (`run_program_ending`) that never exits (a word of `stall`) and was ended there, by that
    /// word, with its deadline.
    pub(super) ended: RefCell<Vec<(String, u64)>>,
    /// Workflow #453 S7: the exit code and the line of `actingd vision <word>`, by its word
    /// (`manifest`, `check`; none set: `null`, no report).
    pub(super) vision: RefCell<BTreeMap<String, (i32, Value)>>,
    /// Workflow #453 S7: the exit code and the line of the Tools' `standard-pack check --root
    /// <game root>`, by the game root as the command line names it (none set: a panic).
    pub(super) standard_pack_checks: RefCell<BTreeMap<String, (i32, Value)>>,
    /// Workflow #453 S7 fix 1: how long each program run to be ended at its deadline
    /// (`run_program_ending`) that exits takes on the fake clock (none by default); one that never
    /// exits takes its deadline.
    pub(super) run_ms: Cell<u64>,
    /// Workflow #453 S12: the exit code of `actingd claim-state-root` and the line it prints
    /// (`null`, no report, until a test sets one).
    pub(super) claim: RefCell<(i32, Value)>,
    /// Workflow #453 S12: the exit code and the line every check-config prints instead of the
    /// default's, when a test sets one (none by default).
    pub(super) check_config_line: RefCell<Option<(i32, Value)>>,
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
            memory_reads: RefCell::new(VecDeque::new()),
            working_set_mb: Cell::new(None),
            read_bytes: Cell::new(None),
            detached: RefCell::new(Vec::new()),
            clock,
            work: work.to_path_buf(),
            crash_after: Cell::new(None),
            crash_between_renames: Cell::new(None),
            crash_before_retire: Cell::new(false),
            stopped: RefCell::new(Vec::new()),
            processes: RefCell::new(Vec::new()),
            own_pid: Cell::new(None),
            installer_package: Cell::new(None),
            programs: Programs {
                refuse_check_config: RefCell::new(Vec::new()),
                refuse_install_root: RefCell::new(Vec::new()),
                check_config_notes: RefCell::new(Vec::new()),
                check_config_causes_total: Cell::new(None),
                check_config_report: RefCell::new(BTreeMap::new()),
                ledger_head: Cell::new(100),
                unreadable: RefCell::new(None),
                verify_exit: Cell::new(0),
                stall: RefCell::new(Vec::new()),
                deadlines: RefCell::new(Vec::new()),
                discover: RefCell::new((0, Value::Null)),
                ended: RefCell::new(Vec::new()),
                vision: RefCell::new(BTreeMap::new()),
                standard_pack_checks: RefCell::new(BTreeMap::new()),
                run_ms: Cell::new(0),
                claim: RefCell::new((0, Value::Null)),
                check_config_line: RefCell::new(None),
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
        let mut declared = speaks();
        if let Some(range) = self.installer_package.get() {
            declared["package"] = json!(range);
        }
        parse_declaration(&json!({
            "schema_version": "actingcommand.component-interfaces.v1",
            "speaks": declared,
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
                parent_pid: None,
            });
        }
        Ok(images)
    }

    fn own_pid(&self) -> Option<u32> {
        self.own_pid.get()
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
            // Workflow #453 S12: the line the test set for every check-config.
            ("actingcommand-actingd.exe", Some("check-config"))
                if self.programs.check_config_line.borrow().is_some() =>
            {
                let (exit_code, line) = self
                    .programs
                    .check_config_line
                    .borrow()
                    .clone()
                    .unwrap_or((0, Value::Null));
                ran(exit_code, line)
            }
            // Workflow #453 S12: the line the test set.
            ("actingcommand-actingd.exe", Some("claim-state-root")) => {
                let (exit_code, line) = self.programs.claim.borrow().clone();
                ran(exit_code, line)
            }
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
                    let mut outcome = json!({"schema_version": "actingcommand.outcome.v1",
                        "code": "check_config_passed", "category": "success", "values": {}});
                    let notes = self.programs.check_config_notes.borrow().clone();
                    if !notes.is_empty() {
                        outcome["causes"] = Value::Array(notes);
                    }
                    if let Some(total) = self.programs.check_config_causes_total.get() {
                        outcome["causes_total"] = json!(total);
                    }
                    let mut report = json!({"policy_plan": {"plan": null, "approvals": null},
                        "standard_packs": {"games": []}});
                    for (name, member) in self.programs.check_config_report.borrow().iter() {
                        report[*name] = member.clone();
                    }
                    ran(
                        0,
                        json!({
                            "schema_version": "actingcommand.actingd.check-config.v2",
                            "outcome": outcome,
                            "report": report,
                        }),
                    )
                }
            }
            ("actingcommand-actingd.exe", Some("ledger-maintenance")) => {
                ran(self.programs.verify_exit.get(), json!({}))
            }
            // Workflow #453 S10: the line the test set.
            ("actingcommand-actingd.exe", Some("discover")) => {
                let (exit_code, line) = self.programs.discover.borrow().clone();
                ran(exit_code, line)
            }
            // Workflow #453 S7: the line the test set for the word after `vision`.
            ("actingcommand-actingd.exe", Some("vision")) => {
                let word = words.get(1).cloned().unwrap_or_default();
                let (exit_code, line) = self
                    .programs
                    .vision
                    .borrow()
                    .get(&word)
                    .cloned()
                    .unwrap_or((0, Value::Null));
                ran(exit_code, line)
            }
            // Workflow #453 S7: the Tools' `standard-pack check` of a game root, the line the
            // test set for that root.
            ("actinglab.exe", Some("--json"))
                if words.get(1).map(String::as_str) == Some("standard-pack") =>
            {
                let game_root = words.last().cloned().unwrap_or_default();
                let (exit_code, line) = self
                    .programs
                    .standard_pack_checks
                    .borrow()
                    .get(&game_root)
                    .cloned()
                    .unwrap_or_else(|| panic!("no standard-pack check set for {game_root}"));
                ran(exit_code, line)
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

    /// Workflow #453 S10 fix 1: a program whose word is in `stall` never exits; it is ended at its
    /// deadline, kept in `ended`, and left among no processes (unlike [`Self::run_program`]'s, which
    /// is left running with `STALLED_PID`). Any other program answers as `run_program` does.
    /// Workflow #453 S7 fix 1: the fake clock moves by `run_ms` for a program that exits, by the
    /// deadline for one that never does. Fix 2: a program that would take longer than its
    /// deadline (`run_ms` past it) is ended there too, as one that never exits.
    fn run_program_ending(
        &self,
        program: &Path,
        arguments: &[OsString],
        timeout_ms: u64,
    ) -> Result<Ran, NotFinished> {
        let doing = arguments
            .first()
            .map(|word| word.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stalled = self
            .programs
            .stall
            .borrow()
            .iter()
            .any(|word| *word == doing);
        if !stalled && self.programs.run_ms.get() <= timeout_ms {
            self.clock
                .set(self.clock.get() + self.programs.run_ms.get());
            return self.run_program(program, arguments, timeout_ms).map_err(
                |not_ran| match not_ran {
                    NotRan::Failed(cause) => NotFinished::Failed(cause),
                    NotRan::StillRunning { .. } => panic!("only a stalled program runs on"),
                },
            );
        }
        let name = program
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let words = arguments
            .iter()
            .map(|word| word.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.log
            .borrow_mut()
            .push(format!("{} {name} {}", build_of(program), words.join(" ")));
        self.programs
            .deadlines
            .borrow_mut()
            .push((doing.clone(), timeout_ms));
        // It runs on to its deadline; there it is ended and reaped.
        self.clock.set(self.clock.get() + timeout_ms);
        self.programs.ended.borrow_mut().push((doing, timeout_ms));
        Err(NotFinished::Ended)
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
        Ok(self
            .memory_reads
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| self.memory_mb.get()))
    }

    fn runtime_read_bytes(&self, _root: &Path) -> Option<u64> {
        self.read_bytes.get()
    }

    fn runtime_working_set_mb(&self, _root: &Path) -> Option<u64> {
        self.working_set_mb.get()
    }

    fn detach(&self, arguments: &[String], _directory: &Path, _log: &Path) -> Result<u32, Outcome> {
        self.detached.borrow_mut().push(arguments.to_vec());
        Ok(9_999)
    }

    /// Workflow #434 LAB-3: the watchdog of the fake Runtime: `status` decides `watchdog_alive`
    /// while it runs, `watchdog_never_started` otherwise; `install` and `uninstall` succeed.
    /// Every question is listed in the host's log (`watchdog <word>`).
    fn watchdog(&self, asked: crate::watchdog::Asked, _root: &Path) -> crate::watchdog::Answered {
        let (word, outcome) = match asked {
            crate::watchdog::Asked::Status if self.runtime.running.get() => {
                let owner = self.runtime.owner.borrow().clone();
                (
                    "status",
                    Outcome::new(ActingctlCode::WatchdogAlive)
                        .owner_epoch(owner.epoch)
                        .pid(owner.pid),
                )
            }
            crate::watchdog::Asked::Status => {
                ("status", Outcome::new(ActingctlCode::WatchdogNeverStarted))
            }
            // Workflow #453 S4: with the task's name, which the code requires (the real one names
            // the task it registered).
            crate::watchdog::Asked::Install => (
                "install",
                Outcome::new(ActingctlCode::WatchdogInstalled).name("neutral-watchdog-task"),
            ),
            crate::watchdog::Asked::Uninstall => (
                "uninstall",
                Outcome::new(ActingctlCode::WatchdogUninstalled),
            ),
        };
        self.log.borrow_mut().push(format!("watchdog {word}"));
        crate::watchdog::Answered {
            line: Some(json!({
                "schema_version": "actingcommand.watchdog.report.v2",
                "outcome": outcome.envelope(actingcommand_contract::outcome::Detail::Full),
                "attention": [],
            })),
            outcome,
            developer_before: Vec::new(),
        }
    }

    /// The detached start's environment first, as `initiator=<word>` for the initiator's variable
    /// and (Workflow #434 LAB-6S0) `marker=<value>` for the marker, any other variable as
    /// `<variable>=<value>`, then the words.
    fn detach_rescue(
        &self,
        arguments: &[String],
        environment: &[(&str, &str)],
        _directory: &Path,
        _log: &Path,
    ) -> Result<u32, Outcome> {
        let mut line = environment
            .iter()
            .map(|&(variable, value)| match variable {
                initiator::ENVIRONMENT_VARIABLE => format!("initiator={value}"),
                initiator::DETACHED_RUN_VARIABLE => format!("marker={value}"),
                other => format!("{other}={value}"),
            })
            .collect::<Vec<_>>();
        line.extend(arguments.iter().cloned());
        self.detached.borrow_mut().push(line);
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

/// Workflow #454: what a test runs at a `StartReport` read of the fake Runtime, given its build.
pub(super) type ReportHook = Box<dyn Fn(&str)>;

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
    /// INST-2 fix round 1: by alias, the fake time until which the instance has a queued
    /// request (its status shows one, and its pause is refused); every batch of pauses asked
    /// at once; the next pauses that fail as the Runtime refusing them (by alias).
    pub(super) queued_until: RefCell<BTreeMap<String, u64>>,
    pub(super) batches: RefCell<Vec<Vec<String>>>,
    pub(super) refuse_pause: RefCell<Vec<String>>,
    /// The Runtime goes away right after it answered readiness (its process died).
    pub(super) dies_after_ready: Cell<bool>,
    /// The `StartReport` read fails (not as an older build's: the connection failed).
    pub(super) report_fails: Cell<bool>,
    pub(super) calls: RefCell<Vec<String>>,
    /// The spawn under way: when, how it behaves and its build.
    pending: RefCell<Option<(u64, FakeStart, String)>>,
    revisions: Cell<u64>,
    /// Workflow #434 LAB-3: the catalog the latest evaluation ran under, and the ones the next
    /// reads answer, one per read (a new composition the workbench activates).
    pub(super) catalog: RefCell<Option<String>>,
    pub(super) catalogs: RefCell<VecDeque<String>>,
    /// Workflow #421 INST-5: the aliases the next start that comes ready registers, unpaused (the
    /// instances a configuration it starts with gained); none by default.
    pub(super) registers_at_start: RefCell<Vec<String>>,
    /// Workflow #454: the aliases the next start that comes ready no longer registers (the
    /// instances a configuration it starts with lost), with their pauses; none by default.
    pub(super) drops_at_start: RefCell<Vec<String>>,
    /// Workflow #454: called with the build of the running Runtime at every `StartReport` read,
    /// before it answers (none by default).
    pub(super) on_report: RefCell<Option<ReportHook>>,
    /// Workflow #454 (register §1.348): by build, the members of the `config.start` fact its
    /// configuration facts carry, over `{owner_epoch: <the running owner's>}`; a build not listed
    /// records none (a build before 0.12.59).
    pub(super) start_fact: RefCell<BTreeMap<String, Value>>,
    /// Workflow #421 INST-5b: every status read of the running Runtime: how many calls `calls`
    /// listed then, and the reason of the global pause it showed.
    pub(super) status_reads: RefCell<Vec<(usize, Option<String>)>>,
    /// Workflow #453 S3: the pauses the next start that comes ready holds, after it registered the
    /// instances it gained, each `(alias, reason)` (a Runtime with Workflow #453 S2 holds an entry
    /// that declares `start_paused` with `instance.added`); none by default.
    pub(super) pauses_at_start: RefCell<Vec<(String, String)>>,
    /// Workflow #453 S3: the aliases whose next resume the Runtime answers by holding the instance
    /// again with `instance.added` at the next revision, as a Runtime with Workflow #453 S2 answers
    /// for an entry its start left unapplied; none by default.
    pub(super) holds_at_release: RefCell<Vec<String>>,
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
            queued_until: RefCell::new(BTreeMap::new()),
            batches: RefCell::new(Vec::new()),
            refuse_pause: RefCell::new(Vec::new()),
            dies_after_ready: Cell::new(false),
            report_fails: Cell::new(false),
            calls: RefCell::new(Vec::new()),
            pending: RefCell::new(None),
            revisions: Cell::new(0),
            catalog: RefCell::new(None),
            catalogs: RefCell::new(VecDeque::new()),
            registers_at_start: RefCell::new(Vec::new()),
            drops_at_start: RefCell::new(Vec::new()),
            on_report: RefCell::new(None),
            start_fact: RefCell::new(BTreeMap::new()),
            status_reads: RefCell::new(Vec::new()),
            pauses_at_start: RefCell::new(Vec::new()),
            holds_at_release: RefCell::new(Vec::new()),
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
                game: None,
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
                reason: SchedulingPauseReason::new(reason).expect("a pause reason"),
                revision,
            });
        }
    }

    /// Workflow #434 LAB-3 fix 2: sets the global pause (another party's).
    pub(super) fn pause_global_as(&self, reason: &str) {
        let revision = self.revisions.get() + 1;
        self.revisions.set(revision);
        *self.global_pause.borrow_mut() = Some(PauseSeen {
            reason: SchedulingPauseReason::new(reason).expect("a pause reason"),
            revision,
        });
    }

    /// Workflow #434 LAB-3 fix 2: registers `alias`, unpaused (an instance the configuration
    /// gained).
    pub(super) fn register(&self, alias: &str) {
        self.instances.borrow_mut().push(InstanceSeen {
            alias: alias.to_owned(),
            pause: None,
            lease_active: false,
            queued_requests: 0,
            game: None,
        });
    }

    /// Workflow #434 LAB-3 fix 2: the Runtime started by another hand (the watchdog, a reboot)
    /// after an installer's start was left running: a new owner epoch, the build of the start under
    /// way (if any), and every persisted pause restored as the real Runtime's restore does for a
    /// start that is not installer-held (`crates/runtime-host/src/host/scheduling_pause.rs`,
    /// `restore_scheduling_pauses`): its reason kept, its revision 1.
    pub(super) fn restart_restored(&self) {
        if let Some((_, _, build)) = self.pending.borrow_mut().take() {
            *self.build.borrow_mut() = build;
        }
        self.running.set(true);
        self.next_owner(self.clock.get());
        for instance in self.instances.borrow_mut().iter_mut() {
            if let Some(pause) = instance.pause.as_mut() {
                pause.revision = 1;
            }
        }
        if let Some(pause) = self.global_pause.borrow_mut().as_mut() {
            pause.revision = 1;
        }
    }

    /// The pause reason of `alias`, if paused.
    pub(super) fn pause_of(&self, alias: &str) -> Option<String> {
        self.instances
            .borrow()
            .iter()
            .find(|instance| instance.alias == alias)
            .and_then(|instance| instance.pause.as_ref())
            .map(|pause| pause.reason.as_str().to_owned())
    }

    /// Whether `alias` has a queued request now, on the fake clock.
    fn queued(&self, alias: &str) -> bool {
        self.queued_until
            .borrow()
            .get(alias)
            .is_some_and(|until| self.clock.get() < *until)
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
        let mut instances = self.instances.borrow().clone();
        for instance in &mut instances {
            if self.queued(&instance.alias) {
                instance.queued_requests = 1;
            }
        }
        let global = self
            .global_pause
            .borrow()
            .as_ref()
            .map(|pause| pause.reason.as_str().to_owned());
        self.status_reads
            .borrow_mut()
            .push((self.calls.borrow().len(), global));
        Ok(Some(RuntimeSeen {
            owner_epoch: owner.epoch,
            pid: owner.pid,
            started_at_unix_ms: owner.started_at_unix_ms,
            global_pause: self.global_pause.borrow().clone(),
            instances,
        }))
    }

    fn config(&self, _state_root: &Path) -> Result<ConfigSeen, Outcome> {
        if !self.running.get() {
            return Err(refused());
        }
        let start = self
            .start_fact
            .borrow()
            .get(&*self.build.borrow())
            .map(|members| {
                let mut row = json!({"owner_epoch": self.owner.borrow().epoch.clone()});
                if let Value::Object(members) = members {
                    for (member, value) in members {
                        row[member] = value.clone();
                    }
                }
                row
            });
        Ok(ConfigSeen {
            instructions: self.instructions.borrow().clone(),
            sources: self.sources.borrow().clone(),
            start,
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
        reason: &SchedulingPauseReason,
        _drain_timeout_ms: u64,
    ) -> Result<Drained, Outcome> {
        if !self.running.get() {
            return Err(refused());
        }
        // A queued request fails the pause at once, as the Runtime's `TransferNotSafe` does.
        if self.queued(alias) {
            self.calls
                .borrow_mut()
                .push(format!("pause {alias} {reason} refused"));
            return Err(refused());
        }
        let mut refuse = self.refuse_pause.borrow_mut();
        if let Some(index) = refuse.iter().position(|known| known == alias) {
            refuse.remove(index);
            self.calls
                .borrow_mut()
                .push(format!("pause {alias} {reason} refused"));
            return Err(refused());
        }
        drop(refuse);
        self.calls
            .borrow_mut()
            .push(format!("pause {alias} {reason}"));
        self.pause_as(alias, reason.as_str());
        Ok(self.drains.borrow().get(alias).copied().unwrap_or_default())
    }

    fn pause_many(
        &self,
        state_root: &Path,
        aliases: &[String],
        reason: &SchedulingPauseReason,
        drain_timeout_ms: u64,
    ) -> Vec<(String, Result<Drained, Outcome>)> {
        self.batches.borrow_mut().push(aliases.to_vec());
        aliases
            .iter()
            .map(|alias| {
                (
                    alias.clone(),
                    self.pause(state_root, alias, reason, drain_timeout_ms),
                )
            })
            .collect()
    }

    fn resume(
        &self,
        _state_root: &Path,
        alias: &str,
        owner_epoch: &str,
        revision: u64,
    ) -> Result<Resumed, Outcome> {
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
        // Workflow #453 S3: the release of an entry the start left unapplied: the old pause lifted
        // and `instance.added` set at the next revision, which the answer carries.
        let mut held = self.holds_at_release.borrow_mut();
        if let Some(index) = held.iter().position(|known| known == alias) {
            held.remove(index);
            let revision = self.revisions.get() + 1;
            self.revisions.set(revision);
            instance.pause = Some(PauseSeen {
                reason: SchedulingPauseReason::new(
                    actingcommand_contract::INSTANCE_ADDED_PAUSE_REASON,
                )
                .expect("a pause reason"),
                revision,
            });
            return Ok(Resumed::HeldAgain { revision });
        }
        instance.pause = None;
        Ok(Resumed::Lifted)
    }

    /// Workflow #421 INST-5b: a global pause already in effect refuses, as the Runtime's
    /// `scheduling_already_paused` does.
    fn pause_global(
        &self,
        _state_root: &Path,
        reason: &SchedulingPauseReason,
        _drain_timeout_ms: u64,
    ) -> Result<u64, Outcome> {
        if !self.running.get() || self.global_pause.borrow().is_some() {
            return Err(refused());
        }
        self.calls
            .borrow_mut()
            .push(format!("global pause {reason}"));
        self.pause_global_as(reason.as_str());
        Ok(self.revisions.get())
    }

    fn resume_global(
        &self,
        _state_root: &Path,
        owner_epoch: &str,
        revision: u64,
    ) -> Result<(), Outcome> {
        if !self.running.get() || self.owner.borrow().epoch != owner_epoch {
            return Err(refused());
        }
        self.calls.borrow_mut().push("global resume".to_owned());
        let mut global = self.global_pause.borrow_mut();
        if global.as_ref().map(|pause| pause.revision) != Some(revision) {
            return Err(refused());
        }
        *global = None;
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
            stopped: true,
        })
    }

    fn start_report(&self, _state_root: &Path) -> Result<Option<Value>, Outcome> {
        if !self.running.get() || self.report_fails.get() {
            return Err(refused());
        }
        if let Some(on_report) = self.on_report.borrow().as_ref() {
            let build = self.build.borrow().clone();
            on_report(build.as_str());
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
            let gained = std::mem::take(&mut *self.registers_at_start.borrow_mut());
            for alias in gained {
                self.register(&alias);
            }
            let held = std::mem::take(&mut *self.pauses_at_start.borrow_mut());
            for (alias, reason) in held {
                self.pause_as(&alias, &reason);
            }
            let lost = std::mem::take(&mut *self.drops_at_start.borrow_mut());
            self.instances
                .borrow_mut()
                .retain(|instance| !lost.contains(&instance.alias));
        }
        if !self.running.get() {
            return None;
        }
        let owner = self.owner.borrow().clone();
        // Ready once, then gone (a Runtime that died after its readiness).
        if self.dies_after_ready.replace(false) {
            self.running.set(false);
        }
        Some(LiveOwner {
            pid: owner.pid,
            owner_epoch: owner.epoch,
            started_at_unix_ms: owner.started_at_unix_ms,
        })
    }

    fn catalog_hash(&self, _state_root: &Path) -> Result<Option<String>, Outcome> {
        if !self.running.get() {
            return Err(refused());
        }
        self.calls.borrow_mut().push("catalog".to_owned());
        if let Some(next) = self.catalogs.borrow_mut().pop_front() {
            *self.catalog.borrow_mut() = Some(next);
        }
        Ok(self.catalog.borrow().clone())
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
