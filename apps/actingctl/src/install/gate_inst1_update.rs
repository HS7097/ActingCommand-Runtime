// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #421 INST-1, test rows IT-5 to IT-12 of the frozen model INST-M v1.0 (§12, "Update
// plan", "Update apply", "Rollback"). Every expected value comes from the model (§4.4, §5, §6.1
// to §6.5, §7) and its contract (contracts/install-cli.md, "Update and rollback", "Interface
// checks", "Safety", "Documents"):
// - IT-5: the interface edges refuse a ledger the target cannot read, a one-way ledger without
//   `--accept-ledger-one-way` (planned and journalled with it), a placed bundle the target
//   cannot run; an unknown declaration is a warning;
// - IT-6: the plan refuses an A/B root, a running Runtime (`--preview` reports it), a held owner
//   lock, a foreign `.part`, a running image other than actingctl.exe that would change, and a
//   running adb.exe that would change;
// - IT-7: apply with a stale plan id is `install_plan_changed` and changes nothing;
// - IT-8: apply swaps Tools before the Runtime, never touches an unchanged file, renames a
//   running actingctl.exe aside while it stays open, keeps the old bytes as `.old-<ms>`,
//   verifies every manifest file, records the state, and leaves extra live files;
// - IT-9: after a stop at each step, `resume` reaches the uninterrupted tree and `undo` restores
//   the old tree byte for byte with every displaced file kept;
// - IT-10: the writer lock is held through every step, a second installer meets
//   `install_operation_unfinished`, a watchdog tick meanwhile is `watchdog_installer_busy`, and
//   after a stop with `current.json` present the tick is `watchdog_installer_unfinished` and
//   `actingctl start` refuses;
// - IT-11: when the ledger ranges differ the target's verification runs before any swap, and a
//   failing one refuses with nothing swapped;
// - IT-12: the rollback gate refuses an unreadable page (with its sequence), an open estimate
//   over the deadline minus the margin, the target's check-config refusal and a target without
//   an open deadline; it passes otherwise, and is evaluated again at apply.
// The machine is the fake host of `fixture.rs`; every root is a temporary folder with neutral
// files.

use super::fixture::{
    BuildSpec, C1, C2, FakeHost, code, live_root, run, sha256_hex, tree, word, write_json,
};
use super::host::RunningImage;
use crate::watchdog::decide::LiveOwner;
use crate::watchdog::observe::{self, HostProbe, WriterProbe};
use crate::watchdog::start::{Attempt, StartedProcess, Target, TickSeams};
use actingcommand_contract::outcome::{Category, Outcome};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use tempfile::TempDir;

/// A root whose live build is `live` (adopted) and that keeps `target` (staged from its release
/// folder as a local build).
fn prepared(
    base: &Path,
    name: &str,
    live: &BuildSpec,
    target: &BuildSpec,
    host: &FakeHost,
) -> PathBuf {
    let root = live_root(base, name, live);
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_root_adopted", "{adopted:?}");
    let release = base.join(format!("{name}-release"));
    target.write_release(&release);
    let staged = run(
        host,
        &[
            "release",
            "stage",
            "--root",
            word(&root),
            "--dir",
            word(&release),
            "--local-build",
        ],
    );
    assert_eq!(code(&staged), "install_build_staged", "{staged:?}");
    root
}

fn plan_update(host: &FakeHost, root: &Path, commit: &str, extra: &[&str]) -> super::Answer {
    let mut words = vec!["update", "plan", "--root", word(root), "--build", commit];
    words.extend_from_slice(extra);
    run(host, &words)
}

fn plan_id(answer: &super::Answer) -> String {
    answer.members["plan_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a plan id: {answer:?}"))
        .to_owned()
}

fn apply_update(
    host: &FakeHost,
    root: &Path,
    commit: &str,
    plan: &str,
    extra: &[&str],
) -> super::Answer {
    let mut words = vec![
        "update",
        "apply",
        "--root",
        word(root),
        "--build",
        commit,
        "--plan",
        plan,
    ];
    words.extend_from_slice(extra);
    run(host, &words)
}

fn refusal(answer: &super::Answer) -> Option<&str> {
    answer
        .outcome
        .value("install_refusal")
        .and_then(Value::as_str)
}

fn link_codes(answer: &super::Answer) -> Vec<String> {
    answer
        .outcome
        .causes()
        .iter()
        .map(|link| link.outcome().code().as_str().to_owned())
        .collect()
}

/// The root's ordinary spelling, as the process probe names images.
fn plain_root(root: &Path) -> String {
    let canonical = fs::canonicalize(root).expect("the canonical root");
    let text = canonical.display().to_string();
    text.strip_prefix(r"\\?\")
        .map_or(text.clone(), str::to_owned)
}

fn image(root: &Path, relative: &str, pid: u32, mcp_serve: bool) -> RunningImage {
    RunningImage {
        pid,
        path: format!("{}\\{}", plain_root(root), relative.replace('/', "\\")),
        mcp_serve,
    }
}

/// The program files of `runtime\` and `tools\`, by path relative to the root.
fn program_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for half in ["runtime", "tools"] {
        for (path, bytes) in tree(&root.join(half)) {
            files.insert(format!("{half}/{path}"), bytes);
        }
    }
    files
}

/// `program_tree` with the milliseconds of every kept name replaced by `*`.
fn normalised(root: &Path) -> BTreeMap<String, Vec<u8>> {
    program_tree(root)
        .into_iter()
        .map(|(path, bytes)| {
            let mut name = String::new();
            let mut rest = path.as_str();
            while let Some(index) = [".old-", ".rolledback-", ".abandoned-", ".new-"]
                .iter()
                .filter_map(|marker| rest.find(marker).map(|at| at + marker.len()))
                .min()
            {
                name.push_str(&rest[..index]);
                name.push('*');
                rest =
                    rest[index..].trim_start_matches(|character: char| character.is_ascii_digit());
            }
            name.push_str(rest);
            (name, bytes)
        })
        .collect()
}

/// The listed files of `spec` as a live root holds them.
fn spec_tree(spec: &BuildSpec) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for (half, part) in [("runtime", &spec.runtime), ("tools", &spec.tools)] {
        for (path, bytes) in part.entries() {
            files.insert(format!("{half}/{path}"), bytes);
        }
    }
    files
}

fn journal_of(root: &Path, operation_id: &str) -> Value {
    serde_json::from_slice(
        &fs::read(
            root.join("install")
                .join("operations")
                .join(format!("{operation_id}.json")),
        )
        .expect("the journal"),
    )
    .expect("journal JSON")
}

fn operation_id(answer: &super::Answer) -> String {
    answer
        .outcome
        .value("install_operation_id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an operation id: {answer:?}"))
        .to_owned()
}

fn set_speaks(spec: &mut BuildSpec, interface: &str, range: Value) {
    spec.runtime.speaks[interface] = range.clone();
    spec.tools.speaks[interface] = range;
}

fn set_packs(root: &Path, packs: Value) {
    let path = root.join("install").join("state.json");
    let mut state: Value =
        serde_json::from_slice(&fs::read(&path).expect("state.json")).expect("state JSON");
    state["packs"] = packs;
    write_json(&path, &state);
}

// IT-5 (table): the update's interface edges.
#[test]
fn inst1_it_5_the_update_plan_refuses_failed_edges_and_warns_an_unknown_declaration() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);

    // The live build's ledger maximum is not in the target's range.
    let mut narrower = BuildSpec::new(C2);
    set_speaks(&mut narrower, "ledger", json!([3, 3]));
    let root = prepared(temp.path(), "narrower", &live, &narrower, &host);
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(code(&refused), "install_update_refused", "{refused:?}");
    assert_eq!(refusal(&refused), Some("interface_edge"));

    // A one-way ledger: refused without the flag, planned and journalled with it.
    let mut one_way = BuildSpec::new(C2);
    set_speaks(&mut one_way, "ledger", json!([3, 5]));
    let root = prepared(temp.path(), "one-way", &live, &one_way, &host);
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(code(&refused), "install_update_refused", "{refused:?}");
    assert_eq!(refusal(&refused), Some("interface_edge"));
    let planned = plan_update(&host, &root, C2, &["--accept-ledger-one-way"]);
    assert_eq!(code(&planned), "install_update_planned", "{planned:?}");
    let applied = apply_update(
        &host,
        &root,
        C2,
        &plan_id(&planned),
        &["--accept-ledger-one-way"],
    );
    assert_eq!(code(&applied), "install_update_applied", "{applied:?}");
    let journal = journal_of(&root, &operation_id(&applied));
    assert_eq!(
        journal["acknowledged"],
        json!(["accept_ledger_one_way"]),
        "{journal}"
    );

    // A placed bundle whose package maximum the target does not run.
    let root = prepared(temp.path(), "bundle", &live, &BuildSpec::new(C2), &host);
    set_packs(
        &root,
        json!([{"game": "neutral", "repository": null, "commit": null, "tag": null,
            "package_range": [6, 6]}]),
    );
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(code(&refused), "install_update_refused", "{refused:?}");
    assert_eq!(refusal(&refused), Some("interface_edge"));

    // An unknown declaration is a warning; the plan stands.
    set_packs(
        &root,
        json!([{"game": "neutral", "repository": null, "commit": null, "tag": null,
            "package_range": null}]),
    );
    let planned = plan_update(&host, &root, C2, &[]);
    assert_eq!(code(&planned), "install_update_planned", "{planned:?}");
    assert!(
        link_codes(&planned).contains(&"install_pack_declaration_unknown".to_owned()),
        "{planned:?}"
    );
}

// IT-6: the plan's refusals; `--preview` reports a running Runtime instead.
#[test]
fn inst1_it_6_the_update_plan_refuses_what_may_not_change() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);
    let target = BuildSpec::new(C2);

    // An A/B root.
    let root = prepared(temp.path(), "ab", &live, &target, &host);
    fs::write(root.join("install").join("active.json"), b"{}").expect("a selection");
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(
        code(&refused),
        "install_layout_ab_unsupported",
        "{refused:?}"
    );

    // A Runtime runs under the root; `--preview` reports it.
    let root = prepared(temp.path(), "running", &live, &target, &host);
    host.processes.replace(vec![image(
        &root,
        "runtime/actingcommand-actingd.exe",
        41,
        false,
    )]);
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(refusal(&refused), Some("runtime_running"), "{refused:?}");
    let previewed = plan_update(&host, &root, C2, &["--preview"]);
    assert_eq!(code(&previewed), "install_update_planned", "{previewed:?}");
    assert_eq!(previewed.members["running"]["runtime"][0]["pid"], json!(41));
    host.processes.replace(Vec::new());

    // The owner lock is held.
    let root = prepared(temp.path(), "owner", &live, &target, &host);
    let owner = File::create(temp.path().join("owner-state").join("owner.lock"))
        .expect("the owner journal");
    owner.lock().expect("hold the owner lock");
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(refusal(&refused), Some("owner_lock_held"), "{refused:?}");
    drop(owner);

    // A part another operation left.
    let root = prepared(temp.path(), "part", &live, &target, &host);
    fs::write(
        root.join("runtime")
            .join("actingctl.exe.new-1700000000000.part"),
        b"part",
    )
    .expect("a foreign part");
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(code(&refused), "install_foreign_part", "{refused:?}");

    // A running image other than actingctl.exe that would change.
    let root = prepared(temp.path(), "ledger-reader", &live, &target, &host);
    host.processes
        .replace(vec![image(&root, "tools/actingledger.exe", 51, false)]);
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(
        code(&refused),
        "install_running_image_blocks",
        "{refused:?}"
    );
    host.processes.replace(Vec::new());

    // A running adb.exe that would change.
    let mut other_bridge = BuildSpec::new(C2);
    for (path, bytes) in &mut other_bridge.tools.files {
        if path.as_str() == "platform-tools/adb.exe" {
            *bytes = b"neutral bridge, another revision".to_vec();
        }
    }
    let root = prepared(temp.path(), "bridge", &live, &other_bridge, &host);
    host.processes.replace(vec![image(
        &root,
        "tools/platform-tools/adb.exe",
        61,
        false,
    )]);
    let refused = plan_update(&host, &root, C2, &[]);
    assert_eq!(
        code(&refused),
        "install_running_image_blocks",
        "{refused:?}"
    );
    host.processes.replace(Vec::new());
}

// IT-7: a stale plan id refuses apply and changes nothing.
#[test]
fn inst1_it_7_apply_with_a_stale_plan_changes_nothing() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(
        temp.path(),
        "root",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    let planned = plan_update(&host, &root, C2, &[]);
    assert_eq!(code(&planned), "install_update_planned", "{planned:?}");
    // The configuration changes after the plan.
    let config = root.join("actingd.config.json");
    let mut document: Value =
        serde_json::from_slice(&fs::read(&config).expect("the configuration")).expect("JSON");
    document["bind_port"] = json!(0);
    write_json(&config, &document);
    let before = program_tree(&root);
    let operations = fs::read_dir(root.join("install").join("operations"))
        .expect("operations")
        .count();

    let refused = apply_update(&host, &root, C2, &plan_id(&planned), &[]);
    assert_eq!(code(&refused), "install_plan_changed", "{refused:?}");
    assert_eq!(program_tree(&root), before, "nothing changed");
    assert_eq!(
        fs::read_dir(root.join("install").join("operations"))
            .expect("operations")
            .count(),
        operations,
        "no operation opened"
    );
}

// IT-8: an apply.
#[test]
fn inst1_it_8_apply_swaps_tools_first_and_keeps_and_leaves_what_it_must() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);
    let mut target = BuildSpec::new(C2);
    target
        .runtime
        .files
        .push(("INSTALL.md".to_owned(), b"neutral notes".to_vec()));
    let root = prepared(temp.path(), "root", &live, &target, &host);
    fs::write(
        root.join("runtime").join("notes.txt"),
        b"an extra live file",
    )
    .expect("an extra file");
    let unchanged = root.join("tools").join("actingwatch.exe");
    let unchanged_before = fs::metadata(&unchanged).expect("the unchanged file");
    host.processes
        .replace(vec![image(&root, "runtime/actingctl.exe", 77, true)]);
    let mut open = File::open(root.join("runtime").join("actingctl.exe")).expect("an open image");

    let planned = plan_update(&host, &root, C2, &[]);
    assert_eq!(code(&planned), "install_update_planned", "{planned:?}");
    assert!(
        planned.members["extra"]
            .as_array()
            .expect("extra")
            .contains(&json!("runtime/notes.txt")),
        "{planned:?}"
    );
    let applied = apply_update(&host, &root, C2, &plan_id(&planned), &[]);
    assert_eq!(code(&applied), "install_update_applied", "{applied:?}");
    assert!(
        link_codes(&applied).contains(&"install_mcp_clients_on_previous_build".to_owned()),
        "{applied:?}"
    );
    assert_eq!(applied.members["running_renamed"][0]["pid"], json!(77));
    assert_eq!(
        applied.members["running_renamed"][0]["mcp_serve"],
        json!(true)
    );

    // Tools before the Runtime.
    let journal = journal_of(&root, &operation_id(&applied));
    let swaps = journal["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .filter(|step| step["kind"] == "swap_file")
        .map(|step| step["target"].as_str().expect("a target").to_owned())
        .collect::<Vec<_>>();
    let first_runtime = swaps
        .iter()
        .position(|path| path.starts_with("runtime/"))
        .expect("a runtime swap");
    assert!(
        swaps[..first_runtime]
            .iter()
            .all(|path| path.starts_with("tools/"))
            && swaps[first_runtime..]
                .iter()
                .all(|path| path.starts_with("runtime/")),
        "{swaps:?}"
    );
    // The unchanged file is the same file.
    let unchanged_after = fs::metadata(&unchanged).expect("the unchanged file");
    assert_eq!(
        unchanged_before.created().ok(),
        unchanged_after.created().ok()
    );
    assert_eq!(
        unchanged_before.modified().ok(),
        unchanged_after.modified().ok()
    );
    assert!(
        !fs::read_dir(root.join("tools"))
            .expect("tools")
            .any(|entry| entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .starts_with("actingwatch.exe.old-")),
        "no copy of an unchanged file"
    );
    // The running image was renamed aside and still reads its old bytes.
    let old_control = live
        .runtime
        .entries()
        .into_iter()
        .find(|(path, _)| path == "actingctl.exe")
        .expect("the old control")
        .1;
    let mut read = Vec::new();
    open.read_to_end(&mut read).expect("read the open image");
    assert_eq!(read, old_control);
    let kept = fs::read_dir(root.join("runtime"))
        .expect("runtime")
        .map(|entry| entry.expect("an entry").path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("actingctl.exe.old-"))
        })
        .expect("the kept old image");
    assert_eq!(fs::read(kept).expect("the kept old image"), old_control);
    // Every manifest file verified, the state recorded, the extra file left.
    let listed = target.runtime.files.len() + target.tools.files.len();
    assert_eq!(applied.members["verified"], json!(listed));
    for (path, bytes) in spec_tree(&target) {
        assert_eq!(
            fs::read(root.join(&path)).ok(),
            Some(bytes),
            "{path} is the target's"
        );
    }
    let state: Value = serde_json::from_slice(
        &fs::read(root.join("install").join("state.json")).expect("state.json"),
    )
    .expect("state JSON");
    assert_eq!(state["live"]["commit"], json!(C2), "{state}");
    assert_eq!(
        state["live"]["operation_id"],
        json!(operation_id(&applied)),
        "{state}"
    );
    assert!(root.join("runtime").join("notes.txt").is_file());
}

// IT-9 (fault hook after each step): `resume` reaches the uninterrupted tree; `undo` restores the
// old tree byte for byte and keeps every displaced file.
#[test]
fn inst1_it_9_resume_and_undo_after_a_stop_at_each_step() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);
    let target = BuildSpec::new(C2);
    // The uninterrupted apply.
    let whole = prepared(temp.path(), "whole", &live, &target, &host);
    let applied = apply_update(
        &host,
        &whole,
        C2,
        &plan_id(&plan_update(&host, &whole, C2, &[])),
        &[],
    );
    assert_eq!(code(&applied), "install_update_applied", "{applied:?}");
    let reference = normalised(&whole);
    let steps = journal_of(&whole, &operation_id(&applied))["steps"]
        .as_array()
        .expect("steps")
        .len();
    assert!(steps > 2, "{steps}");
    for step in 1..=steps {
        // Stopped after `step`, then resumed.
        let root = prepared(
            temp.path(),
            &format!("resume-{step}"),
            &live,
            &target,
            &host,
        );
        let planned = plan_id(&plan_update(&host, &root, C2, &[]));
        host.crash_after.set(Some(step));
        let stopped = apply_update(&host, &root, C2, &planned, &[]);
        assert_eq!(
            code(&stopped),
            "install_operation_interrupted",
            "{step}: {stopped:?}"
        );
        let resumed = run(
            &host,
            &[
                "resume",
                "--root",
                word(&root),
                "--operation",
                operation_id(&stopped).as_str(),
            ],
        );
        assert_eq!(
            code(&resumed),
            "install_operation_resumed",
            "{step}: {resumed:?}"
        );
        assert_eq!(normalised(&root), reference, "{step}: the resumed tree");

        // Stopped after `step`, then undone.
        let root = prepared(temp.path(), &format!("undo-{step}"), &live, &target, &host);
        let planned = plan_id(&plan_update(&host, &root, C2, &[]));
        host.crash_after.set(Some(step));
        let stopped = apply_update(&host, &root, C2, &planned, &[]);
        assert_eq!(
            code(&stopped),
            "install_operation_interrupted",
            "{step}: {stopped:?}"
        );
        let displaced = program_tree(&root)
            .values()
            .map(|bytes| sha256_hex(bytes))
            .collect::<BTreeSet<_>>();
        let undone = run(
            &host,
            &[
                "undo",
                "--root",
                word(&root),
                "--operation",
                operation_id(&stopped).as_str(),
            ],
        );
        assert_eq!(
            code(&undone),
            "install_operation_undone",
            "{step}: {undone:?}"
        );
        let after = program_tree(&root);
        for (path, bytes) in spec_tree(&live) {
            assert_eq!(
                after.get(&path),
                Some(&bytes),
                "{step}: {path} is the old one"
            );
        }
        let kept = after
            .values()
            .map(|bytes| sha256_hex(bytes))
            .collect::<BTreeSet<_>>();
        assert!(
            displaced.is_subset(&kept),
            "{step}: every displaced file is kept"
        );
        assert!(
            !root.join("install").join("current.json").exists(),
            "{step}"
        );
    }
}

/// A machine for the watchdog tick: nothing runs under the root, and nothing may be started.
struct Machine;

impl TickSeams for Machine {
    fn now_unix_ms(&self) -> u64 {
        1_800_000_000_000
    }

    fn monotonic(&self) -> Duration {
        Duration::ZERO
    }

    fn pause(&self, _interval: Duration) {}

    fn host_probe(&self, _root_plain: &Path) -> Result<HostProbe, Outcome> {
        Ok(HostProbe {
            processes: Vec::new(),
            boot_unix_ms: Some(1_700_000_000_000),
        })
    }

    fn spawn(
        &self,
        _target: &Target<'_>,
        _entry: &Path,
        _log_file: File,
        _log: &Path,
        _from_task: bool,
    ) -> Result<(&'static str, Box<dyn StartedProcess>), Attempt> {
        panic!("no Runtime may be started while the installer works or is unfinished")
    }

    fn answering_owner(&self, _state_root: &Path) -> Option<LiveOwner> {
        None
    }
}

/// An owner journal whose last record is active: a Runtime that ended without a close, which
/// the watchdog would start.
fn crashed_owner(state_root: &Path) {
    let record = json!({
        "schema_version": "actingcommand.runtime-owner.v2",
        "revision": 1,
        "owner_epoch": "epoch_neutral",
        "pid": 100,
        "started_at_unix_ms": 1_700_000_000_000_u64,
        "active": true,
        "active_instances": [],
        "closed_at_unix_ms": null,
        "resource_disposition": "in_use",
    });
    fs::write(state_root.join("owner.lock"), format!("{record}\n")).expect("the owner journal");
}

// IT-10: the writer lock through every step, a second installer, a watchdog tick meanwhile, and
// after a stop with `current.json` present.
#[test]
fn inst1_it_10_the_writer_lock_and_the_watchdog_stand_by_the_installer() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(
        temp.path(),
        "root",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    crashed_owner(&temp.path().join("root-state"));
    let planned = plan_id(&plan_update(&host, &root, C2, &[]));
    let seen = Rc::new(std::cell::RefCell::new(Vec::new()));
    {
        let seen = Rc::clone(&seen);
        let root = root.clone();
        let planned = planned.clone();
        let work = temp.path().join("second");
        host.on_step.replace(Some(Box::new(move |step: usize| {
            let busy = matches!(observe::writer_lock(&root), Ok(WriterProbe::Busy));
            let second = FakeHost::new(&work);
            let other = apply_update(&second, &root, C2, &planned, &[]);
            let tick = crate::watchdog::tick_for_test(&Machine, &root);
            seen.borrow_mut().push((
                step,
                busy,
                code(&other).to_owned(),
                tick.code().as_str().to_owned(),
            ));
        })));
    }
    let applied = apply_update(&host, &root, C2, &planned, &[]);
    host.on_step.replace(None);
    assert_eq!(code(&applied), "install_update_applied", "{applied:?}");
    let seen = seen.borrow();
    let steps = journal_of(&root, &operation_id(&applied))["steps"]
        .as_array()
        .expect("steps")
        .len();
    assert_eq!(seen.len(), steps, "every step was observed");
    for (step, busy, other, tick) in seen.iter() {
        assert!(*busy, "step {step}: the writer lock is held");
        assert_eq!(other, "install_operation_unfinished", "step {step}");
        assert_eq!(tick, "watchdog_installer_busy", "step {step}");
    }

    // Stopped after a swap: the OS released the lock, `current.json` stays.
    let root = prepared(
        temp.path(),
        "stopped",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    crashed_owner(&temp.path().join("stopped-state"));
    let planned = plan_id(&plan_update(&host, &root, C2, &[]));
    host.crash_after.set(Some(steps - 2));
    let stopped = apply_update(&host, &root, C2, &planned, &[]);
    assert_eq!(
        code(&stopped),
        "install_operation_interrupted",
        "{stopped:?}"
    );
    let tick = crate::watchdog::tick_for_test(&Machine, &root);
    assert_eq!(
        tick.code().as_str(),
        "watchdog_installer_unfinished",
        "{tick}"
    );
    assert_eq!(tick.category(), Category::Warning);
    let start =
        crate::runtime_start::start_refusal_for_test(&root, &root.join("actingd.config.json"))
            .expect("actingctl start refuses");
    assert_eq!(start.code().as_str(), "start_runtime_refused", "{start}");
    assert_eq!(
        start.causes()[0].outcome().code().as_str(),
        "watchdog_installer_unfinished",
        "{start}"
    );
}

// IT-11: the target's ledger verification runs before any swap; a failing one refuses with
// nothing swapped.
#[test]
fn inst1_it_11_the_ledger_verification_runs_before_any_swap() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);
    let mut target = BuildSpec::new(C2);
    set_speaks(&mut target, "ledger", json!([4, 4]));

    // A failing verification.
    let root = prepared(temp.path(), "failing", &live, &target, &host);
    let before = program_tree(&root);
    let planned = plan_id(&plan_update(&host, &root, C2, &[]));
    host.programs.verify_exit.set(1);
    let refused = apply_update(&host, &root, C2, &planned, &[]);
    host.programs.verify_exit.set(0);
    assert_eq!(code(&refused), "install_update_refused", "{refused:?}");
    assert_eq!(refusal(&refused), Some("ledger_verify_failed"));
    assert_eq!(program_tree(&root), before, "nothing swapped");
    assert!(!root.join("install").join("current.json").exists());

    // A passing verification precedes every swap.
    let root = prepared(temp.path(), "passing", &live, &target, &host);
    let planned = plan_id(&plan_update(&host, &root, C2, &[]));
    host.log.borrow_mut().clear();
    let applied = apply_update(&host, &root, C2, &planned, &[]);
    assert_eq!(code(&applied), "install_update_applied", "{applied:?}");
    let journal = journal_of(&root, &operation_id(&applied));
    let steps = journal["steps"].as_array().expect("steps");
    assert_eq!(steps[0]["kind"], json!("ledger_verify"), "{journal}");
    let first_swap = steps
        .iter()
        .find(|step| step["kind"] == "swap_file")
        .and_then(|step| step["n"].as_u64())
        .expect("a swap");
    let log = host.log.borrow();
    let verify = log
        .iter()
        .position(|line| line.contains("ledger-maintenance verify"))
        .expect("the verification ran");
    let swap = log
        .iter()
        .position(|line| *line == format!("step {first_swap}"))
        .expect("the first swap");
    assert!(verify < swap, "{log:?}");
}

/// A start log of the live build whose ledger open took `phase_ms` in each of its four phases
/// over `event_count` events.
fn start_log(root: &Path, phase_ms: u64, event_count: u64) {
    let line = json!({
        "schema_version": "actingcommand.outcome.v1",
        "code": "ledger_opened",
        "category": "info",
        "values": {
            "event_count": event_count, "artifact_count": 0, "artifact_bytes": 0,
            "worker_count": 1, "sql_read_ms": phase_ms, "verify_ms": phase_ms,
            "material_ms": phase_ms, "restore_ms": phase_ms, "timeout_ms": 600_000
        }
    });
    fs::write(
        root.join("actingd-1800000000999.log"),
        format!("INFO actingd: opened\nOUTCOME actingd: {line}\n"),
    )
    .expect("the start log");
}

/// A root updated from `live` to C2 (S0 = 100), keeping `live` for a rollback, with a start log.
fn updated(temp: &TempDir, host: &FakeHost, name: &str, live: &BuildSpec) -> PathBuf {
    host.programs.ledger_head.set(100);
    let root = prepared(temp.path(), name, live, &BuildSpec::new(C2), host);
    let applied = apply_update(
        host,
        &root,
        C2,
        &plan_id(&plan_update(host, &root, C2, &[])),
        &[],
    );
    assert_eq!(code(&applied), "install_update_applied", "{applied:?}");
    host.programs.ledger_head.set(150);
    start_log(&root, 1_000, 100);
    root
}

fn plan_rollback(host: &FakeHost, root: &Path) -> super::Answer {
    run(
        host,
        &["rollback", "plan", "--root", word(root), "--to", C1],
    )
}

fn gate_parts(answer: &super::Answer) -> Vec<String> {
    answer
        .outcome
        .value("rollback_gate_parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// IT-12 (table): the rollback gate.
#[test]
fn inst1_it_12_the_rollback_gate_refuses_each_failed_part_and_passes_otherwise() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);

    // The target's reader refuses a page.
    let root = updated(&temp, &host, "unreadable", &live);
    host.programs.unreadable.replace(Some((C1.to_owned(), 120)));
    let refused = plan_rollback(&host, &root);
    host.programs.unreadable.replace(None);
    assert_eq!(
        code(&refused),
        "install_rollback_ledger_unreadable",
        "{refused:?}"
    );
    assert_eq!(refused.outcome.value("ledger_sequence"), Some(&json!(101)));

    // The open estimate exceeds the deadline minus the margin.
    let root = updated(&temp, &host, "slow", &live);
    start_log(&root, 300_000, 100);
    let refused = plan_rollback(&host, &root);
    assert_eq!(code(&refused), "install_rollback_refused", "{refused:?}");
    assert_eq!(gate_parts(&refused), ["open_time"]);

    // The target's check-config refuses the configuration.
    let root = updated(&temp, &host, "config", &live);
    host.programs
        .refuse_check_config
        .replace(vec![C1.to_owned()]);
    let refused = plan_rollback(&host, &root);
    host.programs.refuse_check_config.replace(Vec::new());
    assert_eq!(code(&refused), "install_rollback_refused", "{refused:?}");
    assert_eq!(gate_parts(&refused), ["config"]);

    // A target whose defaults name no open deadline (a build before v0.12.11).
    let mut older = BuildSpec::new(C1);
    for (path, bytes) in &mut older.runtime.files {
        if path.as_str() == "actingd.defaults.json" {
            *bytes = b"{}".to_vec();
        }
    }
    let root = updated(&temp, &host, "older", &older);
    let refused = plan_rollback(&host, &root);
    assert_eq!(code(&refused), "install_rollback_refused", "{refused:?}");
    assert_eq!(gate_parts(&refused), ["open_time"]);

    // Every part passes: planned, applied.
    let root = updated(&temp, &host, "passing", &live);
    let planned = plan_rollback(&host, &root);
    assert_eq!(code(&planned), "install_rollback_planned", "{planned:?}");
    let applied = run(
        &host,
        &[
            "rollback",
            "apply",
            "--root",
            word(&root),
            "--to",
            C1,
            "--plan",
            plan_id(&planned).as_str(),
        ],
    );
    assert_eq!(code(&applied), "install_rollback_applied", "{applied:?}");
    for (path, bytes) in spec_tree(&live) {
        assert_eq!(fs::read(root.join(&path)).ok(), Some(bytes), "{path}");
    }

    // A failure between plan and apply refuses at apply, with nothing changed.
    let root = updated(&temp, &host, "between", &live);
    let planned = plan_rollback(&host, &root);
    assert_eq!(code(&planned), "install_rollback_planned", "{planned:?}");
    let before = program_tree(&root);
    host.programs.unreadable.replace(Some((C1.to_owned(), 120)));
    let refused = run(
        &host,
        &[
            "rollback",
            "apply",
            "--root",
            word(&root),
            "--to",
            C1,
            "--plan",
            plan_id(&planned).as_str(),
        ],
    );
    host.programs.unreadable.replace(None);
    assert_eq!(
        code(&refused),
        "install_rollback_ledger_unreadable",
        "{refused:?}"
    );
    assert_eq!(program_tree(&root), before, "nothing changed");
}
