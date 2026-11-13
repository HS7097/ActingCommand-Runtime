// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 (register §1.340; contracts/install-cli.md, "The Lab's rescue module", `place`;
// coordinator work order `rd-1113/brief-453-s5s6-cold-place.md`; design note
// `rd-1113/fresh-root-design.md` §2): the rescue module's `place` on a root whose instances do not
// bind the bundle's game.
// S5 (2a), the composition the checks after a placement expect:
// - `s5_unbound_composition_passes_at_once`: a running Runtime whose one instance binds another
//   game, a bundle that changes `schedule.json`: the journal's input `bound` is empty, and the row
//   `composition` passes with `expected` `unchanged` and `bound` `[]` after one read of the
//   catalog, without a wait;
// - `s5_a_bound_instance_waits_for_a_new_composition`: with an instance bound to the game the row
//   waits, as before, until the catalog changes, and passes with `expected` `new`;
// - `s5_a_journal_without_bound_keeps_the_wait`: an operation stopped before its checks whose
//   journal has no `bound` (as an earlier build writes it), resumed with no instance bound to the
//   game, waits for a new catalog as before;
// - `s5_a_hot_placement_with_a_bound_instance_answers_as_before`: a placement with an instance
//   bound to the game and no `schedule.json` journals the steps of before, the inputs of before
//   and `bound`, the checks of before, and answers the members and steps of before (the literals
//   of the earlier gates `gate_434_lab3` and `gate_451_loadp`).
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files (a schedule here is placeholder
// bytes: `place` reads no rule of it, and the fixture's root carries no `standard-pack check`).

use super::fixture::{BuildSpec, C1, FakeHost, code, live_root, run, speaks, word, write_json};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The initiator the tests run the module as.
const WORD: Initiator = Initiator::LabRescue;
/// The neutral games, pack and table of the bundles (made-up ids, no real game).
const GAME: &str = "neutral-game";
const OTHER_GAME: &str = "neutral-other";
const PACK: &str = "neutral.pack";
const TABLE: &str = "neutral_scores";
/// The `package` range the installer of these tests declares, as this build's declaration does.
const INSTALLER_PACKAGE: [u64; 2] = [3, 10];
/// The `package` range every bundle here declares: [`speaks`]'s.
const BUNDLE_PACKAGE: [u64; 2] = [3, 5];
/// The pack's `control.json` and the game's `schedule.json` a bundle carries (placeholder bytes).
const CONTROL: &[u8] = br#"{"neutral": 1}"#;
const SCHEDULE: &[u8] = br#"{"neutral": "schedule"}"#;
/// Two catalogs the fake Runtime's latest evaluation ran under.
const CATALOG_A: &str = "neutral-catalog-a";
const CATALOG_B: &str = "neutral-catalog-b";

/// The bytes of the valid table [`TABLE`] of [`GAME`] every bundle carries.
fn table_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.table.v1",
        "game": GAME,
        "table_id": TABLE,
        "row_key": ["unit"],
        "columns": [
            {"name": "unit", "type": "key"},
            {"name": "score_milli", "type": "integer", "min": 0, "max": 100000}
        ],
        "rows": [["unit.a", 1000]]
    }))
    .expect("JSON")
}

/// Writes a neutral bundle zip of [`GAME`] to `path`: `bundle.json` with the pack [`PACK`] (its
/// `control.json` holding `control`) and the table [`TABLE`], `schedule.json` holding `schedule`
/// when given, and `interfaces.json` declaring [`BUNDLE_PACKAGE`]. The bytes of its `bundle.json`.
fn write_bundle(path: &Path, control: &[u8], schedule: Option<&[u8]>) -> Vec<u8> {
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": PACK,
            "server": "neutral-server",
            "entry_task_id": "neutral.entry",
            "path": format!("packs/{PACK}"),
            "file_count": 1,
            "byte_count": control.len(),
        }],
        "tables": [TABLE],
    }))
    .expect("JSON");
    let mut declared = speaks();
    declared["package"] = json!(BUNDLE_PACKAGE);
    let interfaces = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.component-interfaces.v1",
        "speaks": declared,
    }))
    .expect("JSON");
    let mut entries = vec![
        ("bundle.json".to_owned(), bundle.clone()),
        ("interfaces.json".to_owned(), interfaces),
        (format!("packs/{PACK}/control.json"), control.to_vec()),
        (format!("tables/{TABLE}.json"), table_bytes()),
    ];
    if let Some(schedule) = schedule {
        entries.push(("schedule.json".to_owned(), schedule.to_vec()));
    }
    let mut writer = zip::ZipWriter::new(fs::File::create(path).expect("create the bundle"));
    for (name, bytes) in entries {
        writer
            .start_file(
                name,
                zip::write::FileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .expect("start a zip entry");
        writer.write_all(&bytes).expect("write a zip entry");
    }
    writer.finish().expect("finish the bundle");
    bundle
}

/// A root `<base>\<name>` whose configuration names the standard-pack root `<base>\<name>-packs`,
/// adopted, with the empty game root of [`GAME`] in it; the installer declares
/// [`INSTALLER_PACKAGE`]. No Runtime runs. The root and the game root.
fn prepared(base: &Path, name: &str, host: &FakeHost) -> (PathBuf, PathBuf) {
    let root = live_root(base, name, &BuildSpec::new(C1));
    let packs = base.join(format!("{name}-packs"));
    let game_root = packs.join(GAME);
    fs::create_dir_all(&game_root).expect("the game root");
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration")).expect("JSON");
    config["standard_packs"] = json!({"root": packs});
    write_json(&config_path, &config);
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    host.installer_package.set(Some(INSTALLER_PACKAGE));
    (root, game_root)
}

/// The fake Runtime booted with `instances`, each `(alias, game)`: registered, unpaused, bound to
/// its game.
fn boot(host: &FakeHost, instances: &[(&str, &str)]) {
    let aliases = instances
        .iter()
        .map(|(alias, _)| *alias)
        .collect::<Vec<_>>();
    host.runtime.boot(&aliases);
    for instance in host.runtime.instances.borrow_mut().iter_mut() {
        instance.game = instances
            .iter()
            .find(|(alias, _)| *alias == instance.alias)
            .map(|(_, game)| (*game).to_owned());
    }
}

/// The rescue module's location of `root`.
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

/// `place` of the bundle `zip` on `root`.
fn place(host: &FakeHost, root: &Path, zip: &Path) -> (Value, u8) {
    rescue_line(host, root, "place", json!({"bundle": word(zip)}))
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

/// The path of the journal `operation_id` of `root`.
fn journal_path(root: &Path, operation_id: &str) -> PathBuf {
    root.join("install")
        .join("operations")
        .join(format!("{operation_id}.json"))
}

/// Every journal of the root, by operation id.
fn journals(root: &Path) -> Vec<(String, Value)> {
    let directory = root.join("install").join("operations");
    let mut found = fs::read_dir(&directory)
        .expect("the journals")
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

/// The rows of the journal's `post_check` step, in order.
fn check_rows(journal: &Value) -> Vec<Value> {
    journal["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .find(|step| step["kind"] == "post_check")
        .and_then(|step| step["evidence"]["checks"].as_array().cloned())
        .unwrap_or_else(|| panic!("the checks after the placement: {journal}"))
}

/// The row `id` of the journal's `post_check` step.
fn check_row(journal: &Value, id: &str) -> Value {
    check_rows(journal)
        .into_iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("no row {id}: {journal}"))
}

/// Every call of the fake Runtime that starts with `prefix`.
fn calls_of(host: &FakeHost, prefix: &str) -> Vec<String> {
    host.runtime
        .calls
        .borrow()
        .iter()
        .filter(|call| call.starts_with(prefix))
        .cloned()
        .collect()
}

// S5: a running Runtime whose one instance binds another game, and a bundle that changes the
// game's `schedule.json`. The input `bound` is empty; the row `composition` passes with `expected`
// `unchanged` and `bound` `[]`. The catalog is read three times: before the hold, once by the check
// (a wait reads it again after each 500 ms on the fake clock), and for the schedule after the
// release; the other game's instance is not paused.
#[test]
fn s5_unbound_composition_passes_at_once() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), "unbound", &host);
    boot(&host, &[("neutral.c", OTHER_GAME)]);
    *host.runtime.catalog.borrow_mut() = Some(CATALOG_A.to_owned());
    let zip = temp.path().join("neutral-unbound.zip");
    write_bundle(&zip, CONTROL, Some(SCHEDULE));

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    assert_eq!(placed["plan"]["schedule_changes"], json!(true), "{placed}");
    assert_eq!(placed["instances"], json!([]), "{placed}");
    let (_, journal) = new_journal(&root, &before);
    assert_eq!(journal["inputs"]["bound"], json!([]), "{journal}");
    assert_eq!(
        check_row(&journal, "composition"),
        json!({"id": "composition", "result": "pass", "values": {
            "catalog_hash": CATALOG_A,
            "catalog_before": CATALOG_A,
            "expected": "unchanged",
            "bound": [],
        }}),
        "{journal}"
    );
    assert_eq!(
        calls_of(&host, "catalog").len(),
        3,
        "{:?}",
        host.runtime.calls.borrow()
    );
    assert!(calls_of(&host, "pause ").is_empty());
    assert_eq!(
        fs::read(game_root.join("schedule.json")).expect("the placed schedule.json"),
        SCHEDULE
    );
}

// S5: an instance bound to the game, and a bundle that changes `schedule.json`. The input `bound`
// names it; the row `composition` waits as before: the catalog read by the check is the one read
// before the hold, so it waits 500 ms and reads again, and passes with `expected` `new` once the
// catalog changed (four reads in all).
#[test]
fn s5_a_bound_instance_waits_for_a_new_composition() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "bound", &host);
    boot(&host, &[("neutral.a", GAME)]);
    *host.runtime.catalog.borrow_mut() = Some(CATALOG_A.to_owned());
    host.runtime.catalogs.borrow_mut().extend([
        CATALOG_A.to_owned(),
        CATALOG_A.to_owned(),
        CATALOG_B.to_owned(),
    ]);
    let zip = temp.path().join("neutral-bound.zip");
    write_bundle(&zip, CONTROL, Some(SCHEDULE));

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let (_, journal) = new_journal(&root, &before);
    assert_eq!(
        journal["inputs"]["bound"],
        json!(["neutral.a"]),
        "{journal}"
    );
    assert_eq!(
        check_row(&journal, "composition"),
        json!({"id": "composition", "result": "pass", "values": {
            "catalog_hash": CATALOG_B,
            "catalog_before": CATALOG_A,
            "expected": "new",
        }}),
        "{journal}"
    );
    assert_eq!(
        calls_of(&host, "catalog").len(),
        4,
        "{:?}",
        host.runtime.calls.borrow()
    );
}

// S5: a placement with no instance bound to the game, stopped after its last item (before its
// checks); its journal then loses `bound`, as an earlier build writes it, and `op resume` runs the
// checks: the row `composition` waits as before (the catalog unchanged at the first read, changed
// at the second, two reads) and passes with `expected` `new`, without `bound`.
#[test]
fn s5_a_journal_without_bound_keeps_the_wait() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "older", &host);
    boot(&host, &[("neutral.c", OTHER_GAME)]);
    *host.runtime.catalog.borrow_mut() = Some(CATALOG_A.to_owned());
    let zip = temp.path().join("neutral-older.zip");
    write_bundle(&zip, CONTROL, Some(SCHEDULE));

    // The table, the pack, `bundle.json` and `schedule.json` are steps 2 to 5.
    host.crash_after.set(Some(5));
    let before = journals(&root);
    let (stopped, exit) = place(&host, &root, &zip);
    assert_ne!(exit, 0, "{stopped}");
    assert_eq!(
        top_code(&stopped),
        "install_operation_interrupted",
        "{stopped}"
    );
    let (operation_id, mut journal) = new_journal(&root, &before);
    assert_eq!(
        step_kinds(&journal),
        [
            "pause",
            "pack_item",
            "pack_item",
            "pack_item",
            "pack_item",
            "post_check",
            "release_hold"
        ]
    );
    assert_eq!(journal["steps"][5]["state"], "planned", "{journal}");
    let removed = journal["inputs"]
        .as_object_mut()
        .expect("the inputs")
        .remove("bound");
    assert_eq!(removed, Some(json!([])), "{journal}");
    write_json(&journal_path(&root, &operation_id), &journal);

    host.runtime
        .catalogs
        .borrow_mut()
        .extend([CATALOG_A.to_owned(), CATALOG_B.to_owned()]);
    let reads = calls_of(&host, "catalog").len();
    let (resumed, exit) = rescue_line(&host, &root, "op resume", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{resumed}");
    let journal: Value =
        serde_json::from_slice(&fs::read(journal_path(&root, &operation_id)).expect("the journal"))
            .expect("journal JSON");
    assert_eq!(
        check_row(&journal, "composition"),
        json!({"id": "composition", "result": "pass", "values": {
            "catalog_hash": CATALOG_B,
            "catalog_before": CATALOG_A,
            "expected": "new",
        }}),
        "{journal}"
    );
    assert_eq!(calls_of(&host, "catalog").len(), reads + 2);
}

// S5, the work order's added test: a placement with an instance bound to the game and no
// `schedule.json`, as the earlier gates place (`gate_451_loadp`'s steps, `gate_434_lab3`'s hold):
// the journal's steps are those of before, its inputs those of before and `bound`; the checks are
// `rerun`, `standard_pack_check` and `composition` with the values of before (no `bound`); the hold
// pauses and resumes the bound instance; the answer's members, steps and the `place` step's members
// are those of before.
#[test]
fn s5_a_hot_placement_with_a_bound_instance_answers_as_before() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "hot", &host);
    boot(&host, &[("neutral.a", GAME)]);
    let zip = temp.path().join("neutral-hot.zip");
    write_bundle(&zip, CONTROL, None);

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    assert_eq!(top_value(&placed, "operation"), "install_pack_place");
    assert_eq!(
        keys(&placed),
        ["facts", "instances", "outcome", "plan", "steps"]
    );
    assert_eq!(placed["instances"], json!(["neutral.a"]), "{placed}");
    let steps = placed["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .map(|step| step["step"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(steps, ["plan", "place", "schedule"], "{placed}");
    let placing = &placed["steps"][1]["result"];
    assert_eq!(
        keys(placing),
        ["checks", "game", "hold", "operation_id", "placed"]
    );
    assert_eq!(placing["hold"]["kept"], json!(false), "{placed}");

    let (_, journal) = new_journal(&root, &before);
    assert_eq!(journal["kind"], "pack_place");
    assert_eq!(journal["initiator"], WORD.as_str());
    assert_eq!(
        step_kinds(&journal),
        [
            "pause",
            "pack_item",
            "pack_item",
            "pack_item",
            "post_check",
            "release_hold"
        ]
    );
    assert_eq!(
        keys(&journal["inputs"]),
        [
            "bound",
            "commit",
            "config_sha256",
            "game",
            "hold",
            "instances",
            "known_pauses",
            "zip",
            "zip_sha256"
        ]
    );
    assert_eq!(journal["inputs"]["bound"], json!(["neutral.a"]));
    assert_eq!(journal["inputs"]["instances"], json!(["neutral.a"]));
    let ids = check_rows(&journal)
        .iter()
        .map(|row| row["id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["rerun", "standard_pack_check", "composition"]);
    assert_eq!(
        check_row(&journal, "composition"),
        json!({"id": "composition", "result": "pass", "values": {
            "catalog_hash": null,
            "catalog_before": null,
            "expected": "unchanged",
        }}),
        "{journal}"
    );
    let paused = calls_of(&host, "pause ");
    assert_eq!(paused.len(), 1, "{:?}", host.runtime.calls.borrow());
    assert!(
        paused[0].starts_with("pause neutral.a lab-place-"),
        "{paused:?}"
    );
    assert_eq!(calls_of(&host, "resume "), ["resume neutral.a"]);
}
