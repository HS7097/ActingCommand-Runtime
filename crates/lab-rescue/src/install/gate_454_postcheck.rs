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
// Workflow #454, the real cause (register §1.342; coordinator work order
// `rd-1113/brief-454-first-epoch.md`), the tests #1679 left out:
// - `postcheck_454_a_composite_rollback_failed_check_closes_its_step_with_its_rows`: `actingctl
//   install rollback apply … --stop --hold … --start` on a root past its first epoch whose
//   previous build's start seeds again: `post_check_failed` (`count` 1, `failed_checks`
//   `no_seeding`), the hold kept, the journal ended with that refusal and its `post_check` step
//   `done` with the evidence of a passed check, exactly `no_seeding` failed;
// - `postcheck_454_a_crash_before_the_end_leaves_the_step_started`: the write that would end a
//   `config replace` whose check failed does not happen (a crash before it): the operation is
//   `install_operation_interrupted`, its `post_check` step `started` with no evidence, no result,
//   `current.json` still naming it;
// and the main point, a restart of a Runtime in its root's first epoch (fix 1, brief
// `rd-1113/brief-1685-fix1.md`: the fact is the Runtime's program fact `config.start`, read with
// the configuration facts just before the close, and the close records it in its step's evidence,
// not in S0, which keeps the shape it has on main):
// - `postcheck_454_config_replace_after_a_first_epoch_passes_its_designed_seeding` and
//   `postcheck_454_instance_add_after_a_first_epoch_passes_its_designed_seeding`: the Runtime the
//   operation closes says its start was the root's first (`first_start`, seeded); the close's
//   evidence records `first_epoch`, the new start's `seed` passes `no_seeding` with the member that
//   names it, the operation ends done and the hold is released;
// - `postcheck_454_a_restart_past_the_first_epoch_fails_as_before`: the closed Runtime seeded but
//   was not the first (the root's second start, which wrote the marker): both commands fail as on
//   the base, the row `{tagged_state: seed}`, no `first_epoch`, the hold kept;
// - `postcheck_454_an_older_runtime_that_does_not_report_the_fact_is_judged_as_before`: a closed
//   Runtime that records no `config.start` (a build before 0.12.59), and one whose facts still
//   hold the fact of an earlier epoch (left by a newer build that ran before it), are judged as
//   before;
// - `postcheck_454_a_deployment_past_the_first_epoch_rolls_back_as_before`: `actingctl install`'s
//   composite update over a closed Runtime that seeded but was not the first rolls back on
//   `no_seeding` as before;
// - `postcheck_454_a_record_this_build_writes_is_read_by_mains_shapes` (fix 1, point 1): the
//   journal of a first-epoch `config replace` reads as main's journal and main's S0, the close's
//   evidence carrying `first_epoch`;
// - `postcheck_454_the_predicate_refuses_another_epoch_and_a_load` (fix 1, point 5): the predicate
//   is false for a fact of another epoch than S0's, and for a first start that loaded;
// and the review's first point, `bindings` against the configuration the restarted Runtime was
// given:
// - `postcheck_454_bindings_judges_the_new_configuration_without_a_removed_instance`: a `config
//   replace` that removes an instance: `bindings` passes, configured the new file's one instance;
// - `postcheck_454_bindings_judges_the_new_configuration_with_an_added_instance`: `instance add`:
//   the row lists the added instance among the configured, passes when the start binds it and
//   fails naming it when it does not.
// The machine is the fake host of `fixture.rs` with its fake Runtime, fake watchdog and fake
// clock: no device, no emulator, no sleep; every root is a temporary folder with neutral files.

use super::fixture::{BuildSpec, C1, C2, FakeHost, code, live_root, run, word, write_json};
use super::host::Host;
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

// ---- Workflow #454, the real cause (register §1.342) ----

/// The Runtime the test booted (build `live`, the one the operation closes) says, in its
/// `config.start` fact of its own epoch, that its start was the root's first: it found no earlier
/// start record and seeded, so it wrote no seeding marker (Workflow #422 LN-4) and the root's next
/// start seeds by design.
fn first_start_before(host: &FakeHost) {
    host.runtime.start_fact.borrow_mut().insert(
        "live".to_owned(),
        json!({"first_start": true, "tagged_state": "seed"}),
    );
}

/// The Runtime the test booted says a seeded start that was not the first: the root's second
/// start, which found the first one's start record and wrote the marker, so the next start must
/// load.
fn seeded_before(host: &FakeHost) {
    host.runtime.start_fact.borrow_mut().insert(
        "live".to_owned(),
        json!({"first_start": false, "tagged_state": "seed"}),
    );
}

/// The evidence of `journal`'s close step that is not an automatic rollback's (the close of the
/// Runtime S0 names).
fn close_evidence(journal: &Value) -> Value {
    journal["steps"]
        .as_array()
        .and_then(|steps| {
            steps
                .iter()
                .find(|step| step["kind"] == "close" && step["intent"].get("rollback").is_none())
        })
        .map(|step| step["evidence"].clone())
        .unwrap_or_else(|| panic!("a close step: {journal}"))
}

/// The row `id` of a post-start check's evidence.
fn row_of(evidence: &Value, id: &str) -> Value {
    evidence["checks"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == id))
        .cloned()
        .unwrap_or_else(|| panic!("the row {id}: {evidence}"))
}

/// The root's configuration document.
fn config_of(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("actingd.config.json")).expect("the configuration"))
        .expect("JSON")
}

/// The parameters of `instance add` of [`ADDED`] on [`instance_root`]'s root.
fn add_params() -> Value {
    json!({
        "alias": ADDED,
        "game": GAME,
        "server": SERVER,
        "mumu_index": 2,
        "touch_backend": "neutral-touch",
        "capture_backend": "neutral-capture",
    })
}

/// `config replace` of `root` with its own document written compactly (other bytes, the same
/// instances); its line and exit code.
fn replace(host: &FakeHost, base: &Path, root: &Path) -> (Value, u8) {
    let candidate = candidate(base, root);
    rescue_line(
        host,
        root,
        "config replace",
        json!({"file": word(&candidate)}),
    )
}

/// A commit whose new start seeded by design after a first epoch: done, the journal's close step
/// records `first_epoch` in its evidence (fix 1: not S0), its `post_check` step `done` with nothing
/// failed, and the row `no_seeding` passed with the tagged state it read and the member that names
/// the first epoch.
fn assert_designed_seeding_passed(line: &Value, exit: u8, journal: &Value) {
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        close_evidence(journal)["first_epoch"],
        json!(true),
        "{journal}"
    );
    assert_eq!(
        journal["result"]["code"], "install_command_done",
        "{journal}"
    );
    let step = post_check_step(journal);
    assert_eq!(step["state"], "done", "{step}");
    assert_eq!(step["evidence"]["failed"], json!([]), "{step}");
    assert_eq!(
        row_of(&step["evidence"], "no_seeding"),
        json!({
            "id": "no_seeding",
            "result": "pass",
            "fail_class": true,
            "values": {"tagged_state": "seed", "first_epoch": true},
        }),
        "{step}"
    );
}

/// No first epoch recorded: the journal's S0 as the base writes it, without a member
/// `first_epoch`, and (fix 1) the close step's evidence with `first_epoch` false.
fn assert_no_first_epoch(journal: &Value) {
    let s0 = journal["s0"]
        .as_object()
        .unwrap_or_else(|| panic!("an S0: {journal}"));
    assert!(!s0.contains_key("first_epoch"), "{journal}");
    assert_eq!(
        close_evidence(journal)["first_epoch"],
        json!(false),
        "{journal}"
    );
}

/// What the base keeps after `no_seeding` failed a commit: no first epoch recorded, and the hold
/// kept on every instance it took (`held`).
fn assert_failed_as_before(host: &FakeHost, line: &Value, journal: &Value, held: &[&str]) {
    assert_no_first_epoch(journal);
    assert_eq!(line["hold"]["kept"], json!(true), "{line}");
    let reason = line["hold"]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("the hold's reason: {line}"));
    for alias in held {
        assert_eq!(
            host.runtime.pause_of(alias).as_deref(),
            Some(reason),
            "{alias}: {line}"
        );
    }
}

// The work order's first test for the main point: `config replace` restarts a Runtime in its
// root's first epoch; the new start's designed seeding passes, the operation ends done and the
// hold is released.
#[test]
fn postcheck_454_config_replace_after_a_first_epoch_passes_its_designed_seeding() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running_root(temp.path(), "first", &host);
    first_start_before(&host);
    seeded_on_start(&host);

    let (line, exit) = replace(&host, temp.path(), &root);
    assert_eq!(
        top_value(&line, "operation"),
        "install_config_replace",
        "{line}"
    );
    assert_designed_seeding_passed(&line, exit, &config_replace_journal(&root));
    assert_eq!(
        host.runtime.pause_of("neutral.a"),
        None,
        "the hold released"
    );
}

// The same for `instance add`: its hold's instance pauses and its global pause are released.
#[test]
fn postcheck_454_instance_add_after_a_first_epoch_passes_its_designed_seeding() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = instance_root(temp.path(), "first-added", &host);
    first_start_before(&host);
    seeded_on_start(&host);

    let (line, exit) = rescue_line(&host, &root, "instance add", add_params());
    assert_eq!(
        top_value(&line, "operation"),
        "install_instance_add",
        "{line}"
    );
    assert_designed_seeding_passed(&line, exit, &config_replace_journal(&root));
    assert_eq!(
        host.runtime.pause_of("neutral.a"),
        None,
        "the hold released"
    );
    assert_eq!(
        host.runtime.pause_of("neutral.b"),
        None,
        "the hold released"
    );
    assert!(
        host.runtime.global_pause.borrow().is_none(),
        "the hold's global pause released"
    );
}

// The same restart on a root past its first epoch, the closed Runtime the root's second start
// (seeded, the marker written): both commands fail exactly as on the base.
#[test]
fn postcheck_454_a_restart_past_the_first_epoch_fails_as_before() {
    {
        let temp = TempDir::new().expect("tempdir");
        let host = FakeHost::new(&temp.path().join("work"));
        let root = running_root(temp.path(), "second", &host);
        seeded_before(&host);
        seeded_on_start(&host);
        let (line, exit) = replace(&host, temp.path(), &root);
        let journal = config_replace_journal(&root);
        assert_closed_with_the_failed_row(&line, exit, &journal);
        assert_failed_as_before(&host, &line, &journal, &["neutral.a"]);
    }
    {
        let temp = TempDir::new().expect("tempdir");
        let host = FakeHost::new(&temp.path().join("work"));
        let root = instance_root(temp.path(), "second-added", &host);
        seeded_before(&host);
        seeded_on_start(&host);
        let (line, exit) = rescue_line(&host, &root, "instance add", add_params());
        let journal = config_replace_journal(&root);
        assert_closed_with_the_failed_row(&line, exit, &journal);
        assert_failed_as_before(&host, &line, &journal, &["neutral.a", "neutral.b"]);
    }
}

// The older Runtime's test: a closed Runtime that does not say the fact of its own epoch is judged
// as before, whether it records no `config.start` (a build before 0.12.59, even on a root's first
// start) or its facts still hold the one a newer build recorded in an earlier epoch, saying that
// start was the first (fix 1: a build that does not record the fact does not supersede it).
#[test]
fn postcheck_454_an_older_runtime_that_does_not_report_the_fact_is_judged_as_before() {
    for an_earlier_epochs_fact in [false, true] {
        let temp = TempDir::new().expect("tempdir");
        let host = FakeHost::new(&temp.path().join("work"));
        let root = running_root(temp.path(), "older", &host);
        if an_earlier_epochs_fact {
            // The booted Runtime runs in `epoch-1`; the fact names the epoch before it.
            host.runtime.start_fact.borrow_mut().insert(
                "live".to_owned(),
                json!({"owner_epoch": "epoch-0", "first_start": true, "tagged_state": "seed"}),
            );
        }
        seeded_on_start(&host);
        let (line, exit) = replace(&host, temp.path(), &root);
        let journal = config_replace_journal(&root);
        assert_closed_with_the_failed_row(&line, exit, &journal);
        assert_failed_as_before(&host, &line, &journal, &["neutral.a"]);
    }
}

/// The value `key` of an installer answer's outcome, as text.
fn text_of<'a>(answer: &'a super::Answer, key: &str) -> Option<&'a str> {
    answer.outcome.value(key).and_then(Value::as_str)
}

/// The journal `operation_id` of `root`.
fn journal_named(root: &Path, operation_id: &str) -> Value {
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

/// The journal an installer answer names.
fn journal_of_answer(root: &Path, answer: &super::Answer) -> Value {
    let id = text_of(answer, "install_operation_id")
        .unwrap_or_else(|| panic!("an operation id: {answer:?}"));
    journal_named(root, id)
}

/// A root as `actingctl install`'s composite takes one (`gate_inst2_deploy.rs`'s `prepared`): the
/// live build C1, adopted, the hold released after a start (`install.hold_after_start` false), C2
/// staged, a start of C1 logged after the adopt (the rollback gate's open of C1); its Runtime
/// running `node.a`.
fn deploy_root(base: &Path, name: &str, host: &FakeHost) -> PathBuf {
    let root = live_root(base, name, &BuildSpec::new(C1));
    let path = root.join("actingd.config.json");
    let mut config = config_of(&root);
    config["install"] = json!({"hold_after_start": false});
    write_json(&path, &config);
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    let release = base.join(format!("{name}-release"));
    BuildSpec::new(C2).write_release(&release);
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
    assert_eq!(code(&staged), "install_command_done", "{staged:?}");
    let open = json!({"schema_version": "actingcommand.outcome.v1", "code": "ledger_opened",
        "category": "info", "values": {"sql_read_ms": 0, "verify_ms": 0, "material_ms": 0,
        "restore_ms": 0, "meta_ms": 10, "tail_ms": 20, "index_ms": 900, "import_ms": 0,
        "timeout_ms": 600_000}});
    let at = host.now_unix_ms();
    fs::write(
        root.join(format!("actingd-{at}.log")),
        format!("OUTCOME actingd: {open}\n"),
    )
    .expect("the previous build's start log");
    host.runtime.boot(&["node.a"]);
    root
}

/// `actingctl install <kind> plan … --preview` (the Runtime runs), then `<kind> apply … --plan
/// <id> --stop --hold <reason> --start` toward `commit`: `update` with `--build`, `rollback` with
/// `--to`.
fn composite_to(
    host: &FakeHost,
    root: &Path,
    kind: &str,
    commit: &str,
    reason: &str,
) -> super::Answer {
    let flag = if kind == "update" { "--build" } else { "--to" };
    let planned = run(
        host,
        &[
            kind,
            "plan",
            "--root",
            word(root),
            flag,
            commit,
            "--preview",
        ],
    );
    assert_eq!(code(&planned), "install_command_done", "{planned:?}");
    let plan_id = planned.members["plan_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a plan id: {planned:?}"))
        .to_owned();
    run(
        host,
        &[
            kind,
            "apply",
            "--root",
            word(root),
            flag,
            commit,
            "--plan",
            plan_id.as_str(),
            "--stop",
            "--hold",
            reason,
            "--start",
        ],
    )
}

// The work order's deployment through `actingctl install` on a root past its first epoch: the
// closed Runtime seeded but was not the first, the new build seeds again: the composite rolls back
// on `no_seeding` as before.
#[test]
fn postcheck_454_a_deployment_past_the_first_epoch_rolls_back_as_before() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = deploy_root(temp.path(), "deploy", &host);
    seeded_before(&host);
    host.runtime
        .report_patch
        .borrow_mut()
        .insert(C2.to_owned(), json!({"tagged_state": "seed"}));

    let answer = composite_to(&host, &root, "update", C2, "deploy-454");
    assert_eq!(code(&answer), "install_command_refused", "{answer:?}");
    assert_eq!(
        text_of(&answer, "install_refusal"),
        Some("deploy_rolled_back"),
        "{answer:?}"
    );
    assert_eq!(
        answer.members["failed_checks"],
        json!(["no_seeding"]),
        "{answer:?}"
    );
    let journal = journal_of_answer(&root, &answer);
    assert_no_first_epoch(&journal);
    let checked = journal["steps"]
        .as_array()
        .and_then(|steps| {
            steps.iter().find(|step| {
                step["kind"] == "post_check" && step["intent"].get("rollback").is_none()
            })
        })
        .cloned()
        .unwrap_or_else(|| panic!("the new build's post_check step: {journal}"));
    assert_eq!(checked["state"], "done", "{checked}");
    assert_eq!(
        row_of(&checked["evidence"], "no_seeding"),
        json!({
            "id": "no_seeding",
            "result": "fail",
            "fail_class": true,
            "values": {"tagged_state": "seed"},
        }),
        "{checked}"
    );
}

// #1679 left this out: a composite rollback through `actingctl install`, which never rolls back
// by itself, on a root past its first epoch: a failed check ends it `post_check_failed` with the
// hold kept, and its journal's `post_check` step is closed `done` with what it read.
#[test]
fn postcheck_454_a_composite_rollback_failed_check_closes_its_step_with_its_rows() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = deploy_root(temp.path(), "rollback", &host);
    let updated = composite_to(&host, &root, "update", C2, "deploy-454");
    assert_eq!(code(&updated), "install_command_done", "{updated:?}");
    // The previous build's next start reports its tagged state seeded again.
    host.runtime
        .report_patch
        .borrow_mut()
        .insert(C1.to_owned(), json!({"tagged_state": "seed"}));

    let answer = composite_to(&host, &root, "rollback", C1, "rollback-454");
    assert_eq!(code(&answer), "install_command_refused", "{answer:?}");
    assert_eq!(
        text_of(&answer, "operation"),
        Some("install_rollback_apply"),
        "{answer:?}"
    );
    assert_eq!(
        text_of(&answer, "install_refusal"),
        Some("post_check_failed"),
        "{answer:?}"
    );
    assert_eq!(answer.outcome.value("count"), Some(&json!(1)), "{answer:?}");
    assert_eq!(
        answer.members["failed_checks"],
        json!(["no_seeding"]),
        "{answer:?}"
    );
    assert_eq!(answer.members["hold"]["kept"], json!(true), "{answer:?}");
    assert_eq!(
        host.runtime.pause_of("node.a").as_deref(),
        Some("rollback-454"),
        "the hold kept"
    );

    let journal = journal_of_answer(&root, &answer);
    assert_eq!(journal["kind"], "rollback", "{journal}");
    assert_no_first_epoch(&journal);
    assert_eq!(
        journal["result"]["values"]["install_refusal"], "post_check_failed",
        "{journal}"
    );
    assert!(journal["finished_at_unix_ms"].is_u64(), "{journal}");
    let step = post_check_step(&journal);
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
        evidence["checks"], answer.members["post_check"],
        "the record holds the rows the answer listed"
    );
}

// #1679 left this out: a crash before the write that ends the operation (here, that write fails:
// its `.part` is a folder) leaves the `post_check` step `started` with no evidence and the
// operation unfinished, for `resume` to run the checks again.
#[test]
fn postcheck_454_a_crash_before_the_end_leaves_the_step_started() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running_root(temp.path(), "crash", &host);
    seeded_on_start(&host);
    // When the new start's report is read (the checks run, their step journalled `started`),
    // every journal's next write is made to fail.
    let operations = root.join("install").join("operations");
    *host.runtime.on_report.borrow_mut() = Some(Box::new(move |build: &str| {
        if build != C1 {
            return;
        }
        for entry in fs::read_dir(&operations).expect("the journals").flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("op-") && name.ends_with(".json") {
                fs::create_dir_all(operations.join(format!("{name}.part")))
                    .expect("the journal's next write blocked");
            }
        }
    }));

    let (line, exit) = replace(&host, temp.path(), &root);
    assert_ne!(exit, 0, "{line}");
    assert_eq!(
        line["outcome"]["code"], "install_operation_interrupted",
        "{line}"
    );
    let journal = config_replace_journal(&root);
    let step = post_check_step(&journal);
    assert_eq!(step["state"], "started", "{step}");
    assert!(step["evidence"].is_null(), "{step}");
    assert!(journal["result"].is_null(), "{journal}");
    assert!(journal["finished_at_unix_ms"].is_null(), "{journal}");
    assert!(
        root.join("install").join("current.json").is_file(),
        "the operation stays unfinished"
    );
}

/// A root whose live build is C1, adopted, its configuration naming the instances `aliases` (by
/// alias alone), with its Runtime running them.
fn aliased_root(base: &Path, name: &str, host: &FakeHost, aliases: &[&str]) -> PathBuf {
    let root = live_root(base, name, &BuildSpec::new(C1));
    let mut config = config_of(&root);
    config["instances"] = Value::Array(
        aliases
            .iter()
            .map(|alias| json!({"alias": alias}))
            .collect(),
    );
    write_json(&root.join("actingd.config.json"), &config);
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    host.runtime.boot(aliases);
    root
}

// The review's first point: a `config replace` whose new file removes an instance. The
// restarted Runtime no longer binds it, and `bindings` judges the new file: it passes. (The hold's
// pause on the removed instance is not restored, so `pauses` fails, and nothing else:
// contracts/install-cli.md, "Taking it back".)
#[test]
fn postcheck_454_bindings_judges_the_new_configuration_without_a_removed_instance() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = aliased_root(temp.path(), "removed", &host, &["neutral.a", "neutral.b"]);
    let mut document = config_of(&root);
    document["instances"] = json!([{"alias": "neutral.a"}]);
    let candidate = temp.path().join("candidate.json");
    fs::write(&candidate, serde_json::to_vec(&document).expect("JSON")).expect("the candidate");
    host.runtime
        .drops_at_start
        .borrow_mut()
        .push("neutral.b".to_owned());

    let (line, exit) = rescue_line(
        &host,
        &root,
        "config replace",
        json!({"file": word(&candidate)}),
    );
    let step = post_check_step(&config_replace_journal(&root));
    assert_eq!(step["state"], "done", "{step}");
    assert_eq!(
        row_of(&step["evidence"], "bindings"),
        json!({
            "id": "bindings",
            "result": "pass",
            "fail_class": true,
            "values": {"configured": ["neutral.a"], "missing": []},
        }),
        "{step}"
    );
    assert_eq!(exit, 1, "{line}");
    assert_eq!(line["failed_checks"], json!(["pauses"]), "{line}");
}

// The review's first point: `instance add`. `bindings` judges the new file: the added instance
// is among the configured ones, the row passes when the restarted Runtime binds it, and fails
// naming it when it does not (the commit then ends `post_check_failed`, the hold kept).
#[test]
fn postcheck_454_bindings_judges_the_new_configuration_with_an_added_instance() {
    let configured = json!(["neutral.a", ADDED, "neutral.b"]);
    {
        let temp = TempDir::new().expect("tempdir");
        let host = FakeHost::new(&temp.path().join("work"));
        let root = instance_root(temp.path(), "bound", &host);
        let (line, exit) = rescue_line(&host, &root, "instance add", add_params());
        assert_eq!(exit, 0, "{line}");
        let step = post_check_step(&config_replace_journal(&root));
        assert_eq!(
            row_of(&step["evidence"], "bindings"),
            json!({
                "id": "bindings",
                "result": "pass",
                "fail_class": true,
                "values": {"configured": configured, "missing": []},
            }),
            "{step}"
        );
    }
    {
        let temp = TempDir::new().expect("tempdir");
        let host = FakeHost::new(&temp.path().join("work"));
        let root = instance_root(temp.path(), "unbound", &host);
        host.runtime.registers_at_start.borrow_mut().clear();
        let (line, exit) = rescue_line(&host, &root, "instance add", add_params());
        assert_eq!(exit, 1, "{line}");
        assert_eq!(
            top_value(&line, "install_refusal"),
            "post_check_failed",
            "{line}"
        );
        assert_eq!(line["failed_checks"], json!(["bindings"]), "{line}");
        assert_eq!(line["hold"]["kept"], json!(true), "{line}");
        let step = post_check_step(&config_replace_journal(&root));
        assert_eq!(
            row_of(&step["evidence"], "bindings"),
            json!({
                "id": "bindings",
                "result": "fail",
                "fail_class": true,
                "values": {"configured": configured, "missing": [ADDED]},
            }),
            "{step}"
        );
    }
}

// ---- Fix 1 (brief rd-1113/brief-1685-fix1.md) ----

/// S0 as main declares it (`journal.rs`, which this slice leaves as it is on main): its four
/// members and no other, refusing any other as main's does.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MainS0 {
    head: Option<u64>,
    epoch: Option<String>,
    pauses: Option<Value>,
    instructions: Option<Value>,
}

// Point 1: a journal this build writes for a first-epoch `config replace` is read by the
// structures as they are on main: the whole record by the journal's own structure (main's shape,
// byte for byte), its S0 by main's four members, and the fact is in the close step's free-form
// evidence, which every version reads.
#[test]
fn postcheck_454_a_record_this_build_writes_is_read_by_mains_shapes() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running_root(temp.path(), "main-shapes", &host);
    first_start_before(&host);
    seeded_on_start(&host);
    let (line, exit) = replace(&host, temp.path(), &root);
    assert_eq!(exit, 0, "{line}");
    let journal = config_replace_journal(&root);

    let read = serde_json::from_value::<super::journal::Journal>(journal.clone())
        .unwrap_or_else(|error| panic!("main's journal reads it: {error}: {journal}"));
    assert!(read.finished(), "{journal}");
    let MainS0 {
        head,
        epoch,
        pauses,
        instructions,
    } = serde_json::from_value::<MainS0>(journal["s0"].clone())
        .unwrap_or_else(|error| panic!("main's S0 reads it: {error}: {journal}"));
    assert!(head.is_some(), "{journal}");
    assert_eq!(epoch.as_deref(), Some("epoch-1"), "{journal}");
    assert!(pauses.is_some(), "{journal}");
    assert!(instructions.is_some(), "{journal}");
    assert_eq!(
        close_evidence(&journal)["first_epoch"],
        json!(true),
        "{journal}"
    );
}

// Point 5: the predicate gives false for a fact of another epoch than S0's, and for a first start
// that loaded; the same fact of S0's epoch that seeded gives true.
#[test]
fn postcheck_454_the_predicate_refuses_another_epoch_and_a_load() {
    use super::postcheck::first_epoch_of;
    let seeded = json!({"owner_epoch": "epoch-1", "first_start": true, "tagged_state": "seed"});
    assert!(first_epoch_of(&seeded, Some("epoch-1")), "S0's first epoch");
    assert!(
        !first_epoch_of(&seeded, Some("epoch-2")),
        "the fact belongs to another epoch than S0"
    );
    let loaded = json!({"owner_epoch": "epoch-1", "first_start": true, "tagged_state": "load"});
    assert!(!first_epoch_of(&loaded, Some("epoch-1")), "first but load");
}
