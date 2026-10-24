// SPDX-License-Identifier: AGPL-3.0-only

// Test-only: the neutral builds, release folders and roots of the INST-1 gates, and the fake host
// they run `actingctl install` on. Every name is neutral; no file is a real program.

use super::host::{Host, Installer, Ran, RunningImage};
use super::manifest::{Speaks, parse_declaration};
use super::{Answer, execute};
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) const C1: &str = "1111111111111111111111111111111111111111";
pub(super) const C2: &str = "2222222222222222222222222222222222222222";
pub(super) const REPOSITORY: &str = "neutral/runtime";
pub(super) const TAG: &str = "v0.0.1-test";

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The interfaces both manifests of a neutral build declare.
pub(super) fn speaks() -> Value {
    json!({
        "actingd-config": [5, 11],
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
/// folder; it stops an operation after the step `crash_after` names, once, and calls `on_step`
/// after each step first. Its processes are `processes`; its programs answer as `programs` says;
/// its start logs lie in the root itself.
pub(super) struct FakeHost {
    clock: Cell<u64>,
    work: PathBuf,
    pub(super) crash_after: Cell<Option<usize>>,
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
    /// The ledger head `open` and `events` read.
    pub(super) ledger_head: Cell<u64>,
    /// A build whose reader refuses the page holding this sequence.
    pub(super) unreadable: RefCell<Option<(String, u64)>>,
    /// The exit code of `ledger-maintenance verify`.
    pub(super) verify_exit: Cell<i32>,
}

impl FakeHost {
    pub(super) fn new(work: &Path) -> Self {
        fs::create_dir_all(work).expect("create the work folder");
        Self {
            clock: Cell::new(1_800_000_000_000),
            work: work.to_path_buf(),
            crash_after: Cell::new(None),
            stopped: RefCell::new(Vec::new()),
            processes: RefCell::new(Vec::new()),
            programs: Programs {
                refuse_check_config: RefCell::new(Vec::new()),
                ledger_head: Cell::new(100),
                unreadable: RefCell::new(None),
                verify_exit: Cell::new(0),
            },
            on_step: RefCell::new(None),
            log: RefCell::new(Vec::new()),
        }
    }
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

    fn processes_under(&self, _root: &Path) -> Result<Vec<RunningImage>, Outcome> {
        Ok(self.processes.borrow().clone())
    }

    fn run_program(&self, program: &Path, arguments: &[OsString]) -> Result<Ran, Outcome> {
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
        let head = self.programs.ledger_head.get();
        Ok(match (name.as_str(), words.first().map(String::as_str)) {
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
            ("actingledger.exe", _) if words.get(2).map(String::as_str) == Some("open") => ran(
                0,
                json!({"command": "open", "data": {"latest_sequence": head}}),
            ),
            ("actingledger.exe", _) => {
                let after = words
                    .iter()
                    .position(|word| word == "--after")
                    .and_then(|index| words.get(index + 1))
                    .and_then(|after| after.parse::<u64>().ok())
                    .expect("an --after page");
                let through = head.min(after + 1024);
                let refused =
                    self.programs
                        .unreadable
                        .borrow()
                        .as_ref()
                        .is_some_and(|(reader, sequence)| {
                            *reader == build && after < *sequence && *sequence <= through
                        });
                if refused {
                    ran(1, json!({}))
                } else {
                    let next = (through < head).then_some(through);
                    ran(
                        0,
                        json!({"command": "events", "data": {"after_sequence": after,
                            "through_sequence": through, "limit": 1024, "events": [],
                            "artifact_evictions": [], "next_after_sequence": next}}),
                    )
                }
            }
            _ => panic!("an unexpected program: {} {words:?}", program.display()),
        })
    }

    fn log_dirs(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.to_path_buf()]
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
            return Err(Outcome::from_io_error(
                &std::io::Error::from(std::io::ErrorKind::Interrupted),
                actingcommand_contract::outcome::IoOp::Write,
            ));
        }
        Ok(())
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
