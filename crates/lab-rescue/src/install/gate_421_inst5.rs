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
