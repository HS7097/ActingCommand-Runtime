// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #454 (register §1.341; contracts/install-cli.md, "The post-start checks"; coordinator
// work order `rd-1113/brief-454-config-replace-ack.md`): a post-start check that fails ends the
// operation `post_check_failed` and closes its step `done` with what it read, the evidence a passed
// check records (`commit`, `checks`, `failed`), in the write that ends the operation. Before the
// fix the step stayed `started` with no evidence, the rows only in the answer.
// - `postcheck_454_config_replace_failed_check_closes_its_step_with_its_rows`: the rescue module's
//   `config replace` whose new start reports a seeded tagged state: the answer is
//   `post_check_failed` (`count` 1, `failed_checks` `no_seeding`), and the operation's journal
//   ended with that refusal and its `post_check` step `done`, `failed` `no_seeding`, exactly the
//   row `no_seeding` failed with the tagged state it read, the rows those the answer listed;
// - `postcheck_454_instance_add_failed_check_closes_its_step_with_its_rows`: the same for
//   `instance add`, which commits through the same path;
// - `postcheck_454_config_replace_passed_check_keeps_its_record`: a `config replace` whose checks
//   pass keeps the record it wrote before: the step `done` with the same three members, `failed`
//   empty, no row failed.
// The machine is the fake host of `fixture.rs` with its fake Runtime, fake watchdog and fake
// clock: no device, no emulator, no sleep; every root is a temporary folder with neutral files.

use super::fixture::{BuildSpec, C1, FakeHost, code, live_root, run, word, write_json};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The initiator the tests run the module as: a Lab process the UI started.
const WORD: Initiator = Initiator::LabRescue;
const GAME: &str = "neutral-game";
const SERVER: &str = "neutral-server";
const HOST_ID: &str = "neutral.host";
/// The alias `instance add` adds.
const ADDED: &str = "neutral.added";

/// The rescue module's location of `root`: its configuration and the state root it names.
fn location(root: &Path) -> actingcommand_lab_ops::locate::Location {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    rescue::location_of(root, &config, &state_root)
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

fn top_value<'a>(line: &'a Value, key: &str) -> &'a Value {
    &line["outcome"]["values"][key]
}

/// The journal of `root` of kind `config_replace` (the one operation `config replace` and
/// `instance add` journal).
fn config_replace_journal(root: &Path) -> Value {
    let directory = root.join("install").join("operations");
    let found = fs::read_dir(&directory)
        .expect("the journals")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_suffix(".json")
                .is_some_and(|id| id.starts_with("op-"))
        })
        .map(|entry| {
            serde_json::from_slice::<Value>(&fs::read(entry.path()).expect("a journal"))
                .expect("journal JSON")
        })
        .filter(|journal| journal["kind"] == "config_replace")
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "one config_replace journal: {found:?}");
    found[0].clone()
}

/// The `post_check` step of `journal`.
fn post_check_step(journal: &Value) -> Value {
    journal["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["kind"] == "post_check"))
        .cloned()
        .unwrap_or_else(|| panic!("a post_check step: {journal}"))
}

/// The members of a step's evidence, sorted.
fn members_of(evidence: &Value) -> Vec<String> {
    let mut members = evidence
        .as_object()
        .map(|members| members.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    members.sort();
    members
}

/// The rows of a post-start check's evidence whose result is `fail`.
fn failing_rows(evidence: &Value) -> Vec<Value> {
    evidence["checks"]
        .as_array()
        .expect("the rows")
        .iter()
        .filter(|row| row["result"] == "fail")
        .cloned()
        .collect()
}

/// The next start of the root's build (C1) reports its tagged state seeded again: on a root that
/// ran before, the fail-class check `no_seeding` fails, and no other check.
fn seeded_on_start(host: &FakeHost) {
    host.runtime
        .report_patch
        .borrow_mut()
        .insert(C1.to_owned(), json!({"tagged_state": "seed"}));
}

/// A root whose live build is C1, adopted, with its Runtime running `neutral.a`.
fn running_root(base: &Path, name: &str, host: &FakeHost) -> PathBuf {
    let root = live_root(base, name, &BuildSpec::new(C1));
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    host.runtime.boot(&["neutral.a"]);
    root
}

/// A candidate configuration: the live document written compactly, other bytes than the live
/// file's, which check-config passes.
fn candidate(base: &Path, root: &Path) -> PathBuf {
    let document: Value = serde_json::from_slice(
        &fs::read(root.join("actingd.config.json")).expect("the configuration"),
    )
    .expect("JSON");
    let path = base.join("candidate.json");
    fs::write(&path, serde_json::to_vec(&document).expect("JSON")).expect("the candidate");
    path
}

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

/// A root as `instance add` takes one (the fixture of `gate_421_inst5.rs`): the live build C1,
/// adopted, the emulator instances `neutral.a` (index 0) and `neutral.b` (index 1) in both places
/// of the configuration, the game root of [`GAME`] with one pack of [`SERVER`] and the application
/// it declares; its Runtime running both, and its next start registering [`ADDED`].
fn instance_root(base: &Path, name: &str, host: &FakeHost) -> PathBuf {
    let root = live_root(base, name, &BuildSpec::new(C1));
    let packs = base.join(format!("{name}-packs"));
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration")).expect("JSON");
    config["standard_packs"] = json!({"root": packs});
    config["instances"] = json!([
        entry("neutral.a", "instance_0000000000000000000000000000000a", 0),
        entry("neutral.b", "instance_0000000000000000000000000000000b", 1),
    ]);
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
            "servers": {SERVER: {"application_id": "neutral.application"}},
        }),
    );
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    host.runtime
        .registers_at_start
        .borrow_mut()
        .push(ADDED.to_owned());
    root
}

/// The answer of a commit whose check `no_seeding` failed, and the journal it ended: the refusal
/// `post_check_failed` with `count` 1 and `failed_checks` `no_seeding`; the journal's result that
/// refusal, the operation ended, and its `post_check` step `done` with the members a passed check
/// records, `failed` `no_seeding`, exactly that row failed with the tagged state it read, and the
/// rows those the answer listed.
fn assert_closed_with_the_failed_row(line: &Value, exit: u8, journal: &Value) {
    assert_eq!(exit, 1, "{line}");
    assert_eq!(
        top_value(line, "install_refusal"),
        "post_check_failed",
        "{line}"
    );
    assert_eq!(top_value(line, "count"), &json!(1), "{line}");
    assert_eq!(line["failed_checks"], json!(["no_seeding"]), "{line}");

    assert_eq!(
        journal["result"]["values"]["install_refusal"], "post_check_failed",
        "{journal}"
    );
    assert!(journal["finished_at_unix_ms"].is_u64(), "{journal}");
    let step = post_check_step(journal);
    assert_eq!(step["state"], "done", "{step}");
    let evidence = &step["evidence"];
    assert_eq!(
        members_of(evidence),
        ["checks", "commit", "failed"],
        "{step}"
    );
    assert_eq!(evidence["commit"], C1, "{step}");
    assert_eq!(evidence["failed"], json!(["no_seeding"]), "{step}");
    assert_eq!(
        failing_rows(evidence),
        [json!({
            "id": "no_seeding",
            "result": "fail",
            "fail_class": true,
            "values": {"tagged_state": "seed"},
        })],
        "{step}"
    );
    assert_eq!(
        evidence["checks"], line["post_check"],
        "the record holds the rows the answer listed"
    );
}

// The work order's first test, its record half: `config replace` whose new start fails a
// post-start check ends `post_check_failed`, and its operation record's step is closed with the
// failing row and what it read.
#[test]
fn postcheck_454_config_replace_failed_check_closes_its_step_with_its_rows() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running_root(temp.path(), "replace", &host);
    seeded_on_start(&host);
    let candidate = candidate(temp.path(), &root);

    let (line, exit) = rescue_line(
        &host,
        &root,
        "config replace",
        json!({"file": word(&candidate)}),
    );
    assert_eq!(
        top_value(&line, "operation"),
        "install_config_replace",
        "{line}"
    );
    assert_closed_with_the_failed_row(&line, exit, &config_replace_journal(&root));
}

// The same for `instance add`, which commits through `config replace`'s path.
#[test]
fn postcheck_454_instance_add_failed_check_closes_its_step_with_its_rows() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = instance_root(temp.path(), "added", &host);
    seeded_on_start(&host);

    let (line, exit) = rescue_line(
        &host,
        &root,
        "instance add",
        json!({
            "alias": ADDED,
            "game": GAME,
            "server": SERVER,
            "mumu_index": 2,
            "touch_backend": "neutral-touch",
            "capture_backend": "neutral-capture",
        }),
    );
    let journal = config_replace_journal(&root);
    assert_eq!(
        journal["inputs"]["instance_add"]["alias"], ADDED,
        "{journal}"
    );
    assert_closed_with_the_failed_row(&line, exit, &journal);
}

// A passed check keeps the record it writes today: the step `done`, the same three members,
// `failed` empty and no row failed.
#[test]
fn postcheck_454_config_replace_passed_check_keeps_its_record() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running_root(temp.path(), "passed", &host);
    let candidate = candidate(temp.path(), &root);

    let (line, exit) = rescue_line(
        &host,
        &root,
        "config replace",
        json!({"file": word(&candidate)}),
    );
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    let journal = config_replace_journal(&root);
    assert_eq!(
        journal["result"]["code"], "install_command_done",
        "{journal}"
    );
    let step = post_check_step(&journal);
    assert_eq!(step["state"], "done", "{step}");
    let evidence = &step["evidence"];
    assert_eq!(
        members_of(evidence),
        ["checks", "commit", "failed"],
        "{step}"
    );
    assert_eq!(evidence["commit"], C1, "{step}");
    assert_eq!(evidence["failed"], json!([]), "{step}");
    assert!(failing_rows(evidence).is_empty(), "{step}");
}
