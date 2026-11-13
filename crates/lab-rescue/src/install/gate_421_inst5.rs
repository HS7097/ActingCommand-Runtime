// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #421 INST-5, first slice (register §1.300; contracts/install-cli.md, "Configuration,
// instances, packs and a fresh root" and "The Lab's rescue module"; coordinator work order
// `rd-1111/brief-inst5.md`): the rescue module's `instance add` on a fake root whose configuration
// has two emulator instances in both places and whose standard-pack root binds one neutral game
// on one neutral server.
// - `inst5_the_added_instance_is_registered_and_paused_after_the_restart`: the added instance is
//   in both places of the configuration, the status read after the restart lists it paused with
//   its own reason `instance.added`, and the instance the hold took is handed back; the steps and
//   the Runtime's calls come in the order hold, close, start, the added instance's pause, release;
// - `inst5_a_duplicate_alias_or_index_is_refused_and_nothing_changes`: an alias the configuration
//   has, and an emulator index an instance binds, are refused `instance_invalid` before anything
//   is written: the root byte for byte as it was, no journal, no call to the Runtime;
// - `inst5_a_candidate_check_config_refuses_changes_nothing`: a candidate the live build's
//   check-config refuses ends `config_refused_by_target` with check-config's own outcome as its
//   cause; the configuration, the journals and the Runtime as they were, the candidate kept;
// - `inst5_the_prev_file_is_kept_and_equals_the_old_file`: the replaced configuration is kept as
//   `actingd.config.json.prev-<ms>`, byte for byte the old file, and no part is left;
// - `inst5_a_pause_from_before_stays_with_its_reason_and_revision`: another party's pause is
//   there after the command with its reason and its revision, never resumed;
// - `inst5_the_journal_carries_the_initiator_and_what_was_added`: the operation's journal
//   (`config_replace`) and the release's carry the initiator, and the first records the instance
//   added (`inputs.instance_add`) with the id the configuration holds.
// Workflow #421 INST-5b (register §1.300, "INST-5b"; coordinator work order
// `rd-1111/brief-inst5b.md`): the hold of `instance add` also holds one global pause of its own
// across the restart.
// - `inst5b_the_global_pause_spans_the_restart`: the calls come in the order hold, global pause,
//   close, start, the added instance's pause, global resume, release; every status read between
//   the start and the added instance's pause shows the global pause in effect; the pause step
//   records it as its own and the release says it resumed it;
// - `inst5b_a_global_pause_from_before_is_neither_replaced_nor_lifted`: another party's global
//   pause is there after the command with its reason and revision, and no global pause or resume
//   was asked;
// - `inst5b_a_refused_pause_of_the_added_instance_keeps_the_global_pause_and_the_hold`: the
//   command ends `post_check_failed` with the global pause and the hold kept and `next` naming the
//   added instance's pause and the global resume; `release-hold` then resumes the global pause and
//   the hold's pause and nothing else;
// - `inst5b_config_replace_and_restore_send_no_global_pause`: neither asks for a global pause or
//   resume.
// Workflow #452 (register §1.300, "Fix #452"; coordinator work order
// `rd-1113/brief-452-inst5-appid.md`): `instance add` writes the entry's `application_id`, the one
// given or the one the game root's `applications.json` declares for the server. The fake game root
// of every test above declares [`DECLARED`] for [`SERVER`] (the one change to their fixture).
// - `inst5_452_a_given_application_id_is_written_over_the_declared_one`: with `application_id`
//   given (not the declared one) the entry is the six members in order with the given id, and the
//   answer's `added` and the journal's input record it with `application_id_from` `given`;
// - `inst5_452_without_the_option_the_declared_id_is_written`: without it the entry carries the
//   declared id, and both records say `declared`;
// - `inst5_452_without_a_declared_id_nothing_is_written`: without it and with no file, a file that
//   is no JSON, no member of the server, a member without `application_id`, or a blank one, each is
//   refused `instance_invalid` with `game`, `server` and the file's `path` (the read's failure as
//   the cause when the read failed, none otherwise): the root byte for byte as it was (nothing
//   staged), no journal, no call to the Runtime;
// - `inst5_452_a_malformed_application_id_is_refused_by_the_assembly_rule`: an empty, a blank, a
//   257-byte one and one with a control character are `actingctl_usage_invalid` `unusable` (a
//   number `wrong_type`) naming `--application-id`, nothing written and nothing asked; a 256-byte
//   one with spaces and capitals, which the assembly takes, is written as given.
// The machine is the fake host of `fixture.rs` with its fake Runtime, fake watchdog and fake
// clock: no device, no emulator, no sleep; every root is a temporary folder with neutral files.

use super::fixture::{BuildSpec, C1, FakeHost, code, live_root, run, tree, word, write_json};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs;
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
/// Workflow #452: the application id the game root declares for [`SERVER`] (`applications.json`).
const DECLARED: &str = "neutral.application";
/// Workflow #452: an application id a caller gives, not the declared one.
const GIVEN: &str = "neutral.application.given";

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
/// holds the game root of [`GAME`] with one pack of [`SERVER`] (written after the adopt, so the
/// pack register stays empty).
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
    // Workflow #452: the game root's applications table, which declares the application of
    // [`SERVER`], as a game root a bundle places holds it; without `application_id` the command
    // takes this one.
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

/// The rescue module's location of `root`: its configuration and the state root it names.
fn location(root: &Path) -> actingcommand_lab_ops::locate::Location {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    rescue::location_of(root, &config, &state_root)
}

/// `instance add <alias>` of [`GAME`] on [`SERVER`] at the emulator index `index`, as [`WORD`];
/// its one line and exit code.
fn add(host: &FakeHost, root: &Path, alias: &str, index: u64) -> (Value, u8) {
    let location = location(root);
    let context = Context {
        host,
        location: &location,
        initiator: WORD,
    };
    let params = match json!({
        "alias": alias,
        "game": GAME,
        "server": SERVER,
        "mumu_index": index,
        "touch_backend": "neutral-touch",
        "capture_backend": "neutral-capture",
    }) {
        Value::Object(params) => params,
        _ => Map::new(),
    };
    match rescue::answer_with(&context, "instance add", &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("instance add: a line, not {other:?}"),
    }
}

fn top_code(line: &Value) -> &str {
    line["outcome"]["code"].as_str().unwrap_or_default()
}

fn top_value<'a>(line: &'a Value, key: &str) -> &'a Value {
    &line["outcome"]["values"][key]
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

fn calls(host: &FakeHost) -> Vec<String> {
    host.runtime.calls.borrow().clone()
}

/// The index of the first call that starts with `prefix`.
fn first(calls: &[String], prefix: &str) -> usize {
    calls
        .iter()
        .position(|call| call.starts_with(prefix))
        .unwrap_or_else(|| panic!("no call {prefix}: {calls:?}"))
}

/// The pause `alias` shows on the fake Runtime: its reason and its revision.
fn pause_seen(host: &FakeHost, alias: &str) -> Option<(String, u64)> {
    host.runtime
        .instances
        .borrow()
        .iter()
        .find(|instance| instance.alias == alias)
        .and_then(|instance| instance.pause.as_ref())
        .map(|pause| (pause.reason.as_str().to_owned(), pause.revision))
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

/// A root with the Runtime running `neutral.a` and `neutral.b`, whose next start registers the
/// added instance as a start with the new configuration does.
fn running(base: &Path, name: &str, host: &FakeHost) -> PathBuf {
    let root = installed(base, name, host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    host.runtime
        .registers_at_start
        .borrow_mut()
        .push(ADDED.to_owned());
    root
}

// The register's first named test (work order, "Tests", first item; build points 1, 3 and 4).
#[test]
fn inst5_the_added_instance_is_registered_and_paused_after_the_restart() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "added", &host);

    let (line, exit) = add(&host, &root, ADDED, 2);
    assert_eq!(exit, 0, "{line}");
    assert_eq!(top_code(&line), "install_command_done", "{line}");
    assert_eq!(top_value(&line, "operation"), "install_instance_add");
    assert_eq!(top_value(&line, "alias"), ADDED);
    assert_eq!(top_value(&line, "game"), GAME);
    assert_eq!(line["runtime_running"], json!(true), "{line}");

    // The status after the restart: the added instance is registered and paused with its own
    // reason; the instance the hold took is handed back.
    assert!(
        host.runtime
            .instances
            .borrow()
            .iter()
            .any(|instance| instance.alias == ADDED),
        "the added instance is registered"
    );
    assert_eq!(
        host.runtime.pause_of(ADDED).as_deref(),
        Some("instance.added")
    );
    assert_eq!(host.runtime.pause_of("neutral.a"), None);
    assert!(host.runtime.running.get());

    // The steps, and what the Runtime was asked, in order.
    let names = line["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .filter_map(|step| step["step"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["check_config", "swap", "pause_added", "alive", "release"],
        "{line}"
    );
    let calls = calls(&host);
    let hold = first(&calls, "pause neutral.a lab-instance-");
    let close = first(&calls, "shutdown");
    let start = first(&calls, "spawn ");
    let paused = first(&calls, "pause neutral.added instance.added");
    let release = first(&calls, "resume neutral.a");
    assert!(
        hold < close && close < start && start < paused && paused < release,
        "{calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("resume neutral.added")),
        "{calls:?}"
    );

    // Both places of the configuration name the instance; its id is a new one.
    let config = config_of(&root);
    let added = member_of(&config, "/instances", "alias", ADDED).expect("the instance's entry");
    assert_eq!(added["instance_index"], json!(2));
    assert_eq!(added["touch_backend"], "neutral-touch");
    assert_eq!(added["capture_backend"], "neutral-capture");
    let id = added["instance_id"].as_str().expect("an instance id");
    assert!(id.starts_with("instance_") && id.len() == 41, "{id}");
    assert!(id != ID_A && id != ID_B, "{id}");
    let row = member_of(&config, "/policy/facts/instances", "instance_id", ADDED)
        .expect("the instance's policy row");
    assert_eq!(row["game_id"], GAME);
    assert_eq!(row["server_id"], SERVER);
    assert_eq!(row["host_id"], HOST_ID);
    assert_eq!(row["available"], json!(true));
    assert_eq!(
        row["capability_operation_ids"],
        json!(["operation.neutral"])
    );
    assert_eq!(line["added"]["instance_id"], id, "{line}");
}

// The register's second named test (work order, "Tests", second item; build point 2).
#[test]
fn inst5_a_duplicate_alias_or_index_is_refused_and_nothing_changes() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "duplicate", &host);
    let before = tree(&root);

    // An alias the configuration has.
    let (refused, exit) = add(&host, &root, "neutral.a", 2);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(top_value(&refused, "operation"), "install_instance_add");
    assert_eq!(top_value(&refused, "install_refusal"), "instance_invalid");
    assert_eq!(top_value(&refused, "alias"), "neutral.a");

    // An emulator index an instance binds.
    let (refused, exit) = add(&host, &root, ADDED, 1);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_value(&refused, "install_refusal"), "instance_invalid");
    assert_eq!(top_value(&refused, "instance_index"), &json!(1));
    assert_eq!(top_value(&refused, "aliases"), &json!(["neutral.b"]));

    // Nothing changed: the root byte for byte, no journal, no restart, nothing asked.
    assert_eq!(tree(&root), before);
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));
    assert!(host.runtime.running.get());
}

// The register's third named test (work order, "Tests", third item; build point 2).
#[test]
fn inst5_a_candidate_check_config_refuses_changes_nothing() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "refused", &host);
    host.programs
        .refuse_check_config
        .borrow_mut()
        .push("live".to_owned());
    let config = fs::read(root.join("actingd.config.json")).expect("the configuration");
    let before = tree(&root);

    let (refused, exit) = add(&host, &root, ADDED, 2);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(top_value(&refused, "operation"), "install_instance_add");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "config_refused_by_target"
    );
    // check-config's own refusal, relayed whole as the cause.
    assert!(
        refused["outcome"]
            .to_string()
            .contains("config_decode_failed"),
        "{refused}"
    );

    // The configuration, the journals and the Runtime as they were; the one new file is the
    // candidate, kept beside the live file.
    assert_eq!(
        fs::read(root.join("actingd.config.json")).expect("the configuration"),
        config
    );
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));
    assert!(host.runtime.running.get());
    let after = tree(&root);
    let added = after
        .keys()
        .filter(|name| !before.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 1, "{added:?}");
    assert!(
        added[0].starts_with("actingd.config.json.new-") && added[0].ends_with(".part"),
        "{added:?}"
    );
    for (name, bytes) in &before {
        assert_eq!(after.get(name), Some(bytes), "{name}");
    }
}

// The register's fourth named test (work order, "Tests", fourth item; build point 3).
#[test]
fn inst5_the_prev_file_is_kept_and_equals_the_old_file() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "prev", &host);
    let old = fs::read(root.join("actingd.config.json")).expect("the configuration");

    let (line, exit) = add(&host, &root, ADDED, 2);
    assert_eq!(exit, 0, "{line}");
    let backup = PathBuf::from(line["backup"].as_str().expect("the backup"));
    let name = backup
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert!(name.starts_with("actingd.config.json.prev-"), "{name}");
    assert_eq!(fs::read(&backup).expect("the kept file"), old);
    assert_ne!(
        fs::read(root.join("actingd.config.json")).expect("the configuration"),
        old
    );
    // The candidate was the swap's own part: none is left beside the live file.
    let parts = tree(&root)
        .into_keys()
        .filter(|name| !name.contains('/') && name.ends_with(".part"))
        .collect::<Vec<_>>();
    assert!(parts.is_empty(), "{parts:?}");
}

// The register's fifth named test (work order, "Tests", fifth item; build point 3).
#[test]
fn inst5_a_pause_from_before_stays_with_its_reason_and_revision() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "kept", &host);
    host.runtime.pause_as("neutral.b", "other-party");
    let other = pause_seen(&host, "neutral.b");
    assert!(other.is_some());

    let (line, exit) = add(&host, &root, ADDED, 2);
    assert_eq!(exit, 0, "{line}");
    assert_eq!(pause_seen(&host, "neutral.b"), other);
    let calls = calls(&host);
    assert!(
        !calls.iter().any(|call| call.starts_with("pause neutral.b")
            || call.starts_with("resume neutral.b")),
        "{calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("resume "))
            .collect::<Vec<_>>(),
        ["resume neutral.a"]
    );
}

// The register's sixth named test (work order, "Tests", sixth item; build point 5).
#[test]
fn inst5_the_journal_carries_the_initiator_and_what_was_added() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "journal", &host);
    let before = journals(&root);

    let (line, exit) = add(&host, &root, ADDED, 2);
    assert_eq!(exit, 0, "{line}");
    let written = new_journals(&root, &before);
    let kinds = written
        .iter()
        .map(|(_, journal)| journal["kind"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["config_replace", "release_hold"], "{line}");
    for (id, journal) in &written {
        assert_eq!(journal["initiator"], WORD.as_str(), "{id}");
    }
    let operation = &written[0].1;
    assert_eq!(
        top_value(&line, "install_operation_id"),
        written[0].0.as_str()
    );
    let added = &operation["inputs"]["instance_add"];
    assert_eq!(added["alias"], ADDED, "{operation}");
    assert_eq!(added["game"], GAME);
    assert_eq!(added["server"], SERVER);
    assert_eq!(added["instance_index"], json!(2));
    assert_eq!(added["touch_backend"], "neutral-touch");
    assert_eq!(added["capture_backend"], "neutral-capture");
    assert_eq!(added["pause_reason"], "instance.added");
    let config = config_of(&root);
    let entry = member_of(&config, "/instances", "alias", ADDED).expect("the instance's entry");
    assert_eq!(added["instance_id"], entry["instance_id"]);
}

/// The rescue row `name` with `params` on `root`, as [`WORD`]; its one line and exit code.
fn rescue_line(host: &FakeHost, root: &Path, name: &str, params: Value) -> (Value, u8) {
    let location = location(root);
    let context = Context {
        host,
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

/// The index of the last call that starts with `prefix`.
fn last(calls: &[String], prefix: &str) -> usize {
    calls
        .iter()
        .rposition(|call| call.starts_with(prefix))
        .unwrap_or_else(|| panic!("no call {prefix}: {calls:?}"))
}

/// The hold's reason, as the hold asked for the pause of `alias`.
fn hold_reason_of(calls: &[String], alias: &str) -> String {
    let prefix = format!("pause {alias} ");
    calls
        .iter()
        .filter_map(|call| call.strip_prefix(prefix.as_str()))
        .find(|reason| reason.starts_with("lab-instance-"))
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("no hold of {alias}: {calls:?}"))
}

/// The global pause the fake Runtime shows: its reason and its revision.
fn global_seen(host: &FakeHost) -> Option<(String, u64)> {
    host.runtime
        .global_pause
        .borrow()
        .as_ref()
        .map(|pause| (pause.reason.as_str().to_owned(), pause.revision))
}

/// The step of `kind` of a journal.
fn step_of<'a>(journal: &'a Value, kind: &str) -> &'a Value {
    journal["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["kind"] == kind))
        .unwrap_or_else(|| panic!("no step {kind}: {journal}"))
}

// INST-5b, the work order's first named test (build points 1, 2 and 4).
#[test]
fn inst5b_the_global_pause_spans_the_restart() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "spans", &host);
    let before = journals(&root);

    let (line, exit) = add(&host, &root, ADDED, 2);
    assert_eq!(exit, 0, "{line}");
    assert_eq!(top_code(&line), "install_command_done", "{line}");

    // The order: hold, global pause, close, start, the added instance's pause, global resume,
    // release.
    let calls = calls(&host);
    let reason = hold_reason_of(&calls, "neutral.a");
    let held = last(&calls, &format!("pause neutral.b {reason}"))
        .max(last(&calls, &format!("pause neutral.a {reason}")));
    let set = first(&calls, &format!("global pause {reason}"));
    let close = first(&calls, "shutdown");
    let start = first(&calls, "spawn ");
    let paused = first(&calls, "pause neutral.added instance.added");
    let lifted = first(&calls, "global resume");
    let released = first(&calls, "resume neutral.a").min(first(&calls, "resume neutral.b"));
    assert!(
        held < set
            && set < close
            && close < start
            && start < paused
            && paused < lifted
            && lifted < released,
        "{calls:?}"
    );
    let global_calls = calls
        .iter()
        .filter(|call| call.starts_with("global "))
        .count();
    assert_eq!(global_calls, 2, "{calls:?}");

    // Between the start and the added instance's pause every status read shows the global pause.
    let window = host
        .runtime
        .status_reads
        .borrow()
        .iter()
        .filter(|(at, _)| *at > start && *at <= paused)
        .map(|(_, shown)| shown.clone())
        .collect::<Vec<_>>();
    assert!(!window.is_empty(), "{calls:?}");
    assert!(
        window
            .iter()
            .all(|shown| shown.as_deref() == Some(reason.as_str())),
        "{window:?}"
    );

    // After it: the global pause lifted, the added instance paused with its own reason.
    assert_eq!(global_seen(&host), None);
    assert_eq!(
        host.runtime.pause_of(ADDED).as_deref(),
        Some("instance.added")
    );
    assert_eq!(host.runtime.pause_of("neutral.a"), None);

    // The journal: the pause step asked for the global pause and records it as its own; the
    // release says it resumed it.
    let written = new_journals(&root, &before);
    assert_eq!(written.len(), 2, "{written:?}");
    let pause = step_of(&written[0].1, "pause");
    assert_eq!(pause["intent"]["global"], json!(true), "{pause}");
    let own = &pause["evidence"]["own_pauses"];
    assert_eq!(own["reason"], reason.as_str(), "{pause}");
    assert!(own["global"].is_u64(), "{pause}");
    let release = step_of(&written[1].1, "release_hold");
    assert_eq!(
        release["evidence"]["global_resumed"],
        json!(true),
        "{release}"
    );
}

// INST-5b, the work order's second named test (build point 1).
#[test]
fn inst5b_a_global_pause_from_before_is_neither_replaced_nor_lifted() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "foreign", &host);
    host.runtime.pause_global_as("other-party");
    let other = global_seen(&host);
    assert!(other.is_some());

    let (line, exit) = add(&host, &root, ADDED, 2);
    assert_eq!(exit, 0, "{line}");
    assert_eq!(top_code(&line), "install_command_done", "{line}");
    assert_eq!(global_seen(&host), other);
    let calls = calls(&host);
    assert!(
        !calls.iter().any(|call| call.starts_with("global ")),
        "{calls:?}"
    );
    assert_eq!(
        host.runtime.pause_of(ADDED).as_deref(),
        Some("instance.added")
    );
}

// INST-5b, the work order's third named test (build point 3).
#[test]
fn inst5b_a_refused_pause_of_the_added_instance_keeps_the_global_pause_and_the_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "refused-added", &host);
    host.runtime.pause_as("neutral.b", "other-party");
    let other = pause_seen(&host, "neutral.b");
    host.runtime
        .refuse_pause
        .borrow_mut()
        .push(ADDED.to_owned());

    let (line, exit) = add(&host, &root, ADDED, 2);
    assert_eq!(exit, 1, "{line}");
    assert_eq!(top_code(&line), "install_command_refused", "{line}");
    assert_eq!(top_value(&line, "install_refusal"), "post_check_failed");
    assert_eq!(top_value(&line, "alias"), ADDED);
    let reason = line["hold"]["reason"]
        .as_str()
        .expect("the hold's reason")
        .to_owned();
    assert_eq!(line["hold"]["kept"], json!(true), "{line}");

    // The global pause and the hold stay; the added instance is not paused.
    assert_eq!(
        global_seen(&host).map(|(found, _)| found),
        Some(reason.clone())
    );
    assert_eq!(host.runtime.pause_of("neutral.a"), Some(reason.clone()));
    assert_eq!(host.runtime.pause_of(ADDED), None);

    // `next` names both ways out: the added instance's pause and the global pause's resume, the
    // core commands as actingctl runs them, beside the module's own release.
    let next = &line["next"];
    assert_eq!(next["pause_added"][0], "actingctl", "{line}");
    assert_eq!(next["pause_added"][1], "pause", "{line}");
    assert_eq!(next["pause_added"][5], ADDED, "{line}");
    assert_eq!(next["pause_added"][7], "instance.added", "{line}");
    assert_eq!(
        next["actingctl_resume_global"],
        json!([
            "actingctl",
            "resume",
            "--state-root",
            next["pause_added"][3]
        ]),
        "{line}"
    );
    assert_eq!(
        next["release_hold"],
        json!(["release-hold", reason]),
        "{line}"
    );

    // `release-hold` hands back the global pause and the hold's pause, and nothing else.
    let asked = calls(&host).len();
    let (released, exit) = rescue_line(&host, &root, "release-hold", json!({"reason": reason}));
    assert_eq!(exit, 0, "{released}");
    assert_eq!(top_code(&released), "install_command_done", "{released}");
    let after = calls(&host)[asked..].to_vec();
    assert_eq!(after, ["global resume", "resume neutral.a"], "{released}");
    assert_eq!(global_seen(&host), None);
    assert_eq!(host.runtime.pause_of("neutral.a"), None);
    assert_eq!(pause_seen(&host, "neutral.b"), other);
    assert_eq!(host.runtime.pause_of(ADDED), None);
}

// INST-5b, the work order's fourth named test (build point 1).
#[test]
fn inst5b_config_replace_and_restore_send_no_global_pause() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = installed(temp.path(), "replace", &host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    let candidate = temp.path().join("candidate.json");
    let document = config_of(&root);
    fs::write(&candidate, serde_json::to_vec(&document).expect("JSON")).expect("the candidate");

    let (replaced, exit) = rescue_line(
        &host,
        &root,
        "config replace",
        json!({"file": word(&candidate)}),
    );
    assert_eq!(exit, 0, "{replaced}");
    assert_eq!(top_value(&replaced, "operation"), "install_config_replace");
    let backup = replaced["backup"].as_str().expect("the backup").to_owned();
    let (restored, exit) = rescue_line(&host, &root, "config restore", json!({"backup": backup}));
    assert_eq!(exit, 0, "{restored}");
    assert_eq!(top_value(&restored, "operation"), "install_config_restore");

    // Each held, closed, started and released, and neither asked for a global pause.
    let calls = calls(&host);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "shutdown")
            .count(),
        2,
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|call| call.starts_with("global ")),
        "{calls:?}"
    );
    assert_eq!(global_seen(&host), None);
}

/// Workflow #452: the members of an added entry, in the order the configuration file holds them:
/// the application id after the instance id.
const ENTRY_MEMBERS: [&str; 6] = [
    "alias",
    "instance_id",
    "application_id",
    "instance_index",
    "touch_backend",
    "capture_backend",
];

/// Workflow #452: the parameters of `instance add <alias>` of [`GAME`] on [`SERVER`] at the
/// emulator index `index`, with `application_id` when one is given.
fn add_params(alias: &str, index: u64, application_id: Option<Value>) -> Value {
    let mut params = json!({
        "alias": alias,
        "game": GAME,
        "server": SERVER,
        "mumu_index": index,
        "touch_backend": "neutral-touch",
        "capture_backend": "neutral-capture",
    });
    if let Some(application_id) = application_id {
        params["application_id"] = application_id;
    }
    params
}

/// The names of an entry's members, in the file's order.
fn members_of(entry: &Value) -> Vec<&str> {
    entry
        .as_object()
        .map(|members| members.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

// Workflow #452, the work order's "Tests": the parameter given (build points 1, 4 and 5).
#[test]
fn inst5_452_a_given_application_id_is_written_over_the_declared_one() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "given", &host);
    let before = journals(&root);

    let (line, exit) = rescue_line(
        &host,
        &root,
        "instance add",
        add_params(ADDED, 2, Some(json!(GIVEN))),
    );
    assert_eq!(exit, 0, "{line}");
    assert_eq!(top_code(&line), "install_command_done", "{line}");

    // The entry: its six members in order, the given id, though the game root declares another.
    let config = config_of(&root);
    let entry = member_of(&config, "/instances", "alias", ADDED).expect("the instance's entry");
    assert_eq!(members_of(entry), ENTRY_MEMBERS, "{entry}");
    assert_eq!(entry["application_id"], GIVEN, "{entry}");

    // The answer and the journal's input record it and where it came from.
    assert_eq!(line["added"]["application_id"], GIVEN, "{line}");
    assert_eq!(line["added"]["application_id_from"], "given", "{line}");
    let written = new_journals(&root, &before);
    let added = &written[0].1["inputs"]["instance_add"];
    assert_eq!(added["application_id"], GIVEN, "{added}");
    assert_eq!(added["application_id_from"], "given", "{added}");
}

// Workflow #452, the work order's "Tests": the parameter absent, the game root declaring an id
// (build points 2 and 5).
#[test]
fn inst5_452_without_the_option_the_declared_id_is_written() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "declared", &host);
    let before = journals(&root);

    let (line, exit) = rescue_line(&host, &root, "instance add", add_params(ADDED, 2, None));
    assert_eq!(exit, 0, "{line}");
    assert_eq!(top_code(&line), "install_command_done", "{line}");

    let config = config_of(&root);
    let entry = member_of(&config, "/instances", "alias", ADDED).expect("the instance's entry");
    assert_eq!(members_of(entry), ENTRY_MEMBERS, "{entry}");
    assert_eq!(entry["application_id"], DECLARED, "{entry}");

    assert_eq!(line["added"]["application_id"], DECLARED, "{line}");
    assert_eq!(line["added"]["application_id_from"], "declared", "{line}");
    let written = new_journals(&root, &before);
    let added = &written[0].1["inputs"]["instance_add"];
    assert_eq!(added["application_id"], DECLARED, "{added}");
    assert_eq!(added["application_id_from"], "declared", "{added}");
}

// Workflow #452, the work order's "Tests": the parameter absent and no id declared (build point
// 3).
#[test]
fn inst5_452_without_a_declared_id_nothing_is_written() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "undeclared", &host);
    let table = temp
        .path()
        .join("undeclared-packs")
        .join(GAME)
        .join("applications.json");
    let before = tree(&root);
    let journals_before = journals(&root);
    let declaring = |servers: Value| {
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": servers,
        }))
        .expect("JSON")
    };

    // Each case: what the file holds (none: the file is moved aside) and the code of the read's
    // failure, when the read failed.
    let cases = [
        ("no file", None, Some("foreign_os_error")),
        (
            "no JSON",
            Some(b"{ no JSON".to_vec()),
            Some("foreign_data_error"),
        ),
        (
            "no member of the server",
            Some(declaring(
                json!({"neutral-other": {"application_id": DECLARED}}),
            )),
            None,
        ),
        (
            "no application_id",
            Some(declaring(
                json!({SERVER: {"default_package_id": "neutral.pack"}}),
            )),
            None,
        ),
        (
            "a blank one",
            Some(declaring(json!({SERVER: {"application_id": "   "}}))),
            None,
        ),
    ];
    for (name, holds, cause) in cases {
        match holds {
            Some(bytes) => fs::write(&table, bytes).expect("the applications table"),
            None => fs::rename(&table, table.with_extension("json.aside"))
                .expect("the applications table moved aside"),
        }
        let (refused, exit) = rescue_line(&host, &root, "instance add", add_params(ADDED, 2, None));
        assert_eq!(exit, 1, "{name}: {refused}");
        assert_eq!(
            top_code(&refused),
            "install_command_refused",
            "{name}: {refused}"
        );
        assert_eq!(top_value(&refused, "operation"), "install_instance_add");
        assert_eq!(
            top_value(&refused, "install_refusal"),
            "instance_invalid",
            "{name}: {refused}"
        );
        assert_eq!(top_value(&refused, "game"), GAME, "{name}: {refused}");
        assert_eq!(top_value(&refused, "server"), SERVER, "{name}: {refused}");
        assert!(
            top_value(&refused, "path")
                .as_str()
                .is_some_and(|path| path.ends_with("applications.json")),
            "{name}: {refused}"
        );
        let causes = &refused["outcome"]["causes"];
        match cause {
            Some(code) => assert_eq!(causes[0]["code"], code, "{name}: {refused}"),
            None => assert!(
                causes.as_array().is_none_or(Vec::is_empty),
                "{name}: {refused}"
            ),
        }
        // Nothing staged, no journal, nothing asked.
        assert_eq!(tree(&root), before, "{name}");
        assert!(new_journals(&root, &journals_before).is_empty(), "{name}");
        assert!(calls(&host).is_empty(), "{name}: {:?}", calls(&host));
    }
}

// Workflow #452, the work order's "Tests": a malformed parameter, judged by the rule of the
// configuration's assembly and by no stricter one (build point 1).
#[test]
fn inst5_452_a_malformed_application_id_is_refused_by_the_assembly_rule() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "malformed", &host);
    let before = tree(&root);

    let cases = [
        ("empty", json!(""), "unusable"),
        ("blank", json!("   "), "unusable"),
        ("257 bytes", json!("a".repeat(257)), "unusable"),
        (
            "a control character",
            json!("neutral\u{7}application"),
            "unusable",
        ),
        ("a number", json!(7), "wrong_type"),
    ];
    for (name, given, problem) in cases {
        let (refused, exit) = rescue_line(
            &host,
            &root,
            "instance add",
            add_params(ADDED, 2, Some(given)),
        );
        assert_eq!(exit, 1, "{name}: {refused}");
        assert_eq!(
            top_code(&refused),
            "actingctl_usage_invalid",
            "{name}: {refused}"
        );
        assert_eq!(
            top_value(&refused, "argument"),
            "--application-id",
            "{name}: {refused}"
        );
        assert_eq!(
            top_value(&refused, "argument_problem"),
            problem,
            "{name}: {refused}"
        );
    }
    assert_eq!(tree(&root), before);
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));

    // 256 bytes with spaces and capitals: the assembly takes it, and so does the command.
    let head = " Neutral Application ";
    let longest = format!("{head}{}", "x".repeat(256 - head.len()));
    let (line, exit) = rescue_line(
        &host,
        &root,
        "instance add",
        add_params(ADDED, 2, Some(json!(longest))),
    );
    assert_eq!(exit, 0, "{line}");
    let config = config_of(&root);
    let entry = member_of(&config, "/instances", "alias", ADDED).expect("the instance's entry");
    assert_eq!(entry["application_id"], longest.as_str(), "{entry}");
    assert_eq!(line["added"]["application_id_from"], "given", "{line}");
}
