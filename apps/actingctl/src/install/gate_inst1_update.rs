// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #421 INST-1, test rows IT-5 to IT-12 of the frozen model INST-M v1.0 (§12, "Update
// plan", "Update apply", "Rollback") and IT-15, IT-16 of INST-1b-F (#421 deploy design §8 S1).
// Every expected value comes from the model (§4.4, §5, §6.1 to §6.5, §7), the design (§1 item 3,
// §4, §8) and their contract (contracts/install-cli.md, "Update and rollback", "Interface
// checks", "Safety", "The rollback gate", "Documents", "Outcome codes"):
// - IT-5: the interface edges refuse a ledger the target cannot read, a one-way ledger without
//   `--accept-ledger-one-way` (planned and journalled with it), a placed bundle the target
//   cannot run; an unknown declaration is the caveat `pack_declaration_unknown`;
// - IT-6: the plan refuses an A/B root, a running Runtime (`--preview` reports it), a held owner
//   lock, a foreign `.part`, a running image other than actingctl.exe that would change, and a
//   running adb.exe that would change;
// - IT-7: apply with a stale plan id refuses `plan_changed` and changes nothing;
// - IT-8: apply swaps Tools before the Runtime, never touches an unchanged file, renames a
//   running actingctl.exe aside while it stays open, keeps the old bytes as `.old-<ms>`,
//   verifies every manifest file, records the state, and leaves extra live files;
// - IT-9: after a stop at each step, `resume` reaches the uninterrupted tree and `undo` restores
//   the old tree byte for byte with every displaced file kept;
// - IT-10: the writer lock is held through every step, a second installer meets
//   `operation_unfinished`, a watchdog tick meanwhile is `watchdog_installer_busy`, and after a
//   stop with `current.json` present the tick is `watchdog_installer_unfinished` and
//   `actingctl start` refuses;
// - IT-11: when the ledger ranges differ the target's verification runs before any swap, and a
//   failing one refuses `ledger_verify_failed` with nothing swapped;
// - IT-12: the rollback gate refuses an unreadable page (with its sequence), the target's own
//   open over the deadline minus the margin, the target's check-config refusal and a target
//   without an open deadline; it passes otherwise, and is evaluated again at apply;
// - IT-15: the open-time part sums all eight phases of the target build's own last open on the
//   root: an LN-8 open (the four complete-read phases 0, `index_ms` large) refuses a deadline
//   below it; without an open of the target the newest open of another build is judged, with
//   the caveat `open_time_from_other_build`;
// - IT-16: `adopt` and `update plan` on a root holding a deploy kit's `.old-<stamp>` and
//   `.rolledback-<stamp>` files: both pass, the files are extra, and `leftovers` lists them
//   without an operation id.
// INST-1b-F fix round 1 (brief `rd-1027\brief-inst1b-f-fix1.md` item 9, the gaps of review S1
// §8), each expected value from the brief's item and the contract it amends:
// - fix1-1 (F1): a target whose check-config refuses `--install-root` (a build before
//   `actingd-config` revision 17, or one that refuses the option) is checked without it, with the
//   caveat `check_config_without_install_root`, and the rollback passes;
// - fix1-3 (F3): a rollback past the build the live one replaced fails `ledger_readability`,
//   naming the one target it may pass for; toward that build it passes;
// - fix1-4 (F4): a child still running at its deadline (the `install.child.*` key) is left
//   running and reported with its `pid` and the key: before the operation opens nothing changes;
//   the ledger verification inside apply leaves the operation for `resume`;
// - fix1-5 (F5): a crash between the two renames of a swap is resumed or undone; a crash after
//   the end is journalled and before `current.json` is retired is finished by `resume`;
// - fix1-9u: the undo of a completed update: without an owner start no gate; after one, the
//   gate decides; an owner journal it cannot read refuses `processes_unreadable` (F6);
// - fix1-9s: `status --check-rollback` evaluates the whole gate toward the previous build, also
//   while a Runtime runs.
// The codes are the family's folded codes (ruling 54): `install_command_done`,
// `install_caveat_noted` and `install_command_refused` with the command as `operation`, and
// `install_operation_interrupted`. The machine is the fake host of `fixture.rs`; every root is a
// temporary folder with neutral files.

use super::fixture::{
    BuildSpec, C1, C2, FakeHost, STALLED_PID, code, live_root, run, sha256_hex, tree, word,
    write_json,
};
use super::host::RunningImage;
use crate::watchdog::decide::LiveOwner;
use crate::watchdog::observe::{self, HostProbe, WriterProbe};
use crate::watchdog::start::{Attempt, StartedProcess, Target, TickSeams};
use actingcommand_contract::outcome::{Category, Outcome};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use tempfile::TempDir;

/// The tick's wall clock.
const T0: u64 = 1_800_000_000_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;

/// The answer's outcome value `key`, when it is a string.
fn text<'a>(answer: &'a super::Answer, key: &str) -> Option<&'a str> {
    answer.outcome.value(key).and_then(Value::as_str)
}

fn refusal(answer: &super::Answer) -> Option<&str> {
    text(answer, "install_refusal")
}

/// Asserts `answer` is `install_command_done` of the command `operation`.
fn assert_done(answer: &super::Answer, operation: &str) {
    assert_eq!(code(answer), "install_command_done", "{answer:?}");
    assert_eq!(text(answer, "operation"), Some(operation), "{answer:?}");
}

/// Asserts `answer` is `install_command_refused` of the command `operation` for `why`.
fn assert_refused(answer: &super::Answer, operation: &str, why: &str) {
    assert_eq!(code(answer), "install_command_refused", "{answer:?}");
    assert_eq!(text(answer, "operation"), Some(operation), "{answer:?}");
    assert_eq!(refusal(answer), Some(why), "{answer:?}");
}

/// The `install_caveat` of every `install_caveat_noted` link of the answer's outcome.
fn caveats(answer: &super::Answer) -> Vec<String> {
    answer
        .outcome
        .causes()
        .iter()
        .map(|link| link.outcome())
        .filter(|link| link.code().as_str() == "install_caveat_noted")
        .filter_map(|link| link.value("install_caveat").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

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
    assert_done(&adopted, "install_adopt");
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
    assert_done(&staged, "install_release_stage");
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
        parent_pid: None,
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
    text(answer, "install_operation_id")
        .unwrap_or_else(|| panic!("an operation id: {answer:?}"))
        .to_owned()
}

/// The Unix milliseconds `op-<ms>` names.
fn ms_of(operation_id: &str) -> u64 {
    operation_id
        .strip_prefix("op-")
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("an operation id: {operation_id}"))
}

fn set_speaks(spec: &mut BuildSpec, interface: &str, range: Value) {
    spec.runtime.speaks[interface] = range.clone();
    spec.tools.speaks[interface] = range;
}

fn state_of(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("install").join("state.json")).expect("state.json"))
        .expect("state JSON")
}

fn set_packs(root: &Path, packs: Value) {
    let mut state = state_of(root);
    state["packs"] = packs;
    write_json(&root.join("install").join("state.json"), &state);
}

/// Sets `ledger_observer.open_timeout_ms` in the root's configuration.
fn set_open_timeout(root: &Path, timeout_ms: u64) {
    let config = root.join("actingd.config.json");
    let mut document: Value =
        serde_json::from_slice(&fs::read(&config).expect("the configuration")).expect("JSON");
    document["ledger_observer"] = json!({"open_timeout_ms": timeout_ms});
    write_json(&config, &document);
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
    assert_refused(&refused, "install_update_plan", "interface_edge");

    // A one-way ledger: refused without the flag, planned and journalled with it.
    let mut one_way = BuildSpec::new(C2);
    set_speaks(&mut one_way, "ledger", json!([3, 5]));
    let root = prepared(temp.path(), "one-way", &live, &one_way, &host);
    let refused = plan_update(&host, &root, C2, &[]);
    assert_refused(&refused, "install_update_plan", "interface_edge");
    let planned = plan_update(&host, &root, C2, &["--accept-ledger-one-way"]);
    assert_done(&planned, "install_update_plan");
    let applied = apply_update(
        &host,
        &root,
        C2,
        &plan_id(&planned),
        &["--accept-ledger-one-way"],
    );
    assert_done(&applied, "install_update_apply");
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
    assert_refused(&refused, "install_update_plan", "interface_edge");

    // An unknown declaration is a caveat; the plan stands.
    set_packs(
        &root,
        json!([{"game": "neutral", "repository": null, "commit": null, "tag": null,
            "package_range": null}]),
    );
    let planned = plan_update(&host, &root, C2, &[]);
    assert_done(&planned, "install_update_plan");
    assert_eq!(
        caveats(&planned),
        ["pack_declaration_unknown"],
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
    assert_refused(&refused, "install_update_plan", "layout_ab_unsupported");

    // A Runtime runs under the root; `--preview` reports it.
    let root = prepared(temp.path(), "running", &live, &target, &host);
    host.processes.replace(vec![image(
        &root,
        "runtime/actingcommand-actingd.exe",
        41,
        false,
    )]);
    let refused = plan_update(&host, &root, C2, &[]);
    assert_refused(&refused, "install_update_plan", "runtime_running");
    let previewed = plan_update(&host, &root, C2, &["--preview"]);
    assert_done(&previewed, "install_update_plan");
    assert_eq!(previewed.members["running"]["runtime"][0]["pid"], json!(41));
    host.processes.replace(Vec::new());

    // The owner lock is held (a journal with a record, locked as a running Runtime locks it).
    let root = prepared(temp.path(), "owner", &live, &target, &host);
    let mut owner = File::create(temp.path().join("owner-state").join("owner.lock"))
        .expect("the owner journal");
    owner
        .write_all(format!("{}\n", owner_record(true)).as_bytes())
        .expect("write the owner journal");
    owner.lock().expect("hold the owner lock");
    let refused = plan_update(&host, &root, C2, &[]);
    assert_refused(&refused, "install_update_plan", "owner_lock_held");
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
    assert_refused(&refused, "install_update_plan", "foreign_part");

    // A running image other than actingctl.exe that would change.
    let root = prepared(temp.path(), "ledger-reader", &live, &target, &host);
    host.processes
        .replace(vec![image(&root, "tools/actingledger.exe", 51, false)]);
    let refused = plan_update(&host, &root, C2, &[]);
    assert_refused(&refused, "install_update_plan", "running_image_blocks");
    assert_eq!(refused.outcome.value("pid"), Some(&json!(51)));
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
    assert_refused(&refused, "install_update_plan", "running_image_blocks");
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
    assert_done(&planned, "install_update_plan");
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
    assert_refused(&refused, "install_update_apply", "plan_changed");
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
    assert_done(&planned, "install_update_plan");
    assert!(
        planned.members["extra"]
            .as_array()
            .expect("extra")
            .contains(&json!("runtime/notes.txt")),
        "{planned:?}"
    );
    let applied = apply_update(&host, &root, C2, &plan_id(&planned), &[]);
    assert_done(&applied, "install_update_apply");
    assert_eq!(
        applied.outcome.value("installation_changed"),
        Some(&json!(true))
    );
    assert_eq!(
        caveats(&applied),
        ["mcp_clients_on_previous_build"],
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
    let state = state_of(&root);
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
    assert_done(&applied, "install_update_apply");
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
        assert_eq!(text(&stopped, "operation"), Some("install_update_apply"));
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
        assert_done(&resumed, "install_resume");
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
        assert_done(&undone, "install_undo");
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

/// A machine for the watchdog tick, booted a day before it: nothing runs under the root, and
/// nothing may be started.
struct Machine;

impl TickSeams for Machine {
    fn now_unix_ms(&self) -> u64 {
        T0
    }

    fn monotonic(&self) -> Duration {
        Duration::ZERO
    }

    fn pause(&self, _interval: Duration) {}

    fn host_probe(&self, _root_plain: &Path) -> Result<HostProbe, Outcome> {
        Ok(HostProbe {
            processes: Vec::new(),
            boot_unix_ms: Some(T0 - DAY_MS),
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

/// An owner journal record started an hour before the tick: `active` is a Runtime that ended
/// without a close, which the watchdog would start.
fn owner_record(active: bool) -> Value {
    json!({
        "schema_version": "actingcommand.runtime-owner.v2",
        "revision": 1,
        "owner_epoch": "epoch_neutral",
        "pid": 100,
        "started_at_unix_ms": T0 - HOUR_MS,
        "active": active,
        "active_instances": [],
        "closed_at_unix_ms": null,
        "resource_disposition": if active { "in_use" } else { "none" },
    })
}

/// The owner journal of a Runtime that crashed.
fn crashed_owner(state_root: &Path) {
    fs::write(
        state_root.join("owner.lock"),
        format!("{}\n", owner_record(true)),
    )
    .expect("the owner journal");
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
                refusal(&other).map(str::to_owned),
                tick.code().as_str().to_owned(),
            ));
        })));
    }
    let applied = apply_update(&host, &root, C2, &planned, &[]);
    host.on_step.replace(None);
    assert_done(&applied, "install_update_apply");
    let seen = seen.borrow();
    let steps = journal_of(&root, &operation_id(&applied))["steps"]
        .as_array()
        .expect("steps")
        .len();
    assert_eq!(seen.len(), steps, "every step was observed");
    for (step, busy, other, why, tick) in seen.iter() {
        assert!(*busy, "step {step}: the writer lock is held");
        assert_eq!(other, "install_command_refused", "step {step}");
        assert_eq!(why.as_deref(), Some("operation_unfinished"), "step {step}");
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
    assert_eq!(
        tick.value("install_operation_id"),
        Some(&json!(operation_id(&stopped))),
        "{tick}"
    );
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
    assert_refused(&refused, "install_update_apply", "ledger_verify_failed");
    assert_eq!(program_tree(&root), before, "nothing swapped");
    assert!(!root.join("install").join("current.json").exists());

    // A passing verification precedes every swap.
    let root = prepared(temp.path(), "passing", &live, &target, &host);
    let planned = plan_id(&plan_update(&host, &root, C2, &[]));
    host.log.borrow_mut().clear();
    let applied = apply_update(&host, &root, C2, &planned, &[]);
    assert_done(&applied, "install_update_apply");
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

/// The eight phases of a ledger open, each `phase_ms`.
fn even_phases(phase_ms: u64) -> [u64; 8] {
    [phase_ms; 8]
}

/// A start log `actingd-<at_ms>.log` in the root (the fake host's log folder) whose ledger open
/// reports `phases` (`sql_read_ms`, `verify_ms`, `material_ms`, `restore_ms`, `meta_ms`,
/// `tail_ms`, `index_ms`, `import_ms`).
fn start_log(root: &Path, at_ms: u64, phases: [u64; 8]) {
    let line = json!({
        "schema_version": "actingcommand.outcome.v1",
        "code": "ledger_opened",
        "category": "info",
        "values": {
            "event_count": 4096, "artifact_count": 0, "artifact_bytes": 0, "worker_count": 1,
            "sql_read_ms": phases[0], "verify_ms": phases[1], "material_ms": phases[2],
            "restore_ms": phases[3], "meta_ms": phases[4], "tail_ms": phases[5],
            "index_ms": phases[6], "import_ms": phases[7], "timeout_ms": 600_000
        }
    });
    fs::write(
        root.join(format!("actingd-{at_ms}.log")),
        format!("INFO actingd: opened\nOUTCOME actingd: {line}\n"),
    )
    .expect("the start log");
}

/// A start of the adopted build C1 on `root`, after the adopt and before the update: the
/// target build's own open for a rollback to C1.
fn own_start_log(root: &Path, phases: [u64; 8]) {
    let state = state_of(root);
    let adopted = state["builds"]
        .as_array()
        .expect("builds")
        .iter()
        .find(|build| build["origin"] == "adopted")
        .and_then(|build| build["operation_id"].as_str())
        .map(ms_of)
        .expect("the adopt");
    start_log(root, adopted + 1, phases);
}

/// A start of the live build C2 on `root`, after the update `applied` ended.
fn live_start_log(root: &Path, applied: &str, phases: [u64; 8]) {
    let finished = journal_of(root, applied)["finished_at_unix_ms"]
        .as_u64()
        .expect("the update's end");
    start_log(root, finished + 1, phases);
}

/// A root updated from `live` to C2 (S0 = 100), keeping `live` for a rollback; the update's
/// operation id.
fn updated(temp: &TempDir, host: &FakeHost, name: &str, live: &BuildSpec) -> (PathBuf, String) {
    host.programs.ledger_head.set(100);
    let root = prepared(temp.path(), name, live, &BuildSpec::new(C2), host);
    let applied = apply_update(
        host,
        &root,
        C2,
        &plan_id(&plan_update(host, &root, C2, &[])),
        &[],
    );
    assert_done(&applied, "install_update_apply");
    host.programs.ledger_head.set(150);
    (root, operation_id(&applied))
}

fn plan_rollback(host: &FakeHost, root: &Path) -> super::Answer {
    run(
        host,
        &["rollback", "plan", "--root", word(root), "--to", C1],
    )
}

fn apply_rollback(host: &FakeHost, root: &Path, plan: &str) -> super::Answer {
    run(
        host,
        &[
            "rollback",
            "apply",
            "--root",
            word(root),
            "--to",
            C1,
            "--plan",
            plan,
        ],
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

/// The values of the gate row `part` of a rollback plan's result.
fn gate_row<'a>(answer: &'a super::Answer, part: &str) -> &'a Value {
    answer.members["gate"]
        .as_array()
        .expect("the gate rows")
        .iter()
        .find(|row| row["part"] == part)
        .map(|row| &row["values"])
        .unwrap_or_else(|| panic!("the gate row {part}: {answer:?}"))
}

// IT-12 (table): the rollback gate.
#[test]
fn inst1_it_12_the_rollback_gate_refuses_each_failed_part_and_passes_otherwise() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);

    // The target's reader refuses a page.
    let (root, _) = updated(&temp, &host, "unreadable", &live);
    own_start_log(&root, even_phases(1_000));
    host.programs.unreadable.replace(Some((C1.to_owned(), 120)));
    let refused = plan_rollback(&host, &root);
    host.programs.unreadable.replace(None);
    assert_refused(&refused, "install_rollback_plan", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["ledger_readability"]);
    // The first sequence the reader refuses (fix round 1, review S1 F11), not the page's first.
    assert_eq!(refused.outcome.value("ledger_sequence"), Some(&json!(120)));

    // The target's own open exceeds the deadline (600 000 ms) minus the margin (20 000 ms).
    let (root, _) = updated(&temp, &host, "slow", &live);
    own_start_log(&root, even_phases(75_000));
    let refused = plan_rollback(&host, &root);
    assert_refused(&refused, "install_rollback_plan", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["open_time"]);

    // The target's check-config refuses the configuration.
    let (root, _) = updated(&temp, &host, "config", &live);
    own_start_log(&root, even_phases(1_000));
    host.programs
        .refuse_check_config
        .replace(vec![C1.to_owned()]);
    let refused = plan_rollback(&host, &root);
    host.programs.refuse_check_config.replace(Vec::new());
    assert_refused(&refused, "install_rollback_plan", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["config"]);

    // A target whose defaults name no open deadline (a build before v0.12.11).
    let mut older = BuildSpec::new(C1);
    for (path, bytes) in &mut older.runtime.files {
        if path.as_str() == "actingd.defaults.json" {
            *bytes = b"{}".to_vec();
        }
    }
    let (root, _) = updated(&temp, &host, "older", &older);
    own_start_log(&root, even_phases(1_000));
    let refused = plan_rollback(&host, &root);
    assert_refused(&refused, "install_rollback_plan", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["open_time"]);

    // Every part passes: planned, applied.
    let (root, _) = updated(&temp, &host, "passing", &live);
    own_start_log(&root, even_phases(1_000));
    let planned = plan_rollback(&host, &root);
    assert_done(&planned, "install_rollback_plan");
    assert!(caveats(&planned).is_empty(), "{planned:?}");
    let applied = apply_rollback(&host, &root, &plan_id(&planned));
    assert_done(&applied, "install_rollback_apply");
    for (path, bytes) in spec_tree(&live) {
        assert_eq!(fs::read(root.join(&path)).ok(), Some(bytes), "{path}");
    }

    // A failure between plan and apply refuses at apply, with nothing changed.
    let (root, _) = updated(&temp, &host, "between", &live);
    own_start_log(&root, even_phases(1_000));
    let planned = plan_rollback(&host, &root);
    assert_done(&planned, "install_rollback_plan");
    let before = program_tree(&root);
    host.programs.unreadable.replace(Some((C1.to_owned(), 120)));
    let refused = apply_rollback(&host, &root, &plan_id(&planned));
    host.programs.unreadable.replace(None);
    assert_refused(&refused, "install_rollback_apply", "rollback_gate_failed");
    assert_eq!(refused.outcome.value("ledger_sequence"), Some(&json!(120)));
    assert_eq!(program_tree(&root), before, "nothing changed");
}

// IT-15: the open-time part on an LN-8 open line. The four complete-read phases are 0 and
// `index_ms` is large (the v0.12.16 open: 195 724 ms of 195 917 ms): the part sums all eight
// phases of the target's own last open, unscaled, and refuses a deadline below it. Without an
// open of the target on the root, the newest open of another build is judged, noted.
#[test]
fn inst1_it_15_the_open_time_part_sums_every_phase_of_the_target_builds_own_open() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);
    let ln8 = [0, 0, 0, 0, 12, 150, 195_724, 31];
    let sum = 195_917;

    // The target's own LN-8 open against a deadline below it.
    let (root, applied) = updated(&temp, &host, "below", &live);
    own_start_log(&root, ln8);
    // The live build's own open, newer and fast, is not the target's.
    live_start_log(&root, &applied, even_phases(10));
    set_open_timeout(&root, 180_000);
    let refused = plan_rollback(&host, &root);
    assert_refused(&refused, "install_rollback_plan", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["open_time"]);
    let open = gate_row(&refused, "open_time");
    assert_eq!(open["estimate_ms"], json!(sum), "{open}");
    assert_eq!(open["deadline_ms"], json!(180_000), "{open}");
    assert_eq!(open["from_target_build"], json!(true), "{open}");
    assert!(caveats(&refused).is_empty(), "{refused:?}");

    // The same open within a deadline above it plus the margin passes.
    set_open_timeout(&root, sum + 20_000);
    let planned = plan_rollback(&host, &root);
    assert_done(&planned, "install_rollback_plan");
    assert_eq!(gate_row(&planned, "open_time")["estimate_ms"], json!(sum));

    // No open of the target on the root: the live build's open is judged, noted.
    let (root, applied) = updated(&temp, &host, "other", &live);
    live_start_log(&root, &applied, ln8);
    set_open_timeout(&root, 180_000);
    let refused = plan_rollback(&host, &root);
    assert_refused(&refused, "install_rollback_plan", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["open_time"]);
    let open = gate_row(&refused, "open_time");
    assert_eq!(open["estimate_ms"], json!(sum), "{open}");
    assert_eq!(open["from_target_build"], json!(false), "{open}");
    assert_eq!(
        caveats(&refused),
        ["open_time_from_other_build"],
        "{refused:?}"
    );
}

// IT-16: a root holding a deploy kit's leftovers: `.old-<stamp>` and `.rolledback-<stamp>` files
// whose stamps name no operation of the root.
#[test]
fn inst1_it_16_adopt_and_plan_tolerate_kit_leftovers_without_operation_ids() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);
    let target = BuildSpec::new(C2);
    let root = live_root(temp.path(), "root", &live);
    let kit = [
        "runtime/actingctl.exe.old-20261026-0832",
        "runtime/actingcommand-actingd.exe.old-1761521460000",
        "tools/actingledger.exe.rolledback-20261027083100",
    ];
    for relative in kit {
        let path = relative
            .split('/')
            .fold(root.clone(), |path, part| path.join(part));
        fs::write(&path, b"a kit's kept file").expect("a kit leftover");
    }

    let adopted = run(&host, &["adopt", "--root", word(&root)]);
    assert_done(&adopted, "install_adopt");
    assert_eq!(
        program_tree(&root.join("install").join("builds").join(C1)),
        spec_tree(&live),
        "the kept build holds exactly the listed files"
    );
    let release = temp.path().join("root-release");
    target.write_release(&release);
    let staged = run(
        &host,
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
    assert_done(&staged, "install_release_stage");

    let planned = plan_update(&host, &root, C2, &[]);
    assert_done(&planned, "install_update_plan");
    let extra = planned.members["extra"].as_array().expect("extra");
    for relative in kit {
        assert!(extra.contains(&json!(relative)), "{relative}: {planned:?}");
    }

    let leftovers = run(&host, &["leftovers", "--root", word(&root)]);
    assert_done(&leftovers, "install_leftovers");
    let rows = leftovers.members["leftovers"].as_array().expect("rows");
    for relative in kit {
        let name = relative.rsplit('/').next().expect("a name");
        let row = rows
            .iter()
            .find(|row| {
                row["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with(name))
            })
            .unwrap_or_else(|| panic!("{relative} is listed: {leftovers:?}"));
        assert_eq!(row["operation_id"], Value::Null, "{row}");
    }
    // The installer's own kept files keep their operation.
    assert!(
        rows.iter().any(|row| row["path"]
            .as_str()
            .is_some_and(|path| path.contains("current.json.finished-"))
            && row["operation_id"].is_string()),
        "{leftovers:?}"
    );
}

// ---- INST-1b-F fix round 1 (review S1 §8) ----

/// A third neutral commit, a build after C2.
const C3: &str = "3333333333333333333333333333333333333333";

/// `relative` (with `/` separators) below `root`.
fn at(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Sets the `install` section of the root's configuration.
fn set_install(root: &Path, section: Value) {
    let config = root.join("actingd.config.json");
    let mut document: Value =
        serde_json::from_slice(&fs::read(&config).expect("the configuration")).expect("JSON");
    document["install"] = section;
    write_json(&config, &document);
}

/// Stages `spec` on `root` from a release folder `name` of its own.
fn stage(host: &FakeHost, base: &Path, root: &Path, spec: &BuildSpec, name: &str) {
    let release = base.join(name);
    spec.write_release(&release);
    let staged = run(
        host,
        &[
            "release",
            "stage",
            "--root",
            word(root),
            "--dir",
            word(&release),
            "--local-build",
        ],
    );
    assert_done(&staged, "install_release_stage");
}

/// `install <command> --root <root> --operation <operation>` (`resume` or `undo`).
fn on_operation(host: &FakeHost, command: &str, root: &Path, operation: &str) -> super::Answer {
    run(
        host,
        &[command, "--root", word(root), "--operation", operation],
    )
}

/// The check-config runs of `build` the host logged, in order.
fn check_configs(host: &FakeHost, build: &str) -> Vec<String> {
    host.log
        .borrow()
        .iter()
        .filter(|line| line.starts_with(build) && line.contains(" check-config "))
        .cloned()
        .collect()
}

/// An owner journal whose record started at `started_at_unix_ms` and ended without a close.
fn owner_started_at(state_root: &Path, started_at_unix_ms: u64) {
    let mut record = owner_record(true);
    record["started_at_unix_ms"] = json!(started_at_unix_ms);
    fs::write(state_root.join("owner.lock"), format!("{record}\n")).expect("the owner journal");
}

/// Asserts the root's program files are those of `spec`.
fn assert_tree_of(root: &Path, spec: &BuildSpec, what: &str) {
    for (path, bytes) in spec_tree(spec) {
        assert_eq!(
            fs::read(at(root, &path)).ok(),
            Some(bytes),
            "{what}: {path}"
        );
    }
}

// fix1-1 (brief item 1; review S1 F1): the check-config of a target before `actingd-config`
// revision 17 never gets `--install-root`, and a target that refuses the option is asked again
// without it; the plan carries the caveat and the rollback passes and applies.
#[test]
fn inst1_fix1_1_a_target_that_does_not_take_install_root_is_checked_without_it() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    host.programs
        .refuse_install_root
        .replace(vec![C1.to_owned()]);

    // A build whose range ends at revision 16 and whose check-config refuses the option, as
    // v0.12.17's refuses more than `--config <path>` (`check_config_usage_invalid`).
    let mut old = BuildSpec::new(C1);
    set_speaks(&mut old, "actingd-config", json!([5, 16]));
    let (root, _) = updated(&temp, &host, "old", &old);
    own_start_log(&root, even_phases(1_000));
    host.log.borrow_mut().clear();
    let planned = plan_rollback(&host, &root);
    assert_done(&planned, "install_rollback_plan");
    assert_eq!(
        caveats(&planned),
        ["check_config_without_install_root"],
        "{planned:?}"
    );
    assert_eq!(gate_row(&planned, "config")["install_root"], json!(false));
    let runs = check_configs(&host, C1);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(!runs[0].contains("--install-root"), "{runs:?}");
    let applied = apply_rollback(&host, &root, &plan_id(&planned));
    assert_done(&applied, "install_rollback_apply");
    assert_eq!(
        caveats(&applied),
        ["check_config_without_install_root"],
        "{applied:?}"
    );
    assert_tree_of(&root, &old, "rolled back");

    // A build that declares revision 17 and still refuses the option: asked again without it.
    let (root, _) = updated(&temp, &host, "refusing", &BuildSpec::new(C1));
    own_start_log(&root, even_phases(1_000));
    host.log.borrow_mut().clear();
    let planned = plan_rollback(&host, &root);
    assert_done(&planned, "install_rollback_plan");
    assert_eq!(
        caveats(&planned),
        ["check_config_without_install_root"],
        "{planned:?}"
    );
    let runs = check_configs(&host, C1);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(
        runs[0].contains("--install-root") && !runs[1].contains("--install-root"),
        "{runs:?}"
    );
    host.programs.refuse_install_root.replace(Vec::new());
}

// fix1-3 (brief item 3; review S1 F3): S0 covers only the records written since the live build
// was installed, so the gate passes only toward the build the live one replaced.
#[test]
fn inst1_fix1_3_a_rollback_past_the_build_the_live_one_replaced_is_refused() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, to_c2) = updated(&temp, &host, "past", &BuildSpec::new(C1));
    stage(
        &host,
        temp.path(),
        &root,
        &BuildSpec::new(C3),
        "past-release-c3",
    );
    let to_c3 = apply_update(
        &host,
        &root,
        C3,
        &plan_id(&plan_update(&host, &root, C3, &[])),
        &[],
    );
    assert_done(&to_c3, "install_update_apply");
    // A start of each older build on the root, so that only the reader part can refuse.
    own_start_log(&root, even_phases(1_000));
    live_start_log(&root, &to_c2, even_phases(1_000));

    // Toward C1, past C2: refused, the part naming the one target it may pass for.
    let refused = run(
        &host,
        &["rollback", "plan", "--root", word(&root), "--to", C1],
    );
    assert_refused(&refused, "install_rollback_plan", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["ledger_readability"]);
    let readability = gate_row(&refused, "ledger_readability");
    assert_eq!(readability["previous"], json!(C2), "{readability}");
    assert_eq!(readability["s0"], Value::Null, "{readability}");

    // Toward C2, the build C3 replaced: planned.
    let planned = run(
        &host,
        &["rollback", "plan", "--root", word(&root), "--to", C2],
    );
    assert_done(&planned, "install_rollback_plan");
    assert_eq!(
        gate_row(&planned, "ledger_readability")["previous"],
        json!(C2)
    );
}

// fix1-4 (brief item 4; review S1 F4): a child still running at its deadline is left running
// and reported with its pid and the key of the deadline; before the operation opens nothing
// changes, and a ledger verification left running keeps the operation for `resume`.
#[test]
fn inst1_fix1_4_a_child_past_its_deadline_is_left_running_and_reported() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let operations = |root: &Path| {
        fs::read_dir(root.join("install").join("operations"))
            .expect("operations")
            .count()
    };

    // S0's head read, with the deadline the configuration names: apply refuses before the
    // operation opens.
    let root = prepared(
        temp.path(),
        "head",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    set_install(&root, json!({"child": {"ledger_read_timeout_ms": 45_000}}));
    let planned = plan_id(&plan_update(&host, &root, C2, &[]));
    let before = program_tree(&root);
    let opened = operations(&root);
    host.programs.stall.replace(vec!["export"]);
    host.programs.deadlines.borrow_mut().clear();
    let refused = apply_update(&host, &root, C2, &planned, &[]);
    host.programs.stall.replace(Vec::new());
    assert_refused(&refused, "install_update_apply", "ledger_head_unreadable");
    assert_eq!(
        refused.outcome.value("pid"),
        Some(&json!(STALLED_PID)),
        "{refused:?}"
    );
    assert_eq!(
        text(&refused, "config_key"),
        Some("install.child.ledger_read_timeout_ms")
    );
    assert!(
        host.programs
            .deadlines
            .borrow()
            .contains(&("export".to_owned(), 45_000)),
        "{:?}",
        host.programs.deadlines.borrow()
    );
    assert_eq!(program_tree(&root), before, "nothing changed");
    assert_eq!(operations(&root), opened, "no operation opened");

    // The target's check-config, with its default deadline: the plan refuses.
    host.programs.stall.replace(vec!["check-config"]);
    host.programs.deadlines.borrow_mut().clear();
    let refused = plan_update(&host, &root, C2, &[]);
    host.programs.stall.replace(Vec::new());
    assert_refused(&refused, "install_update_plan", "check_config_unanswered");
    assert_eq!(refused.outcome.value("pid"), Some(&json!(STALLED_PID)));
    assert_eq!(
        text(&refused, "config_key"),
        Some("install.child.check_config_timeout_ms")
    );
    assert!(
        host.programs
            .deadlines
            .borrow()
            .contains(&("check-config".to_owned(), 900_000)),
        "{:?}",
        host.programs.deadlines.borrow()
    );

    // The ledger verification inside apply: nothing swapped, the operation kept; `resume` once
    // the verification ends finishes the update.
    let mut target = BuildSpec::new(C2);
    set_speaks(&mut target, "ledger", json!([4, 4]));
    let root = prepared(temp.path(), "verify", &BuildSpec::new(C1), &target, &host);
    let planned = plan_id(&plan_update(&host, &root, C2, &[]));
    let before = program_tree(&root);
    host.programs.stall.replace(vec!["ledger-maintenance"]);
    let stopped = apply_update(&host, &root, C2, &planned, &[]);
    host.programs.stall.replace(Vec::new());
    assert_eq!(
        code(&stopped),
        "install_operation_interrupted",
        "{stopped:?}"
    );
    let cause = stopped.outcome.causes()[0].outcome();
    assert_eq!(
        cause.value("install_refusal"),
        Some(&json!("ledger_verify_failed")),
        "{stopped:?}"
    );
    assert_eq!(cause.value("pid"), Some(&json!(STALLED_PID)), "{stopped:?}");
    assert_eq!(
        cause.value("config_key"),
        Some(&json!("install.child.ledger_verify_timeout_ms")),
        "{stopped:?}"
    );
    assert_eq!(program_tree(&root), before, "nothing swapped");
    assert!(root.join("install").join("current.json").is_file());
    let resumed = on_operation(&host, "resume", &root, &operation_id(&stopped));
    assert_done(&resumed, "install_resume");
    assert_tree_of(&root, &target, "resumed");
}

// fix1-5 (brief item 5; review S1 F5 and §8.2): a crash between the two renames of a swap is
// finished by `resume` and reversed by `undo`; a crash after the end is journalled and before
// `current.json` is retired is finished by `resume`, after which the update can be undone.
#[test]
fn inst1_fix1_5_a_crash_inside_a_swap_or_before_current_json_is_retired_is_resumable() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);
    let target = BuildSpec::new(C2);
    let whole = prepared(temp.path(), "whole", &live, &target, &host);
    let applied = apply_update(
        &host,
        &whole,
        C2,
        &plan_id(&plan_update(&host, &whole, C2, &[])),
        &[],
    );
    assert_done(&applied, "install_update_apply");
    let reference = normalised(&whole);
    // The first swap that replaces a file (it has a `.old-<ms>` to rename aside).
    let first_swap = journal_of(&whole, &operation_id(&applied))["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["kind"] == "swap_file" && step["intent"]["old"].is_string())
        .and_then(|step| step["n"].as_u64())
        .and_then(|n| usize::try_from(n).ok())
        .expect("a swap that replaces a file");

    for ending in ["resume", "undo"] {
        let root = prepared(
            temp.path(),
            &format!("between-{ending}"),
            &live,
            &target,
            &host,
        );
        let planned = plan_id(&plan_update(&host, &root, C2, &[]));
        host.crash_between_renames.set(Some(first_swap));
        let stopped = apply_update(&host, &root, C2, &planned, &[]);
        assert_eq!(
            code(&stopped),
            "install_operation_interrupted",
            "{ending}: {stopped:?}"
        );
        // The live file is aside and the part not yet in its place.
        let operation = operation_id(&stopped);
        let swap = journal_of(&root, &operation)["steps"][first_swap - 1].clone();
        let intent = &swap["intent"];
        let named = |member: &str| at(&root, intent[member].as_str().expect("a path"));
        assert!(!named("dst").exists(), "{ending}: {swap}");
        assert!(named("old").is_file(), "{ending}: {swap}");
        assert!(named("part").is_file(), "{ending}: {swap}");
        let ended = on_operation(&host, ending, &root, &operation);
        if ending == "resume" {
            assert_done(&ended, "install_resume");
            assert_eq!(normalised(&root), reference, "the resumed tree");
        } else {
            assert_done(&ended, "install_undo");
            assert_tree_of(&root, &live, "undone");
        }
        assert!(
            !root.join("install").join("current.json").exists(),
            "{ending}"
        );
    }

    // The end journalled, `current.json` not yet retired.
    let root = prepared(temp.path(), "retire", &live, &target, &host);
    let planned = plan_id(&plan_update(&host, &root, C2, &[]));
    host.crash_before_retire.set(true);
    let stopped = apply_update(&host, &root, C2, &planned, &[]);
    assert_eq!(
        code(&stopped),
        "install_operation_interrupted",
        "{stopped:?}"
    );
    let operation = operation_id(&stopped);
    assert!(
        journal_of(&root, &operation)["finished_at_unix_ms"].is_u64(),
        "the end is journalled"
    );
    assert!(root.join("install").join("current.json").is_file());
    let resumed = on_operation(&host, "resume", &root, &operation);
    assert_done(&resumed, "install_resume");
    let ended = resumed.outcome.causes()[0].outcome();
    assert_eq!(ended.code().as_str(), "install_command_done", "{resumed:?}");
    assert_eq!(
        ended.value("operation"),
        Some(&json!("install_update_apply")),
        "{resumed:?}"
    );
    assert!(!root.join("install").join("current.json").exists());
    assert_eq!(normalised(&root), reference, "the tree is the update's");
    let undone = on_operation(&host, "undo", &root, &operation);
    assert_done(&undone, "install_undo");
    assert_tree_of(&root, &live, "undone after the resume");
}

// fix1-9u (brief item 9; review S1 §8.3): the undo of a completed update. Without an owner start
// since it ended, no gate; after one, the gate decides; an owner journal this installer cannot
// read refuses `processes_unreadable` (F6).
#[test]
fn inst1_fix1_9_the_undo_of_a_completed_update() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);

    // No owner started since the update ended: undone without the gate.
    let (root, applied) = updated(&temp, &host, "quiet", &live);
    host.log.borrow_mut().clear();
    let undone = on_operation(&host, "undo", &root, &applied);
    assert_done(&undone, "install_undo");
    assert!(
        !host
            .log
            .borrow()
            .iter()
            .any(|line| line.contains(" events ")),
        "no gate: {:?}",
        host.log.borrow()
    );
    assert_tree_of(&root, &live, "undone");
    assert_eq!(state_of(&root)["live"]["commit"], json!(C1));
    assert!(!root.join("install").join("current.json").exists());

    // An owner started since: the gate decides. The target's reader refusing a record refuses
    // the undo with nothing changed; once it reads, the update is undone.
    let (root, applied) = updated(&temp, &host, "started", &live);
    own_start_log(&root, even_phases(1_000));
    let ended = journal_of(&root, &applied)["finished_at_unix_ms"]
        .as_u64()
        .expect("the update's end");
    owner_started_at(&temp.path().join("started-state"), ended + 10);
    let before = program_tree(&root);
    host.programs.unreadable.replace(Some((C1.to_owned(), 120)));
    let refused = on_operation(&host, "undo", &root, &applied);
    host.programs.unreadable.replace(None);
    assert_refused(&refused, "install_undo", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["ledger_readability"]);
    assert_eq!(program_tree(&root), before, "nothing changed");
    let undone = on_operation(&host, "undo", &root, &applied);
    assert_done(&undone, "install_undo");
    assert_tree_of(&root, &live, "undone after the gate passed");

    // An owner journal this installer does not recognise: refused, nothing changed.
    let (root, applied) = updated(&temp, &host, "unrecognised", &live);
    fs::write(
        temp.path().join("unrecognised-state").join("owner.lock"),
        b"no owner record\n",
    )
    .expect("the owner journal");
    let before = program_tree(&root);
    let refused = on_operation(&host, "undo", &root, &applied);
    assert_refused(&refused, "install_undo", "processes_unreadable");
    assert_eq!(program_tree(&root), before, "nothing changed");
}

// fix1-9s (brief item 9; review S1 §8.4): `status --check-rollback` evaluates the whole gate
// toward the build the live one replaced, also while a Runtime runs, and names the first record
// the target's reader refuses.
#[test]
fn inst1_fix1_9_status_check_rollback_evaluates_the_whole_gate() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = updated(&temp, &host, "status", &BuildSpec::new(C1));
    own_start_log(&root, even_phases(1_000));
    let check = || {
        run(
            &host,
            &["status", "--root", word(&root), "--check-rollback"],
        )
    };

    host.processes.replace(vec![image(
        &root,
        "runtime/actingcommand-actingd.exe",
        41,
        false,
    )]);
    let status = check();
    assert_done(&status, "install_status");
    let gate = &status.members["rollback_gate"];
    assert_eq!(gate["previous"], json!(C1), "{gate}");
    assert_eq!(gate["passes"], json!(true), "{gate}");
    let rows = gate["parts"].as_array().expect("the parts");
    let parts = rows
        .iter()
        .map(|row| row["part"].as_str().expect("a part").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        parts,
        [
            "interface_edges",
            "ledger_readability",
            "open_time",
            "config"
        ],
        "{gate}"
    );
    assert!(rows.iter().all(|row| row["result"] == "pass"), "{gate}");
    assert_eq!(gate["refused"], Value::Null, "{gate}");

    host.programs.unreadable.replace(Some((C1.to_owned(), 120)));
    let status = check();
    host.programs.unreadable.replace(None);
    host.processes.replace(Vec::new());
    assert_done(&status, "install_status");
    let gate = &status.members["rollback_gate"];
    assert_eq!(gate["passes"], json!(false), "{gate}");
    let refused = &gate["refused"]["outcome"];
    assert_eq!(refused["code"], json!("install_command_refused"), "{gate}");
    assert_eq!(
        refused["values"]["install_refusal"],
        json!("rollback_gate_failed"),
        "{gate}"
    );
    assert_eq!(refused["values"]["ledger_sequence"], json!(120), "{gate}");
}

/// #411 P49 (brief rd-1028/brief-wave1.md W1-10; MCP-ENTRY review L9): the update plan's notice
/// asks for MCP clients to reconnect only when a renamed `mcp-serve` image was started by a
/// client itself. One that an MCP entry under the root started is switched to the live build by
/// that entry. The process probe reads each image's parent.
#[test]
fn p49_l9_the_notice_asks_no_reconnect_for_an_mcp_entrys_child() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let live = BuildSpec::new(C1);
    let target = BuildSpec::new(C2);
    let root = prepared(temp.path(), "root", &live, &target, &host);
    let entry = image(&root, "mcp-entry/g1/actingcommand-mcp-entry.exe", 50, false);
    let child = RunningImage {
        parent_pid: Some(50),
        ..image(&root, "runtime/actingctl.exe", 77, true)
    };
    host.processes.replace(vec![entry.clone(), child]);
    let planned = plan_update(&host, &root, C2, &[]);
    assert_done(&planned, "install_update_plan");
    assert_eq!(
        planned.members["running"]["renamed"][0]["pid"],
        json!(77),
        "{planned:?}"
    );
    assert_eq!(
        planned.members["notice"]["mcp_clients_reconnect"],
        json!(false),
        "{planned:?}"
    );

    // A server a client started itself: that client reconnects.
    let direct = RunningImage {
        parent_pid: Some(60),
        ..image(&root, "runtime/actingctl.exe", 78, true)
    };
    host.processes.replace(vec![entry, direct]);
    let planned = plan_update(&host, &root, C2, &[]);
    assert_done(&planned, "install_update_plan");
    assert_eq!(
        planned.members["notice"]["mcp_clients_reconnect"],
        json!(true),
        "{planned:?}"
    );
    host.processes.replace(Vec::new());

    // The probe's row names the parent.
    let plain = super::files::plain(&root);
    let rows = json!([{
        "pid": 77,
        "path": format!("{plain}\\runtime\\actingctl.exe"),
        "mcp": true,
        "parent": 50,
    }]);
    let parsed = super::host::parse_processes(&rows.to_string(), &root).expect("the probe's rows");
    assert_eq!(parsed.len(), 1, "{parsed:?}");
    assert_eq!(parsed[0].parent_pid, Some(50));
}
