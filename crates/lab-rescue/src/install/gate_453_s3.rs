// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S3 (register §1.364; contracts/install-cli.md, "Instances, the first slice" and
// "The Lab's rescue module"; contracts/scheduling-pause.md, "Pauses held at a first start";
// coordinator work order `rd-1114/brief-453-s3-cold-instance-add.md`; the design survey
// `rd-1113/fresh-root-design.md` §1(c) "1b. The cold commit"): the rescue module's `instance add`
// while no Runtime answers, `config replace` under the same stopped rule, the record of a hand-back
// the Runtime answers by holding the instance again, and the start after a cold add.
// - `s3_cold_add_swaps_alone_and_only_the_new_entry_starts_paused`: with no Runtime, the answer is
//   `install_command_done` with `runtime_running: false` and the steps `check_config` and `swap`
//   (no `pause_added`); the new entry is the six members of before and `start_paused: true` last,
//   and no other entry carries the member; the replaced file is kept as
//   `actingd.config.json.prev-<ms>`, byte for byte, and no part is left; one `config_replace`
//   journal with the swap's two steps, no hold and no S0, its input `instance_add` with
//   `start_paused: true`; nothing asked of a Runtime and nothing started;
// - `s3_cold_add_refuses_a_starting_runtime_a_held_owner_lock_or_an_undecided_probe`: an actingd
//   under the root refuses `runtime_running` (its `pid`), the state root's owner lock held
//   `owner_lock_held`, an owner journal that cannot be read `processes_unreadable` (caused by
//   `watchdog_journal_unreadable`); each leaves the root byte for byte as it was, no journal;
// - `s3_cold_add_a_refused_candidate_is_kept_and_listed`: a live build whose check-config refuses
//   the candidate ends `config_refused_by_target` with check-config's own refusal as its cause,
//   nothing swapped, no journal; the one new file is the candidate's part (its new entry starts
//   paused), which `leftovers` lists;
// - `s3_config_replace_with_no_runtime_applies_the_stopped_rule_then_swaps`: `config replace`
//   with no Runtime answering refuses the same three (nothing swapped, no journal, nothing asked);
//   with the Runtime stopped it swaps as before (the swap alone, `runtime_running: false`, the
//   replaced file kept);
// - `s3_a_crash_after_each_step_resumes_to_the_same_tree`: a cold add stopped after its step
//   `stage_file`, and after its step `swap_file` (the fake host's crash), ends
//   `install_operation_interrupted`; `op resume` finishes it, and the root outside `install\` has
//   the files a cold add without a crash leaves (the kept name's milliseconds aside): the live
//   file the journal's candidate, the replaced one kept, no part, `current.json` retired;
// - `s3_a_hand_back_that_meets_an_unapplied_entry_records_it_held_again` (ruling 4): the Runtime
//   answers the module's `release-hold` of one instance by holding it again with `instance.added`
//   (as a Runtime with S2 answers for an entry its start left unapplied): the answer's `count` and
//   `release.resumed` name only the instance resumed, `release.held_again` the other with
//   `instance.added` and the revision the Runtime answered, and the journal's release step and its
//   hand-back record the same;
// - `s3_a_start_after_a_cold_add_expects_the_first_start_pause` (ruling 5): after a stop with a
//   hold and a cold add, a start whose Runtime holds the new instance with `instance.added` passes
//   the post-start row `pauses`, which names it in `start_paused`, and the answer carries no
//   `post_check_mismatch`; the same start with another reason on the new instance fails the row
//   and is the caveat `post_check_mismatch`.
// Fix 1 (the coordinator's answers to the slice's four questions and the independent review):
// - `s3_fix1_resume_of_a_cold_commit_applies_the_stopped_rule`: a cold add stopped after its first
//   step; `op resume` under an owner lock held is `owner_lock_held`, with an actingd under the root
//   `runtime_running`, each with the root and the journal as they were; once stopped it is done;
// - `s3_fix1_undo_of_a_cold_commit_applies_the_stopped_rule_and_restores_a_half_done_swap`: a cold
//   add stopped between the swap's two renames (no live configuration); `undo` under an owner lock
//   held and with an actingd under the root refuses the same way, nothing moved; once stopped it
//   puts the kept file back byte for byte and sets the candidate aside, no part left;
// - `s3_fix1_an_added_pause_on_an_entry_without_the_member_still_fails_the_row`: the start after a
//   cold add whose entry no longer declares `start_paused` (a person took the member out) holds the
//   new instance with `instance.added`: the row `pauses` fails, names no `start_paused`, and the
//   answer is the caveat `post_check_mismatch`;
// - `s3_fix1_the_hot_path_records_no_start_paused`: with a Runtime answering, the answer's `added`
//   and the journal's input `instance_add` carry no `start_paused`, the new entry is the six members
//   of before and no entry carries the member;
// - `s3_fix1_config_restore_with_no_runtime_applies_the_stopped_rule_then_swaps`: `config restore`
//   with no Runtime answering refuses the same three as `config replace`, then swaps when stopped.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// emulator, no sleep; every root is a temporary folder with neutral files.

use super::fixture::{
    BuildSpec, C1, FakeHost, code, live_root, run, sha256_hex, tree, word, write_json,
};
use super::host::RunningImage;
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The initiator the tests run the module as: a Lab process the UI started (§4.1).
const WORD: Initiator = Initiator::LabRescue;
const GAME: &str = "neutral-game";
const SERVER: &str = "neutral-server";
const HOST_ID: &str = "neutral.host";
/// The alias the tests add.
const ADDED: &str = "neutral.added";
const ID_A: &str = "instance_0000000000000000000000000000000a";
const ID_B: &str = "instance_0000000000000000000000000000000b";
/// The application id the game root declares for [`SERVER`] (`applications.json`).
const DECLARED: &str = "neutral.application";
/// The reason of the module's own hold in the tests that stop the Runtime first.
const HOLD: &str = "neutral-hold";
/// The process id of an actingd starting under the root (one that answers no status yet).
const STARTING_PID: u32 = 41;
/// The members of an entry the cold path adds, in the file's order: the six of before, then
/// `start_paused`.
const COLD_ENTRY_MEMBERS: [&str; 7] = [
    "alias",
    "instance_id",
    "application_id",
    "instance_index",
    "touch_backend",
    "capture_backend",
    "start_paused",
];

fn entry(alias: &str, instance_id: &str, index: u16) -> Value {
    json!({
        "alias": alias,
        "instance_id": instance_id,
        "instance_index": index,
        "touch_backend": "neutral-touch",
        "capture_backend": "neutral-capture",
    })
}

fn row(alias: &str) -> Value {
    json!({
        "instance_id": alias,
        "server_id": SERVER,
        "game_id": GAME,
        "host_id": HOST_ID,
        "available": true,
        "capability_operation_ids": ["operation.neutral"],
        "preferred_task_ids": [],
    })
}

/// A root whose live build is C1, adopted, whose configuration has the emulator instances
/// `neutral.a` (index 0) and `neutral.b` (index 1) in both places, and whose standard-pack root
/// holds the game root of [`GAME`] with one pack of [`SERVER`] and the application [`DECLARED`]
/// for it, as `gate_421_inst5` makes it; no Runtime runs.
fn installed(base: &Path, name: &str, host: &FakeHost) -> PathBuf {
    let root = live_root(base, name, &BuildSpec::new(C1));
    let packs = base.join(format!("{name}-packs"));
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration")).expect("JSON");
    config["standard_packs"] = json!({"root": packs});
    config["instances"] = json!([entry("neutral.a", ID_A, 0), entry("neutral.b", ID_B, 1)]);
    config["policy"] = json!({
        "facts": {
            "ledger_position": 0,
            "fact_snapshot_id": "neutral",
            "facts": [],
            "outcomes": [],
            "tasks": [],
            "instances": [row("neutral.a"), row("neutral.b")],
        },
        "resources": {
            "pools": [],
            "hosts": [{
                "host_id": HOST_ID,
                "cpu_available_milli": 1000,
                "gpu_available_milli": 1000,
                "io_available_milli": 1000,
            }],
        },
    });
    write_json(&config_path, &config);
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    let game_root = packs.join(GAME);
    fs::create_dir_all(&game_root).expect("the game root");
    write_json(
        &game_root.join("bundle.json"),
        &json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": [{
                "package_id": "neutral.pack",
                "server": SERVER,
                "entry_task_id": "neutral.entry",
                "path": "packs/neutral.pack",
                "file_count": 1,
                "byte_count": 2,
            }],
        }),
    );
    write_json(
        &game_root.join("applications.json"),
        &json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {SERVER: {"application_id": DECLARED}},
        }),
    );
    root
}

/// The state root `installed` gives the root `name` below `base`.
fn state_of(base: &Path, name: &str) -> PathBuf {
    base.join(format!("{name}-state"))
}

/// The rescue module's location of `root`: its configuration and the state root it names.
fn location(root: &Path) -> actingcommand_lab_ops::locate::Location {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    rescue::location_of(root, &config, &state_root)
}

/// The rescue row `name` with `params` on `root`, as [`WORD`]; its one line and exit code.
fn rescue_line(host: &FakeHost, root: &Path, name: &str, params: Value) -> (Value, u8) {
    rescue_at(host, &location(root), name, params)
}

/// The rescue row `name` with `params` at `location` (read before the live configuration went
/// away), as [`WORD`]; its one line and exit code.
fn rescue_at(
    host: &FakeHost,
    location: &actingcommand_lab_ops::locate::Location,
    name: &str,
    params: Value,
) -> (Value, u8) {
    let context = Context {
        host,
        location,
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

/// `instance add` of [`ADDED`] for [`GAME`] on [`SERVER`] at the emulator index 2.
fn add(host: &FakeHost, root: &Path) -> (Value, u8) {
    rescue_line(
        host,
        root,
        "instance add",
        json!({
            "alias": ADDED,
            "game": GAME,
            "server": SERVER,
            "mumu_index": 2,
            "touch_backend": "neutral-touch",
            "capture_backend": "neutral-capture",
        }),
    )
}

fn top_code(line: &Value) -> &str {
    line["outcome"]["code"].as_str().unwrap_or_default()
}

fn top_value<'a>(line: &'a Value, key: &str) -> &'a Value {
    &line["outcome"]["values"][key]
}

/// A refusal of the command `operation` with `install_refusal` `refusal`, exit 1.
fn assert_refused(line: &Value, exit: u8, operation: &str, refusal: &str) {
    assert_eq!(exit, 1, "{refusal}: {line}");
    assert_eq!(
        top_code(line),
        "install_command_refused",
        "{refusal}: {line}"
    );
    assert_eq!(top_value(line, "operation"), operation, "{refusal}: {line}");
    assert_eq!(
        top_value(line, "install_refusal"),
        refusal,
        "{refusal}: {line}"
    );
}

/// Every journal of the root, by operation id.
fn journals(root: &Path) -> Vec<(String, Value)> {
    let directory = root.join("install").join("operations");
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut found = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?.to_owned();
            id.starts_with("op-").then(|| {
                let journal = serde_json::from_slice(&fs::read(entry.path()).expect("a journal"))
                    .expect("journal JSON");
                (id, journal)
            })
        })
        .collect::<Vec<_>>();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The journals written since `before`.
fn new_journals(root: &Path, before: &[(String, Value)]) -> Vec<(String, Value)> {
    journals(root)
        .into_iter()
        .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
        .collect()
}

/// What the fake Runtime was asked (pauses, resumes, closes, spawns), in order.
fn calls(host: &FakeHost) -> Vec<String> {
    host.runtime.calls.borrow().clone()
}

/// The configuration file's document.
fn config_of(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("actingd.config.json")).expect("the configuration"))
        .expect("JSON")
}

/// The member of the list at `pointer` of `config` whose `member` is `value`.
fn member_of<'a>(config: &'a Value, pointer: &str, member: &str, value: &str) -> Option<&'a Value> {
    config
        .pointer(pointer)
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find(|item| item[member] == value))
}

/// The names of an entry's members, in the file's order.
fn members_of(entry: &Value) -> Vec<&str> {
    entry
        .as_object()
        .map(|members| members.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// The aliases of the entries of `config` that carry the member `start_paused`.
fn starting_paused(config: &Value) -> Vec<String> {
    config["instances"]
        .as_array()
        .expect("the instances")
        .iter()
        .filter(|entry| entry.get("start_paused").is_some())
        .filter_map(|entry| entry["alias"].as_str().map(str::to_owned))
        .collect()
}

/// `document` without the entry and the row of [`ADDED`].
fn without_added(mut document: Value) -> Value {
    for (pointer, member) in [
        ("/instances", "alias"),
        ("/policy/facts/instances", "instance_id"),
    ] {
        if let Some(Value::Array(items)) = document.pointer_mut(pointer) {
            items.retain(|item| item[member] != ADDED);
        }
    }
    document
}

/// The names of the steps of an answer, in order.
fn step_names(line: &Value) -> Vec<&str> {
    line["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .filter_map(|step| step["step"].as_str())
        .collect()
}

/// The kinds of the steps of a journal, in order.
fn step_kinds(journal: &Value) -> Vec<&str> {
    journal["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .filter_map(|step| step["kind"].as_str())
        .collect()
}

/// The step of `kind` of a journal.
fn step_of<'a>(journal: &'a Value, kind: &str) -> &'a Value {
    journal["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["kind"] == kind))
        .unwrap_or_else(|| panic!("no step {kind}: {journal}"))
}

/// The files directly in `root` whose name holds `.part`.
fn parts(root: &Path) -> Vec<String> {
    tree(root)
        .into_keys()
        .filter(|name| !name.contains('/') && name.contains(".part"))
        .collect()
}

/// An actingd running under `root` with `pid` (a Runtime starting, before it answers a status
/// read or holds its owner lock).
fn actingd_image(root: &Path, pid: u32) -> RunningImage {
    RunningImage {
        pid,
        path: format!(
            "{}\\runtime\\actingcommand-actingd.exe",
            super::files::plain(root)
        ),
        mcp_serve: false,
        parent_pid: None,
    }
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

/// The owner journal of the state root `state` locked as a running Runtime locks it, until the
/// file returned is dropped.
fn hold_owner_lock(state: &Path) -> fs::File {
    let mut owner = fs::File::create(state.join("owner.lock")).expect("the owner journal");
    owner
        .write_all(format!("{}\n", owner_record()).as_bytes())
        .expect("write the owner journal");
    owner.lock().expect("hold the owner lock");
    owner
}

/// An owner journal of the state root `state` that cannot be read: longer than the limit the
/// watchdog's reader takes, so the stopped rule's probe cannot decide.
fn unreadable_owner_journal(state: &Path) {
    let length = usize::try_from(actingcommand_contract::OWNER_JOURNAL_LIMIT + 1)
        .expect("a length in memory");
    fs::write(state.join("owner.lock"), vec![b' '; length]).expect("the owner journal");
}

/// Whether the line carries, as its top or as a link at any depth, an outcome `code` whose value
/// `key` is `value`.
fn carries(line: &Value, code: &str, key: &str, value: &Value) -> bool {
    match line {
        Value::Object(members) => {
            (members.get("code").and_then(Value::as_str) == Some(code)
                && members.get("values").and_then(|values| values.get(key)) == Some(value))
                || members
                    .values()
                    .any(|inner| carries(inner, code, key, value))
        }
        Value::Array(items) => items.iter().any(|inner| carries(inner, code, key, value)),
        _ => false,
    }
}

/// `name` with the milliseconds after every `.prev-` and `.new-` replaced by `*`.
fn normalised(name: &str) -> String {
    let mut normal = String::new();
    let mut rest = name;
    while let Some(at) = [".prev-", ".new-"]
        .iter()
        .filter_map(|marker| rest.find(marker).map(|index| index + marker.len()))
        .min()
    {
        normal.push_str(&rest[..at]);
        normal.push('*');
        rest = rest[at..].trim_start_matches(|character: char| character.is_ascii_digit());
    }
    normal.push_str(rest);
    normal
}

/// The files of `root` outside `install\`, by relative path, each kept name's milliseconds `*`.
fn shape(root: &Path) -> BTreeSet<String> {
    tree(root)
        .into_keys()
        .filter(|name| !name.starts_with("install/"))
        .map(|name| normalised(&name))
        .collect()
}

// Build points 1 and 6 (the answer and the file); ruling 3 (the member only on this path).
#[test]
fn s3_cold_add_swaps_alone_and_only_the_new_entry_starts_paused() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = installed(temp.path(), "cold", &host);
    assert!(!host.runtime.running.get());
    let old = fs::read(root.join("actingd.config.json")).expect("the configuration");
    let before = journals(&root);

    let (line, exit) = add(&host, &root);
    assert_eq!(exit, 0, "{line}");
    assert_eq!(top_code(&line), "install_command_done", "{line}");
    assert_eq!(top_value(&line, "operation"), "install_instance_add");
    assert_eq!(top_value(&line, "alias"), ADDED);
    assert_eq!(top_value(&line, "game"), GAME);
    assert_eq!(line["runtime_running"], json!(false), "{line}");
    assert_eq!(step_names(&line), ["check_config", "swap"], "{line}");
    assert!(line.get("hold").is_none(), "{line}");
    assert_eq!(line["added"]["start_paused"], json!(true), "{line}");

    // The file: the new entry with the six members of before and `start_paused: true` last, its
    // row, and no other entry with the member.
    let config = config_of(&root);
    let added = member_of(&config, "/instances", "alias", ADDED).expect("the instance's entry");
    assert_eq!(members_of(added), COLD_ENTRY_MEMBERS, "{added}");
    assert_eq!(added["start_paused"], json!(true), "{added}");
    assert_eq!(added["instance_index"], json!(2), "{added}");
    assert_eq!(added["application_id"], DECLARED, "{added}");
    assert_eq!(starting_paused(&config), [ADDED], "{config}");
    let row = member_of(&config, "/policy/facts/instances", "instance_id", ADDED)
        .expect("the instance's policy row");
    assert_eq!(row["host_id"], HOST_ID, "{row}");
    assert_eq!(line["added"]["instance_id"], added["instance_id"], "{line}");

    // The replaced file kept, byte for byte; the candidate was the swap's own part: none left.
    let backup = PathBuf::from(line["backup"].as_str().expect("the backup"));
    let name = backup
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert!(name.starts_with("actingd.config.json.prev-"), "{name}");
    assert_eq!(fs::read(&backup).expect("the kept file"), old);
    assert!(parts(&root).is_empty(), "{:?}", parts(&root));

    // One operation, the swap alone: no hold, no S0, the initiator and what was added.
    let written = new_journals(&root, &before);
    assert_eq!(written.len(), 1, "{written:?}");
    let (id, journal) = &written[0];
    assert_eq!(top_value(&line, "install_operation_id"), id.as_str());
    assert_eq!(journal["kind"], "config_replace", "{journal}");
    assert_eq!(journal["initiator"], WORD.as_str(), "{journal}");
    assert_eq!(
        step_kinds(journal),
        ["stage_file", "swap_file"],
        "{journal}"
    );
    assert!(journal["inputs"]["hold"].is_null(), "{journal}");
    assert!(journal["s0"].is_null(), "{journal}");
    let recorded = &journal["inputs"]["instance_add"];
    assert_eq!(recorded["start_paused"], json!(true), "{recorded}");
    assert_eq!(recorded["pause_reason"], "instance.added", "{recorded}");
    assert_eq!(recorded["instance_id"], added["instance_id"], "{recorded}");
    assert!(!root.join("install").join("current.json").exists());

    // Nothing asked of a Runtime, nothing started.
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));
    assert!(!host.runtime.running.get());
}

// Build point 1 and ruling 1: the update path's stopped rule, before the candidate is written.
#[test]
fn s3_cold_add_refuses_a_starting_runtime_a_held_owner_lock_or_an_undecided_probe() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = installed(temp.path(), "stopped", &host);
    let state = state_of(temp.path(), "stopped");
    let before = tree(&root);
    let journals_before = journals(&root);
    let unchanged = |case: &str| {
        assert_eq!(tree(&root), before, "{case}");
        assert!(new_journals(&root, &journals_before).is_empty(), "{case}");
        assert!(calls(&host).is_empty(), "{case}: {:?}", calls(&host));
    };

    // An actingd under the root that answers no status read yet.
    host.processes
        .replace(vec![actingd_image(&root, STARTING_PID)]);
    let (refused, exit) = add(&host, &root);
    assert_refused(&refused, exit, "install_instance_add", "runtime_running");
    assert_eq!(
        top_value(&refused, "pid"),
        &json!(STARTING_PID),
        "{refused}"
    );
    unchanged("runtime_running");
    host.processes.replace(Vec::new());

    // The state root's owner lock held.
    let owner = hold_owner_lock(&state);
    let (refused, exit) = add(&host, &root);
    drop(owner);
    assert_refused(&refused, exit, "install_instance_add", "owner_lock_held");
    unchanged("owner_lock_held");

    // A probe that cannot decide: the owner journal cannot be read.
    unreadable_owner_journal(&state);
    let (refused, exit) = add(&host, &root);
    assert_refused(
        &refused,
        exit,
        "install_instance_add",
        "processes_unreadable",
    );
    assert_eq!(
        refused["outcome"]["causes"][0]["code"], "watchdog_journal_unreadable",
        "{refused}"
    );
    unchanged("processes_unreadable");
}

// Build point 2: a check-config refusal before the operation opens leaves the candidate's part.
#[test]
fn s3_cold_add_a_refused_candidate_is_kept_and_listed() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = installed(temp.path(), "refused", &host);
    host.programs
        .refuse_check_config
        .borrow_mut()
        .push("live".to_owned());
    let config = fs::read(root.join("actingd.config.json")).expect("the configuration");
    let before = tree(&root);
    let journals_before = journals(&root);

    let (refused, exit) = add(&host, &root);
    assert_refused(
        &refused,
        exit,
        "install_instance_add",
        "config_refused_by_target",
    );
    // check-config's own refusal, relayed whole as the cause.
    assert!(
        refused["outcome"]
            .to_string()
            .contains("config_decode_failed"),
        "{refused}"
    );

    // Nothing swapped, no journal, nothing asked; the one new file is the candidate's part, whose
    // new entry starts paused.
    assert_eq!(
        fs::read(root.join("actingd.config.json")).expect("the configuration"),
        config
    );
    assert!(new_journals(&root, &journals_before).is_empty());
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));
    let after = tree(&root);
    let added = after
        .keys()
        .filter(|name| !before.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 1, "{added:?}");
    let part = added[0].clone();
    assert!(
        part.starts_with("actingd.config.json.new-") && part.ends_with(".part"),
        "{part}"
    );
    for (name, bytes) in &before {
        assert_eq!(after.get(name), Some(bytes), "{name}");
    }
    let candidate: Value = serde_json::from_slice(&after[&part]).expect("the candidate's JSON");
    assert_eq!(starting_paused(&candidate), [ADDED], "{candidate}");

    // `leftovers` lists it.
    let (listed, exit) = rescue_line(&host, &root, "leftovers", json!({}));
    assert_eq!(exit, 0, "{listed}");
    assert!(
        listed["leftovers"]
            .as_array()
            .expect("the leftovers")
            .iter()
            .any(|row| row["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(part.as_str()))),
        "{listed}"
    );
}

/// Build point 4 and ruling 2: the rescue row `row` (`config replace` or `config restore`, whose
/// file is its parameter `member` and whose outcomes name `operation`) with no Runtime answering:
/// the three refusals of the stopped rule, nothing swapped; then, stopped, the swap alone.
fn swap_while_stopped(row: &str, member: &str, operation: &str) {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = installed(temp.path(), "replace", &host);
    let state = state_of(temp.path(), "replace");
    let old = fs::read(root.join("actingd.config.json")).expect("the configuration");
    // The same document in other bytes, so that the file changes.
    let candidate = temp.path().join("candidate.json");
    fs::write(
        &candidate,
        serde_json::to_vec(&config_of(&root)).expect("JSON"),
    )
    .expect("the candidate");
    let mut params = json!({});
    params[member] = json!(word(&candidate));
    let journals_before = journals(&root);
    let nothing_swapped = |case: &str| {
        assert_eq!(
            fs::read(root.join("actingd.config.json")).expect("the configuration"),
            old,
            "{case}"
        );
        assert!(new_journals(&root, &journals_before).is_empty(), "{case}");
        assert!(
            !root.join("install").join("current.json").exists(),
            "{case}"
        );
        assert!(calls(&host).is_empty(), "{case}: {:?}", calls(&host));
    };

    host.processes
        .replace(vec![actingd_image(&root, STARTING_PID)]);
    let (refused, exit) = rescue_line(&host, &root, row, params.clone());
    assert_refused(&refused, exit, operation, "runtime_running");
    assert_eq!(
        top_value(&refused, "pid"),
        &json!(STARTING_PID),
        "{refused}"
    );
    nothing_swapped("runtime_running");
    host.processes.replace(Vec::new());

    let owner = hold_owner_lock(&state);
    let (refused, exit) = rescue_line(&host, &root, row, params.clone());
    drop(owner);
    assert_refused(&refused, exit, operation, "owner_lock_held");
    nothing_swapped("owner_lock_held");

    unreadable_owner_journal(&state);
    let (refused, exit) = rescue_line(&host, &root, row, params.clone());
    assert_refused(&refused, exit, operation, "processes_unreadable");
    nothing_swapped("processes_unreadable");

    // Stopped (an owner journal no Runtime holds): the swap alone, as before.
    fs::write(
        state.join("owner.lock"),
        format!("{}\n", owner_record()).as_bytes(),
    )
    .expect("the owner journal");
    let (replaced, exit) = rescue_line(&host, &root, row, params);
    assert_eq!(exit, 0, "{replaced}");
    assert_eq!(top_code(&replaced), "install_command_done", "{replaced}");
    assert_eq!(top_value(&replaced, "operation"), operation, "{replaced}");
    assert_eq!(replaced["runtime_running"], json!(false), "{replaced}");
    assert_eq!(
        step_names(&replaced),
        ["check_config", "swap"],
        "{replaced}"
    );
    let backup = PathBuf::from(replaced["backup"].as_str().expect("the backup"));
    assert_eq!(fs::read(&backup).expect("the kept file"), old);
    assert_eq!(
        fs::read(root.join("actingd.config.json")).expect("the configuration"),
        fs::read(&candidate).expect("the candidate")
    );
    let written = new_journals(&root, &journals_before);
    assert_eq!(written.len(), 1, "{written:?}");
    assert_eq!(
        step_kinds(&written[0].1),
        ["stage_file", "swap_file"],
        "{written:?}"
    );
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));
}

// Build point 4 and ruling 2: `config replace` with no Runtime answering.
#[test]
fn s3_config_replace_with_no_runtime_applies_the_stopped_rule_then_swaps() {
    swap_while_stopped("config replace", "file", "install_config_replace");
}

// Fix 1 (the review): `config restore`, on `config replace`'s path, with no Runtime answering.
#[test]
fn s3_fix1_config_restore_with_no_runtime_applies_the_stopped_rule_then_swaps() {
    swap_while_stopped("config restore", "backup", "install_config_restore");
}

// Build point 2 and the work order's crash test: a crash after each of the operation's two steps.
// (A crash between the two renames of the swap leaves no live configuration, which `resume` cannot
// load, a known limit of every `config_replace`; `undo` puts it back:
// `s3_fix1_undo_of_a_cold_commit_applies_the_stopped_rule_and_restores_a_half_done_swap`.)
#[test]
fn s3_a_crash_after_each_step_resumes_to_the_same_tree() {
    let temp = TempDir::new().expect("tempdir");
    // What a cold add without a crash leaves outside `install\`.
    let clean_host = FakeHost::new(&temp.path().join("clean-work"));
    let clean_root = installed(temp.path(), "clean", &clean_host);
    let (line, exit) = add(&clean_host, &clean_root);
    assert_eq!(exit, 0, "{line}");
    let clean = shape(&clean_root);
    assert!(
        clean.contains("actingd.config.json.prev-*") && clean.contains("actingd.config.json"),
        "{clean:?}"
    );

    for (name, step) in [("after-stage", 1), ("after-swap", 2)] {
        let host = FakeHost::new(&temp.path().join(format!("{name}-work")));
        let root = installed(temp.path(), name, &host);
        let old = fs::read(root.join("actingd.config.json")).expect("the configuration");
        let old_document = config_of(&root);
        let before = journals(&root);
        host.crash_after.set(Some(step));

        let (stopped, exit) = add(&host, &root);
        assert_ne!(exit, 0, "{name}: {stopped}");
        assert_eq!(
            top_code(&stopped),
            "install_operation_interrupted",
            "{name}: {stopped}"
        );
        let written = new_journals(&root, &before);
        assert_eq!(written.len(), 1, "{name}: {written:?}");
        let operation_id = written[0].0.clone();
        assert!(root.join("install").join("current.json").exists(), "{name}");

        let (resumed, exit) = rescue_line(&host, &root, "op resume", json!({"op": operation_id}));
        assert_eq!(exit, 0, "{name}: {resumed}");
        assert_eq!(
            top_code(&resumed),
            "install_command_done",
            "{name}: {resumed}"
        );

        // The same tree as without the crash; the live file the journal's candidate, the
        // replaced one kept, nothing left unfinished.
        assert_eq!(shape(&root), clean, "{name}");
        let journal = new_journals(&root, &before)
            .into_iter()
            .find(|(id, _)| *id == operation_id)
            .map(|(_, journal)| journal)
            .expect("the journal");
        let live = fs::read(root.join("actingd.config.json")).expect("the configuration");
        assert_eq!(
            journal["inputs"]["config_sha256"],
            format!("sha256:{}", sha256_hex(&live)),
            "{name}: {journal}"
        );
        assert!(journal["finished_at_unix_ms"].is_u64(), "{name}: {journal}");
        let backup = PathBuf::from(journal["inputs"]["backup"].as_str().expect("the backup"));
        assert_eq!(fs::read(&backup).expect("the kept file"), old, "{name}");
        let config = config_of(&root);
        assert_eq!(starting_paused(&config), [ADDED], "{name}: {config}");
        assert_eq!(without_added(config), old_document, "{name}");
        assert!(parts(&root).is_empty(), "{name}: {:?}", parts(&root));
        assert!(
            !root.join("install").join("current.json").exists(),
            "{name}"
        );
        assert!(calls(&host).is_empty(), "{name}: {:?}", calls(&host));
    }
}

// Ruling 4: a hand-back that meets an entry the start left unapplied, as a Runtime with S2 answers
// it, says what happened in the record and in the answer.
#[test]
fn s3_a_hand_back_that_meets_an_unapplied_entry_records_it_held_again() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = installed(temp.path(), "held-again", &host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    let (down, exit) = rescue_line(&host, &root, "down", json!({"reason": HOLD}));
    assert_eq!(exit, 0, "{down}");
    let (up, exit) = rescue_line(&host, &root, "up", json!({}));
    assert_eq!(exit, 0, "{up}");
    assert_eq!(host.runtime.pause_of("neutral.b").as_deref(), Some(HOLD));
    host.runtime
        .holds_at_release
        .borrow_mut()
        .push("neutral.b".to_owned());
    let asked = calls(&host).len();
    let before = journals(&root);

    let (released, exit) = rescue_line(&host, &root, "release-hold", json!({"reason": HOLD}));
    assert_eq!(exit, 0, "{released}");
    assert_eq!(top_code(&released), "install_command_done", "{released}");
    // Both asked; one resumed, the other held again with `instance.added`.
    assert_eq!(
        calls(&host)[asked..].to_vec(),
        ["resume neutral.a", "resume neutral.b"]
    );
    assert_eq!(host.runtime.pause_of("neutral.a"), None);
    let revision = host
        .runtime
        .instances
        .borrow()
        .iter()
        .find(|instance| instance.alias == "neutral.b")
        .and_then(|instance| instance.pause.as_ref())
        .filter(|pause| pause.reason.as_str() == "instance.added")
        .map(|pause| pause.revision)
        .expect("neutral.b held again with instance.added");
    let held_again =
        json!([{"alias": "neutral.b", "reason": "instance.added", "revision": revision}]);

    // The answer: `count` the instance resumed; `release` says which was held again.
    assert_eq!(top_value(&released, "count"), &json!(1), "{released}");
    assert_eq!(
        released["release"]["resumed"],
        json!(["neutral.a"]),
        "{released}"
    );
    assert_eq!(released["release"]["held_again"], held_again, "{released}");

    // The record: the release step and its hand-back say the same.
    let written = new_journals(&root, &before);
    assert_eq!(written.len(), 1, "{written:?}");
    let evidence = &step_of(&written[0].1, "release_hold")["evidence"];
    assert_eq!(evidence["resumed"], json!(["neutral.a"]), "{evidence}");
    assert_eq!(evidence["held_again"], held_again, "{evidence}");
    let rows = evidence["handed_back"]["instances"]
        .as_array()
        .expect("the hand-back's rows");
    let resumed_of = |alias: &str| {
        rows.iter()
            .find(|row| row["alias"] == alias)
            .map(|row| row["resumed"].clone())
    };
    assert_eq!(resumed_of("neutral.a"), Some(json!(true)), "{evidence}");
    assert_eq!(resumed_of("neutral.b"), Some(json!(false)), "{evidence}");
}

/// Ruling 5: on a root `name` below `base` whose Runtime runs `neutral.a` and `neutral.b`, `down`
/// with [`HOLD`], a cold add of [`ADDED`] (fix 1: then, unless `declares`, its entry rewritten
/// without `start_paused`, as a person may), then `up`, whose Runtime registers the new instance
/// and holds it with `reason`; the answer of `up`, its exit code and its post-start row `pauses`.
fn start_after_cold_add(
    base: &Path,
    name: &str,
    reason: &str,
    declares: bool,
) -> (Value, u8, Value) {
    let host = FakeHost::new(&base.join(format!("{name}-work")));
    let root = installed(base, name, &host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    let (down, exit) = rescue_line(&host, &root, "down", json!({"reason": HOLD}));
    assert_eq!(exit, 0, "{name}: {down}");
    assert!(!host.runtime.running.get(), "{name}");
    let (added, exit) = add(&host, &root);
    assert_eq!(exit, 0, "{name}: {added}");
    assert_eq!(added["runtime_running"], json!(false), "{name}: {added}");
    if !declares {
        let mut config = config_of(&root);
        if let Some(Value::Array(entries)) = config.get_mut("instances") {
            for entry in entries.iter_mut().filter(|entry| entry["alias"] == ADDED) {
                if let Value::Object(members) = entry {
                    members.remove("start_paused");
                }
            }
        }
        assert!(starting_paused(&config).is_empty(), "{name}: {config}");
        write_json(&root.join("actingd.config.json"), &config);
    }
    host.runtime
        .registers_at_start
        .borrow_mut()
        .push(ADDED.to_owned());
    host.runtime
        .pauses_at_start
        .borrow_mut()
        .push((ADDED.to_owned(), reason.to_owned()));
    let (up, exit) = rescue_line(&host, &root, "up", json!({}));
    assert_eq!(top_code(&up), "install_command_done", "{name}: {up}");
    assert_eq!(top_value(&up, "operation"), "install_start", "{name}: {up}");
    let pauses = up["post_check"]["checks"]
        .as_array()
        .expect("the post-start rows")
        .iter()
        .find(|row| row["id"] == "pauses")
        .cloned()
        .unwrap_or_else(|| panic!("{name}: no row pauses: {up}"));
    (up, exit, pauses)
}

// Ruling 5: the start after a cold add.
#[test]
fn s3_a_start_after_a_cold_add_expects_the_first_start_pause() {
    let temp = TempDir::new().expect("tempdir");
    let mismatch = json!("post_check_mismatch");

    // The new instance held with `instance.added`, its entry declaring `start_paused`: expected.
    let (up, exit, pauses) = start_after_cold_add(temp.path(), "expected", "instance.added", true);
    assert_eq!(exit, 0, "{up}");
    assert_eq!(pauses["result"], "pass", "{pauses}");
    assert_eq!(pauses["values"]["start_paused"], json!([ADDED]), "{pauses}");
    assert_eq!(
        pauses["values"]["actual"][ADDED], "instance.added",
        "{pauses}"
    );
    assert!(
        pauses["values"]["expected"].get(ADDED).is_none(),
        "{pauses}"
    );
    assert_eq!(up["post_check"]["failed"], json!([]), "{up}");
    assert!(
        !carries(&up, "install_caveat_noted", "install_caveat", &mismatch),
        "{up}"
    );

    // Another reason on the new instance is an extra pause: the row fails, the caveat says so.
    let (up, exit, pauses) =
        start_after_cold_add(temp.path(), "other", "neutral-other-party", true);
    assert_eq!(exit, 0, "{up}");
    assert_eq!(pauses["result"], "fail", "{pauses}");
    assert!(pauses["values"].get("start_paused").is_none(), "{pauses}");
    assert_eq!(
        pauses["values"]["actual"][ADDED], "neutral-other-party",
        "{pauses}"
    );
    assert_eq!(up["post_check"]["failed"], json!(["pauses"]), "{up}");
    assert!(
        carries(&up, "install_caveat_noted", "install_caveat", &mismatch),
        "{up}"
    );
}

// Fix 1 (the review): `instance.added` on an instance whose entry does not declare `start_paused`
// is not the Runtime's first-start hold: the row fails, and the start's answer is the caveat.
#[test]
fn s3_fix1_an_added_pause_on_an_entry_without_the_member_still_fails_the_row() {
    let temp = TempDir::new().expect("tempdir");
    let (up, exit, pauses) =
        start_after_cold_add(temp.path(), "undeclared", "instance.added", false);
    assert_eq!(exit, 0, "{up}");
    assert_eq!(pauses["result"], "fail", "{pauses}");
    assert!(pauses["values"].get("start_paused").is_none(), "{pauses}");
    assert_eq!(
        pauses["values"]["actual"][ADDED], "instance.added",
        "{pauses}"
    );
    assert!(
        pauses["values"]["expected"].get(ADDED).is_none(),
        "{pauses}"
    );
    assert_eq!(up["post_check"]["failed"], json!(["pauses"]), "{up}");
    assert!(
        carries(
            &up,
            "install_caveat_noted",
            "install_caveat",
            &json!("post_check_mismatch")
        ),
        "{up}"
    );
}

// Fix 1 (the review): with a Runtime answering, the answer's `added`, the journal's input and the
// file carry no `start_paused`.
#[test]
fn s3_fix1_the_hot_path_records_no_start_paused() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = installed(temp.path(), "hot", &host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    host.runtime
        .registers_at_start
        .borrow_mut()
        .push(ADDED.to_owned());
    let before = journals(&root);

    let (line, exit) = add(&host, &root);
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["runtime_running"], json!(true), "{line}");
    assert_eq!(
        step_names(&line),
        ["check_config", "swap", "pause_added", "alive", "release"],
        "{line}"
    );
    assert_eq!(line["added"]["alias"], ADDED, "{line}");
    assert!(line["added"].get("start_paused").is_none(), "{line}");

    let config = config_of(&root);
    let added = member_of(&config, "/instances", "alias", ADDED).expect("the instance's entry");
    assert_eq!(
        members_of(added),
        COLD_ENTRY_MEMBERS[..6].to_vec(),
        "{added}"
    );
    assert!(starting_paused(&config).is_empty(), "{config}");

    let written = new_journals(&root, &before);
    let operation = written
        .iter()
        .find(|(_, journal)| journal["kind"] == "config_replace")
        .map(|(_, journal)| journal)
        .unwrap_or_else(|| panic!("a config_replace journal: {written:?}"));
    let recorded = &operation["inputs"]["instance_add"];
    assert_eq!(recorded["alias"], ADDED, "{recorded}");
    assert!(recorded.get("start_paused").is_none(), "{recorded}");
    assert_eq!(
        host.runtime.pause_of(ADDED).as_deref(),
        Some("instance.added")
    );
}

/// Fix 1: a cold add a crash of the fake host stopped: the root, its state root, the location read
/// before the crash, the configuration before the add and the interrupted operation's id.
struct Interrupted {
    root: PathBuf,
    state: PathBuf,
    located: actingcommand_lab_ops::locate::Location,
    old: Vec<u8>,
    operation_id: String,
}

/// Fix 1: a cold add on a root `name` below `base` stopped by the fake host's crash `crash`.
fn interrupted_cold_add(
    base: &Path,
    name: &str,
    host: &FakeHost,
    crash: impl Fn(&FakeHost),
) -> Interrupted {
    let root = installed(base, name, host);
    let located = location(&root);
    let old = fs::read(root.join("actingd.config.json")).expect("the configuration");
    let before = journals(&root);
    crash(host);
    let (stopped, exit) = add(host, &root);
    assert_ne!(exit, 0, "{name}: {stopped}");
    assert_eq!(
        top_code(&stopped),
        "install_operation_interrupted",
        "{name}: {stopped}"
    );
    let written = new_journals(&root, &before);
    assert_eq!(written.len(), 1, "{name}: {written:?}");
    assert!(root.join("install").join("current.json").exists(), "{name}");
    Interrupted {
        root,
        state: state_of(base, name),
        located,
        old,
        operation_id: written[0].0.clone(),
    }
}

// Fix 1 (the coordinator's answer 3): the resume of a cold commit applies the stopped rule first.
#[test]
fn s3_fix1_resume_of_a_cold_commit_applies_the_stopped_rule() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let Interrupted {
        root,
        state,
        located,
        operation_id,
        ..
    } = interrupted_cold_add(temp.path(), "resume", &host, |host| {
        host.crash_after.set(Some(1));
    });
    let halfway = tree(&root);
    let resume = json!({"op": operation_id});

    let owner = hold_owner_lock(&state);
    let (refused, exit) = rescue_at(&host, &located, "op resume", resume.clone());
    drop(owner);
    assert_refused(&refused, exit, "install_resume", "owner_lock_held");
    assert_eq!(tree(&root), halfway, "owner_lock_held");

    host.processes
        .replace(vec![actingd_image(&root, STARTING_PID)]);
    let (refused, exit) = rescue_at(&host, &located, "op resume", resume.clone());
    assert_refused(&refused, exit, "install_resume", "runtime_running");
    assert_eq!(
        top_value(&refused, "pid"),
        &json!(STARTING_PID),
        "{refused}"
    );
    assert_eq!(tree(&root), halfway, "runtime_running");
    host.processes.replace(Vec::new());

    // Stopped again: the swap finished, the operation done.
    let (resumed, exit) = rescue_at(&host, &located, "op resume", resume);
    assert_eq!(exit, 0, "{resumed}");
    assert_eq!(top_code(&resumed), "install_command_done", "{resumed}");
    assert_eq!(starting_paused(&config_of(&root)), [ADDED]);
    assert!(parts(&root).is_empty(), "{:?}", parts(&root));
    assert!(!root.join("install").join("current.json").exists());
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));
}

// Fix 1 (the coordinator's answer 3): the undo of a cold commit applies the stopped rule first, and
// puts back a swap stopped between its two renames, which left no live configuration.
#[test]
fn s3_fix1_undo_of_a_cold_commit_applies_the_stopped_rule_and_restores_a_half_done_swap() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let Interrupted {
        root,
        state,
        located,
        old,
        operation_id,
    } = interrupted_cold_add(temp.path(), "undo", &host, |host| {
        host.crash_between_renames.set(Some(2));
    });
    assert!(!root.join("actingd.config.json").exists());
    let halfway = tree(&root);
    let undo = json!({"op": operation_id});

    let owner = hold_owner_lock(&state);
    let (refused, exit) = rescue_at(&host, &located, "undo", undo.clone());
    drop(owner);
    assert_refused(&refused, exit, "install_undo", "owner_lock_held");
    assert_eq!(tree(&root), halfway, "owner_lock_held");

    host.processes
        .replace(vec![actingd_image(&root, STARTING_PID)]);
    let (refused, exit) = rescue_at(&host, &located, "undo", undo.clone());
    assert_refused(&refused, exit, "install_undo", "runtime_running");
    assert_eq!(tree(&root), halfway, "runtime_running");
    host.processes.replace(Vec::new());

    // Stopped: the kept file back in place byte for byte, the candidate set aside, no part.
    let (undone, exit) = rescue_at(&host, &located, "undo", undo);
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(top_code(&undone), "install_command_done", "{undone}");
    assert_eq!(top_value(&undone, "operation"), "install_undo", "{undone}");
    assert_eq!(
        fs::read(root.join("actingd.config.json")).expect("the configuration"),
        old
    );
    assert!(parts(&root).is_empty(), "{:?}", parts(&root));
    let aside = tree(&root)
        .into_iter()
        .filter(|(name, _)| !name.contains('/') && name.contains(".abandoned-"))
        .map(|(_, bytes)| bytes)
        .collect::<Vec<_>>();
    assert_eq!(aside.len(), 1, "one candidate set aside");
    let candidate: Value = serde_json::from_slice(&aside[0]).expect("the candidate's JSON");
    assert_eq!(starting_paused(&candidate), [ADDED], "{candidate}");
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));
}
