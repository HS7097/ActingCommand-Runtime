// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S9 (register §1.364; contracts/install-cli.md, "The Lab's rescue module",
// `vision place`; contracts/vision-model-folders.md, "Placing a vision root from a folder";
// coordinator work order rd-1114/brief-453-s9-vision-place.md; design note
// rd-1113/fresh-root-design.md §3(c)): the rescue row `vision place --from <dir>` places the files
// the configured vision root lacks or holds with another SHA-256, each verified below the folder
// given before anything is written, while the Runtime is stopped:
// - `s9_missing_files_are_placed_and_the_post_check_passes`: an empty vision root (but a file the
//   manifest does not name and another model folder): every manifest file placed, the tree the
//   manifest's files plus what was there; one `vision_place` journal with one `vision_file` step
//   per file and `post_check`, which passes; actingd's check run before and after; watchdog row
//   7a″ stands aside after every step; the file below `--from` the manifest does not name counted
//   in `ignored` and not copied;
// - `s9_a_differing_file_is_replaced_and_the_old_one_kept_byte_for_byte`: one file of another
//   SHA-256: one step, the right file in place, the old one kept as `<file>.old-<ms>` (the
//   operation's milliseconds), byte for byte, and named in the answer;
// - `s9_a_complete_root_opens_no_operation_and_writes_nothing`: `installation_changed` false, no
//   operation id, actingd's check run once, the root's whole tree (its installer folder and its
//   vision root) as before;
// - `s9_a_wrong_or_missing_source_file_refuses_before_anything_is_written`: a file below `--from`
//   of another size, one of the same size and another SHA-256, and one missing: each
//   `vision_source_invalid` with its path and its state, the vision root and the installer folder
//   as before, no journal;
// - `s9_a_running_runtime_a_held_owner_lock_or_an_undecidable_probe_refuses`: an actingd under
//   the root `runtime_running`, the state root's owner lock held `owner_lock_held`, an owner
//   journal the probe cannot read `processes_unreadable`: each with nothing written;
// - `s9_a_crash_after_each_step_or_between_the_renames_resumes_to_the_same_tree`: a crash after
//   each of the three steps, and between the two renames of the replaced file's step, ends the
//   command `install_operation_interrupted` with the watchdog standing aside; `op resume` finishes
//   it, and the vision root is the tree of an uninterrupted placement (the kept file's milliseconds
//   aside), the replaced file kept;
// - `s9_undo_sets_the_placed_files_aside_and_puts_the_replaced_file_back`: `undo` of a finished
//   placement: the vision root as before, each placed file kept beside it as
//   `.rolledback-<ms2>`, nothing deleted;
// - `s9_another_model_folder_and_an_unnamed_file_are_untouched`: another folder under `models\`
//   and a file the manifest does not name, in the vision root and (with other bytes) below
//   `--from`: the vision root's own byte for byte, nothing of `--from`'s copied, `ignored` 3.
// The closing round after the independent review (coordinator's message of 11-14):
// - `s9_undo_after_a_crash_between_the_two_renames_puts_the_old_file_back`: the replacing step
//   stopped with its file renamed `.old-<ms>` and its part waiting; `undo` renames the old file
//   back, sets the part aside as `.abandoned-<ms2>` and the file placed before as
//   `.rolledback-<ms2>`, deleting nothing;
// - `s9_a_failed_check_after_the_placement_ends_post_check_failed_with_the_files_placed`: the
//   check after the placement finds the manifest's model of another identity: the command ends
//   `post_check_failed` (`count` 1) with the operation's id and `undo` as its next step, the files
//   placed, the step closed with what it read, the operation ended, nothing unfinished;
// - `s9_a_file_the_report_lists_unpinned_refuses_and_nothing_is_copied`: a report that lists a
//   license file without a pin refuses `vision_report_unanswered`, caused by
//   `foreign_data_error`, before anything is written: the vision root and the installer folder as
//   before, no journal (the work order's ruling 8).
// The machine is the fake host of `fixture.rs` seen through [`Machine`], whose live actingd's
// `vision check` answers from the vision root on disk against an injected pin table of made-up
// files ([`PINS`]), comparing each by its size and then its bytes as actingd's own check does: no
// real model, library or license file, no program spawned, no sleep (the fake clock).

use super::fixture::{BuildSpec, C1, FakeHost, code, live_root, run, sha256_hex, tree, word};
use super::host::{Host, Installer, NotFinished, NotRan, Ran, RunningImage};
use super::manifest::Speaks;
use super::runtime::RuntimeLink;
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tempfile::TempDir;

/// The initiator the tests run the module as.
const WORD: Initiator = Initiator::LabRescue;

/// One file of the made-up manifest: its place below the vision root, its bytes (the pin's size
/// and SHA-256 are theirs) and whether it is a license file.
struct Pin {
    path: &'static str,
    bytes: &'static [u8],
    license: bool,
}

/// The made-up manifest of provider `cpu`, in byte order of the place, as actingd lists it.
const PINS: [Pin; 6] = [
    Pin {
        path: "models/neutral.ocr/det.onnx",
        bytes: b"neutral detector",
        license: false,
    },
    Pin {
        path: "models/neutral.ocr/keys.txt",
        bytes: b"neutral dictionary",
        license: false,
    },
    Pin {
        path: "models/neutral.ocr/rec.onnx",
        bytes: b"neutral recognizer",
        license: false,
    },
    Pin {
        path: "ort/licenses/onnxruntime/LICENSE",
        bytes: b"neutral license text",
        license: true,
    },
    Pin {
        path: "ort/onnxruntime.dll",
        bytes: b"neutral runtime library",
        license: false,
    },
    Pin {
        path: "ort/onnxruntime_providers_shared.dll",
        bytes: b"neutral providers library",
        license: false,
    },
];
/// The index in [`PINS`] of the dictionary and of the recognizer.
const KEYS: usize = 1;
const REC: usize = 2;
/// The manifest's one model and its made-up identity.
const MODEL: &str = "neutral.ocr";

/// The fake host of `fixture.rs`, but for the live build's `actingd vision check`, which answers
/// from the vision root on disk against [`PINS`]; how many times it ran. Its report can also say
/// that the manifest's model folder is of another identity (`identity_differs`), and list one file
/// of [`PINS`] without a pin (`unpinned`, its index).
struct Machine {
    fake: FakeHost,
    vision_root: PathBuf,
    checks: Cell<usize>,
    identity_differs: Cell<bool>,
    unpinned: Cell<Option<usize>>,
}

/// The state of `pin` at `at`, as actingd's check states it: its size first, then its bytes.
fn state_of(at: &Path, pin: &Pin) -> &'static str {
    match fs::metadata(at) {
        Err(error) if error.kind() == ErrorKind::NotFound => "missing",
        Err(_) => "unreadable",
        Ok(metadata) if !metadata.is_file() || metadata.len() != pin.bytes.len() as u64 => {
            "sha256_mismatch"
        }
        Ok(_) => match fs::read(at) {
            Ok(bytes) if bytes == pin.bytes => "ok",
            Ok(_) => "sha256_mismatch",
            Err(_) => "unreadable",
        },
    }
}

/// A place with `/` separators below `base`.
fn below(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

/// Writes `bytes` at the place `relative` below `base`, its folders created.
fn write_at(base: &Path, relative: &str, bytes: &[u8]) {
    let path = below(base, relative);
    fs::create_dir_all(path.parent().expect("a parent")).expect("the folders");
    fs::write(&path, bytes).expect("write a file");
}

impl Machine {
    /// The line the live build's `actingd vision check` prints for the vision root as it is now.
    fn check_line(&self) -> Value {
        let files = PINS
            .iter()
            .enumerate()
            .map(|(index, pin)| {
                let pinned = self.unpinned.get() != Some(index);
                let (size, sha256) = if pinned {
                    (json!(pin.bytes.len()), json!(sha256_hex(pin.bytes)))
                } else {
                    (Value::Null, Value::Null)
                };
                json!({
                    "vision_root_path": pin.path,
                    "size": size,
                    "sha256": sha256,
                    "component": "neutral-component",
                    "archive_path": null,
                    "license": pin.license,
                    "pinned": pinned,
                    "state": state_of(&below(&self.vision_root, pin.path), pin),
                })
            })
            .collect::<Vec<_>>();
        let model_ok = !self.identity_differs.get()
            && PINS
                .iter()
                .filter(|pin| pin.path.starts_with("models/"))
                .all(|pin| state_of(&below(&self.vision_root, pin.path), pin) == "ok");
        let identity = "a".repeat(64);
        let found = if model_ok {
            Value::from(identity.clone())
        } else {
            Value::Null
        };
        json!({
            "schema_version": "actingcommand.actingd.vision.v1",
            "outcome": {"schema_version": "actingcommand.outcome.v1",
                "code": "vision_check_completed", "category": "success", "values": {}},
            "report": {
                "provider": "cpu",
                "vision_root": self.vision_root.to_string_lossy(),
                "files": files,
                "folders": [],
                "models": [{"model_ref": MODEL, "family": "ppocr-ctc", "model_sha256": identity,
                    "present": true, "identity": found,
                    "identity_matches": model_ok}],
            },
        })
    }
}

impl Host for Machine {
    fn now_unix_ms(&self) -> u64 {
        self.fake.now_unix_ms()
    }

    fn installer(&self) -> Installer {
        self.fake.installer()
    }

    fn installer_speaks(&self) -> Option<Speaks> {
        self.fake.installer_speaks()
    }

    fn work_dir(&self) -> PathBuf {
        self.fake.work_dir()
    }

    fn processes_under(&self, root: &Path) -> Result<Vec<RunningImage>, Outcome> {
        self.fake.processes_under(root)
    }

    fn own_pid(&self) -> Option<u32> {
        self.fake.own_pid()
    }

    fn run_program(
        &self,
        program: &Path,
        arguments: &[OsString],
        timeout_ms: u64,
    ) -> Result<Ran, NotRan> {
        self.fake.run_program(program, arguments, timeout_ms)
    }

    /// The live build's `actingd vision check` answers [`Machine::check_line`]; every other
    /// program as the fake host's.
    fn run_program_ending(
        &self,
        program: &Path,
        arguments: &[OsString],
        timeout_ms: u64,
    ) -> Result<Ran, NotFinished> {
        let words = arguments
            .iter()
            .map(|word| word.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let daemon = program
            .file_name()
            .is_some_and(|name| name == "actingcommand-actingd.exe");
        if daemon
            && words.first().is_some_and(|word| word == "vision")
            && words.get(1).is_some_and(|word| word == "check")
        {
            self.fake.log.borrow_mut().push(format!(
                "live actingcommand-actingd.exe {}",
                words.join(" ")
            ));
            self.checks.set(self.checks.get() + 1);
            return Ok(Ran {
                exit_code: Some(0),
                stdout: serde_json::to_vec(&self.check_line()).expect("JSON"),
                stderr: Vec::new(),
            });
        }
        self.fake.run_program_ending(program, arguments, timeout_ms)
    }

    fn log_dirs(&self, root: &Path) -> Vec<PathBuf> {
        self.fake.log_dirs(root)
    }

    fn after_step(&self, operation_id: &str, step: usize) -> Result<(), Outcome> {
        self.fake.after_step(operation_id, step)
    }

    fn between_renames(&self, operation_id: &str, step: usize) -> Result<(), Outcome> {
        self.fake.between_renames(operation_id, step)
    }

    fn before_retire(&self, operation_id: &str) -> Result<(), Outcome> {
        self.fake.before_retire(operation_id)
    }

    fn runtime(&self) -> &dyn RuntimeLink {
        self.fake.runtime()
    }

    fn pause_ms(&self, ms: u64) {
        self.fake.pause_ms(ms);
    }

    fn available_memory_mb(&self) -> Result<u64, Outcome> {
        self.fake.available_memory_mb()
    }

    fn runtime_read_bytes(&self, root: &Path) -> Option<u64> {
        self.fake.runtime_read_bytes(root)
    }

    fn runtime_working_set_mb(&self, root: &Path) -> Option<u64> {
        self.fake.runtime_working_set_mb(root)
    }

    fn detach(&self, arguments: &[String], directory: &Path, log: &Path) -> Result<u32, Outcome> {
        self.fake.detach(arguments, directory, log)
    }

    fn watchdog(&self, asked: crate::watchdog::Asked, root: &Path) -> crate::watchdog::Answered {
        self.fake.watchdog(asked, root)
    }

    fn detach_rescue(
        &self,
        arguments: &[String],
        environment: &[(&str, &str)],
        directory: &Path,
        log: &Path,
    ) -> Result<u32, Outcome> {
        self.fake
            .detach_rescue(arguments, environment, directory, log)
    }
}

/// An adopted root `<base>\<name>` with its default vision root `<root>\vision` (empty) and a
/// folder `<base>\<name>-from` laid out as a vision root that holds every file of [`PINS`].
struct Prepared {
    temp: TempDir,
    machine: Machine,
    root: PathBuf,
    state_root: PathBuf,
    vision: PathBuf,
    from: PathBuf,
}

fn prepared(name: &str) -> Prepared {
    let temp = TempDir::new().expect("tempdir");
    let fake = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), name, &BuildSpec::new(C1));
    let adopted = run(&fake, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    // The writer lock's file, which the first installer command that takes the lock creates: so
    // the installer's folder is compared whole before and after.
    fs::write(root.join("install").join("writer.lock"), b"").expect("the writer lock's file");
    let vision = root.join("vision");
    fs::create_dir_all(&vision).expect("the vision root");
    let from = temp.path().join(format!("{name}-from"));
    for pin in &PINS {
        write_at(&from, pin.path, pin.bytes);
    }
    let state_root = temp.path().join(format!("{name}-state"));
    Prepared {
        machine: Machine {
            fake,
            vision_root: vision.clone(),
            checks: Cell::new(0),
            identity_differs: Cell::new(false),
            unpinned: Cell::new(None),
        },
        temp,
        root,
        state_root,
        vision,
        from,
    }
}

/// [`prepared`] whose vision root holds every file of [`PINS`] but the dictionary, and the
/// recognizer with other bytes, which it returns: the placement places one file and replaces one.
fn half_placed(name: &str) -> (Prepared, Vec<u8>) {
    let prepared = prepared(name);
    for (index, pin) in PINS.iter().enumerate() {
        if index != KEYS {
            write_at(&prepared.vision, pin.path, pin.bytes);
        }
    }
    let old = b"a recognizer of another build".to_vec();
    write_at(&prepared.vision, PINS[REC].path, &old);
    (prepared, old)
}

/// The rescue row `name` with `params` on the prepared root, as [`WORD`]; its one line and exit
/// code.
fn rescue_line(prepared: &Prepared, name: &str, params: Value) -> (Value, u8) {
    let config = prepared.root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    let location = rescue::location_of(&prepared.root, &config, &state_root);
    let context = Context {
        host: &prepared.machine,
        location: &location,
        initiator: WORD,
    };
    let params = match params {
        Value::Object(params) => params,
        _ => Map::new(),
    };
    match rescue::answer_with(&context, name, &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("{name}: a line, not {other:?}"),
    }
}

/// `vision place --from <from>` on the prepared root.
fn vision_place(prepared: &Prepared, from: &Path) -> (Value, u8) {
    rescue_line(prepared, "vision place", json!({"from": word(from)}))
}

fn top_code(line: &Value) -> &str {
    line["outcome"]["code"].as_str().unwrap_or_default()
}

fn top_value<'a>(line: &'a Value, key: &str) -> &'a Value {
    &line["outcome"]["values"][key]
}

/// The names of an object's members, sorted.
fn keys(value: &Value) -> Vec<String> {
    let mut keys = value
        .as_object()
        .map(|members| members.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// Every journal of the root, by operation id.
fn journals(root: &Path) -> Vec<(String, Value)> {
    let directory = root.join("install").join("operations");
    let mut found = fs::read_dir(&directory)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let id = name.strip_suffix(".json")?.to_owned();
                    id.starts_with("op-").then(|| {
                        let journal =
                            serde_json::from_slice(&fs::read(entry.path()).expect("a journal"))
                                .expect("journal JSON");
                        (id, journal)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The one journal written since `before`: its operation id and the journal.
fn new_journal(root: &Path, before: &[(String, Value)]) -> (String, Value) {
    let mut written = journals(root)
        .into_iter()
        .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
        .collect::<Vec<_>>();
    assert_eq!(written.len(), 1, "{written:?}");
    written.remove(0)
}

/// The kinds of a journal's steps, in order.
fn step_kinds(journal: &Value) -> Vec<String> {
    journal["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .map(|step| step["kind"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The files of [`PINS`] as a tree, each at its place.
fn pinned_tree() -> BTreeMap<String, Vec<u8>> {
    PINS.iter()
        .map(|pin| (pin.path.to_owned(), pin.bytes.to_vec()))
        .collect()
}

/// A tree with the milliseconds of each kept name (`.old-<ms>`) written `<ms>`.
fn without_ms(files: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    files
        .into_iter()
        .map(|(path, bytes)| match path.find(".old-") {
            Some(at) => (format!("{}.old-<ms>", &path[..at]), bytes),
            None => (path, bytes),
        })
        .collect()
}

/// The milliseconds of an operation id (`op-<ms>`).
fn ms_of(operation_id: &str) -> &str {
    operation_id.strip_prefix("op-").expect("an operation id")
}

/// An owner record of a running Runtime, as `gate_inst1_update` writes one.
fn owner_record() -> Value {
    json!({
        "schema_version": "actingcommand.runtime-owner.v2",
        "revision": 1,
        "owner_epoch": "epoch_neutral",
        "pid": 100,
        "started_at_unix_ms": 1_700_000_000_000_u64,
        "active": true,
        "active_instances": [],
        "closed_at_unix_ms": null,
        "resource_disposition": "in_use",
    })
}

// An empty vision root but for a file the manifest does not name and another model folder; a
// `--from` that holds every manifest file and one more. Every manifest file is placed: the vision
// root is the manifest's files and what was there. One `vision_place` journal (the initiator, no
// plan, the inputs), one `vision_file` step per file in the manifest's order and `post_check`,
// whose one row passes; actingd's check ran twice (before and as the post-check). After each of
// the seven steps `current.json` names the operation, so watchdog row 7a″ stands aside; at the end
// none is unfinished. `--from` is read only, and its file the manifest does not name is counted.
#[test]
fn s9_missing_files_are_placed_and_the_post_check_passes() {
    let prepared = prepared("missing");
    write_at(&prepared.vision, "notes.txt", b"a note of the person's own");
    write_at(
        &prepared.vision,
        "models/neutral.other/model.onnx",
        b"another model of the person's own",
    );
    write_at(
        &prepared.from,
        "extra.bin",
        b"a file the manifest does not name",
    );
    let vision_before = tree(&prepared.vision);
    let from_before = tree(&prepared.from);
    let unfinished = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&unfinished);
    let watched = prepared.root.clone();
    prepared
        .machine
        .fake
        .on_step
        .replace(Some(Box::new(move |_step: usize| {
            seen.borrow_mut()
                .push(super::unfinished_operation(&watched));
        })));
    let before = journals(&prepared.root);

    let (placed, exit) = vision_place(&prepared, &prepared.from);

    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    assert_eq!(top_value(&placed, "operation"), "install_vision_place");
    assert_eq!(top_value(&placed, "installation_changed"), &json!(true));
    let mut expected = vision_before;
    expected.extend(pinned_tree());
    assert_eq!(tree(&prepared.vision), expected);
    assert_eq!(tree(&prepared.from), from_before, "--from is only read");
    assert_eq!(placed["ignored"], json!(1), "{placed}");
    assert_eq!(placed["provider"], "cpu", "{placed}");
    assert_eq!(
        placed["placed"].as_array().map(Vec::len),
        Some(PINS.len()),
        "{placed}"
    );
    assert_eq!(placed["checks"]["failed"], json!([]), "{placed}");
    assert_eq!(placed["checks"]["checks"][0]["result"], "pass", "{placed}");

    let (operation_id, journal) = new_journal(&prepared.root, &before);
    assert_eq!(
        top_value(&placed, "install_operation_id"),
        operation_id.as_str()
    );
    assert_eq!(
        placed["next"],
        json!({"undo": ["undo", operation_id]}),
        "{placed}"
    );
    assert_eq!(journal["kind"], "vision_place");
    assert_eq!(journal["initiator"], WORD.as_str());
    assert_eq!(journal["plan_id"], Value::Null);
    assert_eq!(
        keys(&journal["inputs"]),
        [
            "commit",
            "config_sha256",
            "from",
            "ignored",
            "provider",
            "vision_root"
        ]
    );
    let mut kinds = vec!["vision_file"; PINS.len()];
    kinds.push("post_check");
    assert_eq!(step_kinds(&journal), kinds);
    for (step, pin) in journal["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .zip(&PINS)
    {
        assert_eq!(step["intent"]["vision_root_path"], pin.path, "{step}");
        assert_eq!(step["intent"]["replaces"], json!(false), "{step}");
        assert_eq!(step["state"], "done", "{step}");
    }
    assert_eq!(prepared.machine.checks.get(), 2);

    let unfinished = unfinished.borrow();
    assert_eq!(unfinished.len(), PINS.len() + 1, "{unfinished:?}");
    let named = Some(Some(operation_id));
    assert!(
        unfinished.iter().all(|read| *read == named),
        "{unfinished:?}"
    );
    assert_eq!(super::unfinished_operation(&prepared.root), None);
}

// A vision root that holds every manifest file but the recognizer, which has other bytes. One
// `vision_file` step (it replaces) and `post_check`: the recognizer is the pinned one, and the one
// of before is kept beside it as `rec.onnx.old-<ms>`, `<ms>` the operation's own milliseconds,
// byte for byte, the answer naming it; nothing else changes.
#[test]
fn s9_a_differing_file_is_replaced_and_the_old_one_kept_byte_for_byte() {
    let prepared = prepared("differs");
    for pin in &PINS {
        write_at(&prepared.vision, pin.path, pin.bytes);
    }
    let old = b"a recognizer of another build".to_vec();
    write_at(&prepared.vision, PINS[REC].path, &old);
    let before = journals(&prepared.root);

    let (placed, exit) = vision_place(&prepared, &prepared.from);

    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let (operation_id, journal) = new_journal(&prepared.root, &before);
    assert_eq!(step_kinds(&journal), ["vision_file", "post_check"]);
    assert_eq!(journal["steps"][0]["intent"]["replaces"], json!(true));
    let kept_name = format!("{}.old-{}", PINS[REC].path, ms_of(&operation_id));
    let mut expected = pinned_tree();
    expected.insert(kept_name.clone(), old.clone());
    assert_eq!(tree(&prepared.vision), expected);
    let kept = below(&prepared.vision, &kept_name);
    assert_eq!(fs::read(&kept).expect("the kept recognizer"), old);
    assert_eq!(
        placed["placed"][0]["kept"],
        json!([super::files::plain(&kept)]),
        "{placed}"
    );
}

// A vision root that holds every manifest file: `installation_changed` false, no operation id,
// `placed` empty, actingd's report as `report`; actingd's check ran once; the root's whole tree,
// its installer folder and its vision root are as before, and no journal and no `current.json`
// were written.
#[test]
fn s9_a_complete_root_opens_no_operation_and_writes_nothing() {
    let prepared = prepared("complete");
    for pin in &PINS {
        write_at(&prepared.vision, pin.path, pin.bytes);
    }
    let root_before = tree(&prepared.root);
    let install_before = tree(&prepared.root.join("install"));
    let vision_before = tree(&prepared.vision);
    let before = journals(&prepared.root);

    let (answer, exit) = vision_place(&prepared, &prepared.from);

    assert_eq!(exit, 0, "{answer}");
    assert_eq!(top_code(&answer), "install_command_done", "{answer}");
    assert_eq!(top_value(&answer, "operation"), "install_vision_place");
    assert_eq!(top_value(&answer, "installation_changed"), &json!(false));
    assert_eq!(top_value(&answer, "install_operation_id"), &Value::Null);
    assert_eq!(answer["placed"], json!([]), "{answer}");
    assert_eq!(answer["ignored"], json!(0), "{answer}");
    assert_eq!(answer["report"]["provider"], "cpu", "{answer}");
    assert_eq!(prepared.machine.checks.get(), 1);
    assert_eq!(tree(&prepared.vision), vision_before);
    assert_eq!(tree(&prepared.root.join("install")), install_before);
    assert_eq!(tree(&prepared.root), root_before);
    assert_eq!(journals(&prepared.root), before);
    assert!(!prepared.root.join("install").join("current.json").exists());
}

// An empty vision root, and three folders laid out as vision roots, each holding every manifest
// file but the recognizer, which is in the first of another size, in the second of the same size
// and another SHA-256, and missing in the third. Each refuses `vision_source_invalid`, its path
// that file below the folder (`--from` resolved), the answer's `source` naming its place and its
// state; before anything is written: the vision root and the installer's folder as before, no
// journal.
#[test]
fn s9_a_wrong_or_missing_source_file_refuses_before_anything_is_written() {
    let prepared = prepared("source");
    let vision_before = tree(&prepared.vision);
    let install_before = tree(&prepared.root.join("install"));
    let before = journals(&prepared.root);
    let other_size: &[u8] = b"neutral recognizer and more";
    let same_size: &[u8] = b"NEUTRAL RECOGNIZER";
    assert_eq!(same_size.len(), PINS[REC].bytes.len());
    let cases = [
        ("size", Some(other_size), "sha256_mismatch"),
        ("digest", Some(same_size), "sha256_mismatch"),
        ("missing", None, "missing"),
    ];
    for (label, recognizer, state) in cases {
        let from = prepared.temp.path().join(format!("source-{label}"));
        for (index, pin) in PINS.iter().enumerate() {
            match (index == REC, recognizer) {
                (false, _) => write_at(&from, pin.path, pin.bytes),
                (true, Some(bytes)) => write_at(&from, pin.path, bytes),
                (true, None) => {}
            }
        }

        let (refused, exit) = vision_place(&prepared, &from);

        assert_eq!(exit, 1, "{label}: {refused}");
        assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
        assert_eq!(top_value(&refused, "operation"), "install_vision_place");
        assert_eq!(
            top_value(&refused, "install_refusal"),
            "vision_source_invalid",
            "{refused}"
        );
        let path = top_value(&refused, "path").as_str().unwrap_or_default();
        assert!(
            path.ends_with(r"models\neutral.ocr\rec.onnx")
                && path.contains(&format!("source-{label}")),
            "{label}: {refused}"
        );
        assert_eq!(refused["source"]["vision_root_path"], PINS[REC].path);
        assert_eq!(refused["source"]["state"], state, "{label}: {refused}");
        assert_eq!(tree(&prepared.vision), vision_before, "{label}");
        assert_eq!(
            tree(&prepared.root.join("install")),
            install_before,
            "{label}"
        );
        assert_eq!(journals(&prepared.root), before, "{label}");
    }
}

// An empty vision root and a good `--from`. An actingd under the root (a Runtime running) refuses
// `runtime_running`; the state root's owner lock held refuses `owner_lock_held`; an owner journal
// the probe cannot read (a folder where the file is) refuses `processes_unreadable`. Each under
// the writer lock, before anything is written: the vision root and the installer's folder as
// before, no journal, no `current.json`.
#[test]
fn s9_a_running_runtime_a_held_owner_lock_or_an_undecidable_probe_refuses() {
    let assert_refused = |prepared: &Prepared, refusal: &str| {
        let vision_before = tree(&prepared.vision);
        let install_before = tree(&prepared.root.join("install"));
        let before = journals(&prepared.root);
        let (refused, exit) = vision_place(prepared, &prepared.from);
        assert_eq!(exit, 1, "{refusal}: {refused}");
        assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
        assert_eq!(top_value(&refused, "operation"), "install_vision_place");
        assert_eq!(top_value(&refused, "install_refusal"), refusal, "{refused}");
        assert_eq!(tree(&prepared.vision), vision_before, "{refusal}");
        assert_eq!(
            tree(&prepared.root.join("install")),
            install_before,
            "{refusal}"
        );
        assert_eq!(journals(&prepared.root), before, "{refusal}");
        assert!(!prepared.root.join("install").join("current.json").exists());
    };

    let running = prepared("running");
    running.machine.fake.runtime.running.set(true);
    assert_refused(&running, "runtime_running");

    let locked = prepared("locked");
    let mut owner =
        fs::File::create(locked.state_root.join("owner.lock")).expect("the owner journal");
    owner
        .write_all(format!("{}\n", owner_record()).as_bytes())
        .expect("write the owner journal");
    owner.lock().expect("hold the owner lock");
    assert_refused(&locked, "owner_lock_held");
    drop(owner);

    let undecided = prepared("undecided");
    fs::create_dir_all(undecided.state_root.join("owner.lock"))
        .expect("a folder where the owner journal is");
    assert_refused(&undecided, "processes_unreadable");
}

/// Where a test stops the placement: after its step `n`, or between the two renames of its step
/// `n`.
#[derive(Debug, Clone, Copy)]
enum Crash {
    After(usize),
    Between(usize),
}

// A vision root that lacks the dictionary and holds another recognizer: step 1 places the
// dictionary, step 2 replaces the recognizer, step 3 is `post_check`. Placed uninterrupted, the
// vision root is the manifest's files and the old recognizer kept. Stopped after each step, and
// between the two renames of step 2 (the old recognizer renamed aside, the part not yet in its
// place), the command ends `install_operation_interrupted` with `current.json` naming the
// operation (watchdog row 7a″ stands aside); `op resume` finishes it, and the vision root is the
// tree of the uninterrupted placement, the kept file's milliseconds aside, the old recognizer kept
// byte for byte.
#[test]
fn s9_a_crash_after_each_step_or_between_the_renames_resumes_to_the_same_tree() {
    let (clean, old) = half_placed("clean");
    let (placed, exit) = vision_place(&clean, &clean.from);
    assert_eq!(exit, 0, "{placed}");
    let uninterrupted = without_ms(tree(&clean.vision));
    let mut expected = pinned_tree();
    expected.insert(format!("{}.old-<ms>", PINS[REC].path), old.clone());
    assert_eq!(uninterrupted, expected);

    for crash in [
        Crash::After(1),
        Crash::After(2),
        Crash::After(3),
        Crash::Between(2),
    ] {
        let (prepared, _) = half_placed(
            &format!("{crash:?}")
                .to_ascii_lowercase()
                .replace(['(', ')'], "-"),
        );
        match crash {
            Crash::After(step) => prepared.machine.fake.crash_after.set(Some(step)),
            Crash::Between(step) => prepared.machine.fake.crash_between_renames.set(Some(step)),
        }
        let before = journals(&prepared.root);

        let (interrupted, exit) = vision_place(&prepared, &prepared.from);

        assert_eq!(exit, 2, "{crash:?}: {interrupted}");
        assert_eq!(
            top_code(&interrupted),
            "install_operation_interrupted",
            "{crash:?}: {interrupted}"
        );
        let (operation_id, _) = new_journal(&prepared.root, &before);
        assert_eq!(
            super::unfinished_operation(&prepared.root),
            Some(Some(operation_id.clone())),
            "{crash:?}"
        );

        let (resumed, exit) = rescue_line(&prepared, "op resume", json!({"op": operation_id}));

        assert_eq!(exit, 0, "{crash:?}: {resumed}");
        assert_eq!(top_code(&resumed), "install_command_done", "{resumed}");
        assert_eq!(top_value(&resumed, "operation"), "install_resume");
        assert_eq!(
            without_ms(tree(&prepared.vision)),
            uninterrupted,
            "{crash:?}"
        );
        assert_eq!(
            super::unfinished_operation(&prepared.root),
            None,
            "{crash:?}"
        );
    }
}

// A finished placement of the dictionary and the recognizer, undone: the vision root holds what it
// held before (the old recognizer back in its place, no dictionary), and each file the placement
// placed is kept beside its place as `.rolledback-<ms2>`; nothing is deleted.
#[test]
fn s9_undo_sets_the_placed_files_aside_and_puts_the_replaced_file_back() {
    let (prepared, _) = half_placed("undo");
    let vision_before = tree(&prepared.vision);
    let before = journals(&prepared.root);
    let (placed, exit) = vision_place(&prepared, &prepared.from);
    assert_eq!(exit, 0, "{placed}");
    let (operation_id, _) = new_journal(&prepared.root, &before);

    let (undone, exit) = rescue_line(&prepared, "undo", json!({"op": operation_id}));

    assert_eq!(exit, 0, "{undone}");
    assert_eq!(top_code(&undone), "install_command_done", "{undone}");
    assert_eq!(top_value(&undone, "operation"), "install_undo");
    let after = tree(&prepared.vision);
    let restored = after
        .iter()
        .filter(|(path, _)| !path.contains(".rolledback-"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(restored, vision_before);
    let aside = after
        .iter()
        .filter(|(path, _)| path.contains(".rolledback-"))
        .map(|(path, bytes)| {
            (
                path.split(".rolledback-")
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                bytes.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let expected = [KEYS, REC]
        .iter()
        .map(|index| (PINS[*index].path.to_owned(), PINS[*index].bytes.to_vec()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(aside, expected);
    assert_eq!(after.len(), vision_before.len() + 2, "{:?}", after.keys());
}

// Another folder under `models\` and a file the manifest does not name, in the vision root, and
// below `--from` the same places with other bytes and one more model folder. After the placement
// the vision root's own two are byte for byte as before, nothing of `--from`'s three is there, and
// the answer counts the three as ignored.
#[test]
fn s9_another_model_folder_and_an_unnamed_file_are_untouched() {
    let prepared = prepared("untouched");
    let other_model = "models/neutral.other/model.onnx";
    let unnamed = "ort/extra.dll";
    write_at(&prepared.vision, other_model, b"the other model of before");
    write_at(&prepared.vision, unnamed, b"an unnamed file of before");
    write_at(
        &prepared.from,
        other_model,
        b"another build of the other model",
    );
    write_at(&prepared.from, unnamed, b"another unnamed file");
    write_at(
        &prepared.from,
        "models/neutral.third/model.onnx",
        b"a third model",
    );
    let vision_before = tree(&prepared.vision);

    let (placed, exit) = vision_place(&prepared, &prepared.from);

    assert_eq!(exit, 0, "{placed}");
    assert_eq!(placed["ignored"], json!(3), "{placed}");
    assert_eq!(
        fs::read(below(&prepared.vision, other_model)).expect("the other model"),
        b"the other model of before"
    );
    assert_eq!(
        fs::read(below(&prepared.vision, unnamed)).expect("the unnamed file"),
        b"an unnamed file of before"
    );
    assert!(!below(&prepared.vision, "models/neutral.third").exists());
    let mut expected = vision_before;
    expected.extend(pinned_tree());
    assert_eq!(tree(&prepared.vision), expected);
}

/// The outcome's links, each `(relation, code)`.
fn links(line: &Value) -> Vec<(String, String)> {
    line["outcome"]["causes"]
        .as_array()
        .map(|causes| {
            causes
                .iter()
                .map(|cause| {
                    (
                        cause["relation"].as_str().unwrap_or_default().to_owned(),
                        cause["code"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

// Closing round: a vision root that lacks the dictionary and holds another recognizer, stopped
// between the two renames of step 2 (the old recognizer renamed `.old-<ms>`, its place empty, the
// part waiting). `undo` of the unfinished operation renames the old recognizer back into its place,
// sets the waiting part aside as `rec.onnx.abandoned-<ms2>` and the dictionary step 1 placed as
// `keys.txt.rolledback-<ms2>`: the vision root is as before but for those two, nothing is deleted,
// no `.old-` name is left and nothing is unfinished.
#[test]
fn s9_undo_after_a_crash_between_the_two_renames_puts_the_old_file_back() {
    let (prepared, _) = half_placed("between-undo");
    let vision_before = tree(&prepared.vision);
    prepared.machine.fake.crash_between_renames.set(Some(2));
    let before = journals(&prepared.root);
    let (interrupted, exit) = vision_place(&prepared, &prepared.from);
    assert_eq!(exit, 2, "{interrupted}");
    assert_eq!(
        top_code(&interrupted),
        "install_operation_interrupted",
        "{interrupted}"
    );
    let (operation_id, _) = new_journal(&prepared.root, &before);
    assert!(
        !below(&prepared.vision, PINS[REC].path).exists(),
        "stopped between the renames: the recognizer's place is empty"
    );

    let (undone, exit) = rescue_line(&prepared, "undo", json!({"op": operation_id}));

    assert_eq!(exit, 0, "{undone}");
    assert_eq!(top_code(&undone), "install_command_done", "{undone}");
    assert_eq!(top_value(&undone, "operation"), "install_undo");
    let after = tree(&prepared.vision);
    let set_aside = |marker: &str| {
        after
            .iter()
            .filter(|(path, _)| path.contains(marker))
            .map(|(path, bytes)| {
                (
                    path.split(marker).next().unwrap_or_default().to_owned(),
                    bytes.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let restored = after
        .iter()
        .filter(|(path, _)| !path.contains(".rolledback-") && !path.contains(".abandoned-"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(restored, vision_before);
    assert_eq!(
        set_aside(".rolledback-"),
        BTreeMap::from([(PINS[KEYS].path.to_owned(), PINS[KEYS].bytes.to_vec())])
    );
    assert_eq!(
        set_aside(".abandoned-"),
        BTreeMap::from([(PINS[REC].path.to_owned(), PINS[REC].bytes.to_vec())])
    );
    assert!(
        after.keys().all(|path| !path.contains(".old-")),
        "{:?}",
        after.keys()
    );
    assert_eq!(after.len(), vision_before.len() + 2, "{:?}", after.keys());
    assert_eq!(super::unfinished_operation(&prepared.root), None);
}

// Closing round: an empty vision root, every file placed, and the check after the placement finds
// the manifest's model folder of another identity. The command ends `post_check_failed` (`count`
// 1, the operation's id), its answer's checks failing the row `vision_check` with the model in
// `models_unmatched` and no file `not_ok`, and `next` naming the operation's `undo`. What is left:
// the files placed, the operation ended (its result recorded, its `post_check` step closed with
// what it read), no `current.json`, so nothing is unfinished.
#[test]
fn s9_a_failed_check_after_the_placement_ends_post_check_failed_with_the_files_placed() {
    let prepared = prepared("failed-check");
    prepared.machine.identity_differs.set(true);
    let vision_before = tree(&prepared.vision);
    let before = journals(&prepared.root);

    let (refused, exit) = vision_place(&prepared, &prepared.from);

    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(top_value(&refused, "operation"), "install_vision_place");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "post_check_failed",
        "{refused}"
    );
    assert_eq!(top_value(&refused, "count"), &json!(1), "{refused}");
    let (operation_id, journal) = new_journal(&prepared.root, &before);
    assert_eq!(
        top_value(&refused, "install_operation_id"),
        operation_id.as_str()
    );
    assert_eq!(
        refused["checks"]["failed"],
        json!(["vision_check"]),
        "{refused}"
    );
    let row = &refused["checks"]["checks"][0];
    assert_eq!(row["result"], "fail", "{row}");
    assert_eq!(row["values"]["models_unmatched"], json!([MODEL]), "{row}");
    assert_eq!(row["values"]["not_ok"], json!([]), "{row}");
    assert_eq!(
        refused["next"],
        json!({"undo": ["undo", operation_id]}),
        "{refused}"
    );

    let mut expected = vision_before;
    expected.extend(pinned_tree());
    assert_eq!(tree(&prepared.vision), expected);
    assert!(journal["finished_at_unix_ms"].is_u64(), "{journal}");
    assert_eq!(
        journal["result"]["code"], "install_command_refused",
        "{journal}"
    );
    let steps = journal["steps"].as_array().expect("the steps");
    assert_eq!(steps.len(), PINS.len() + 1);
    assert!(
        steps.iter().all(|step| step["state"] == "done"),
        "{journal}"
    );
    let check = steps.last().expect("the post_check step");
    assert_eq!(check["kind"], "post_check");
    assert_eq!(
        check["evidence"]["failed"],
        json!(["vision_check"]),
        "{check}"
    );
    assert!(!prepared.root.join("install").join("current.json").exists());
    assert_eq!(super::unfinished_operation(&prepared.root), None);
    assert_eq!(prepared.machine.checks.get(), 2);
}

// Closing round (the work order's ruling 8): a report that lists the ONNX Runtime license file
// without a pin (`pinned` false, no size, no SHA-256). The command refuses
// `vision_report_unanswered`, caused by `foreign_data_error`, before anything is written: actingd's
// check ran once, the vision root and the installer folder are as before, no journal, no
// `current.json`, and nothing of `--from` was copied.
#[test]
fn s9_a_file_the_report_lists_unpinned_refuses_and_nothing_is_copied() {
    let prepared = prepared("unpinned");
    let license = PINS
        .iter()
        .position(|pin| pin.license)
        .expect("a license file");
    prepared.machine.unpinned.set(Some(license));
    let vision_before = tree(&prepared.vision);
    let install_before = tree(&prepared.root.join("install"));
    let before = journals(&prepared.root);

    let (refused, exit) = vision_place(&prepared, &prepared.from);

    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(top_value(&refused, "operation"), "install_vision_place");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "vision_report_unanswered",
        "{refused}"
    );
    assert_eq!(
        links(&refused),
        [("caused_by".to_owned(), "foreign_data_error".to_owned())],
        "{refused}"
    );
    assert_eq!(prepared.machine.checks.get(), 1);
    assert_eq!(tree(&prepared.vision), vision_before);
    assert_eq!(tree(&prepared.root.join("install")), install_before);
    assert_eq!(journals(&prepared.root), before);
    assert!(!prepared.root.join("install").join("current.json").exists());
}
