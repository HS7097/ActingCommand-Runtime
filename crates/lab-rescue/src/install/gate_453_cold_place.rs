// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 (register §1.342; contracts/install-cli.md, "The Lab's rescue module", `place`;
// coordinator work order `rd-1113/brief-453-s5s6-cold-place.md`; design note
// `rd-1113/fresh-root-design.md` §2): the rescue module's `place` on a root whose instances do not
// bind the bundle's game, and while the Runtime is stopped.
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
// S6 (2b), `place` while no Runtime answers:
// - `s6_cold_place_places_the_items_and_keeps_the_replaced_ones`: the items placed, each replaced
//   one kept as `.old-<ms>`; the journal with the input `cold`, one `pack_item` step per item (none
//   waiting) and `post_check`, no `pause` and no `release_hold`; the rows `rerun`,
//   `standard_pack_check`, `check_config` (the live build's, `--install-root`) and `composition`
//   `not_evaluated`; nothing asked of the Runtime; the watchdog's row 7a″ stands aside meanwhile;
// - `s6_cold_place_refuses_a_running_actingd_or_a_held_owner_lock`: an actingd under the root
//   refuses `runtime_running`, the state root's owner lock held `owner_lock_held`, each with
//   nothing written;
// - `s6_a_failed_check_names_undo_and_undo_restores_the_game_root`: a configuration the live
//   build's check-config refuses ends `post_check_failed` with the items placed and `next` naming
//   `undo` alone; that `undo` restores the game root, the placed items kept beside it;
// - `s6_the_same_bundle_has_the_same_plan_id_hot_and_cold`: planned and placed with the Runtime
//   running and stopped, the same plan and plan id.
// S6 fix 1 (coordinator work order `rd-1113/brief-1680-fix1.md`, after the independent review):
// - `s6_fix1_a_note_naming_the_placed_game_fails_check_config`: check-config passes with a note
//   that names the placed game: the row `check_config` fails with that note in `game_notes`, the
//   operation ends `post_check_failed` with `undo` as its next step;
// - `s6_fix1_no_bound_instance_says_the_composition_was_not_previewed`: no instance the
//   configuration binds to the game: the input `bound` empty, the row says `composition`
//   `not_previewed`, and the answer's `next` names the start;
// - `s6_fix1_a_bound_instance_and_no_note_passes_check_config`: an instance bound to the game and
//   no note: the row passes without that member;
// - `s6_fix1_a_check_config_that_does_not_answer_fails_its_row`: check-config still running at
//   its deadline: the row fails with `check_config_unanswered`, the operation `post_check_failed`;
// - `s6_fix1_undo_of_a_stopped_placement_refuses_a_running_runtime` and
//   `s6_fix1_resume_of_a_stopped_placement_refuses_a_running_runtime`: under the writer lock the
//   stopped rule again (`runtime_running`, `owner_lock_held`), nothing moved, then done once the
//   Runtime is stopped;
// - `s6_fix1_undo_and_resume_of_a_running_placement_are_unchanged`: with a Runtime, both go on
//   while an actingd runs under the root, as before;
// - `s6_fix1_a_failed_check_closes_its_step_with_what_it_read` and
//   `s6_fix1_a_passed_check_keeps_its_record`: as Workflow #454 closes the post-start checks, with
//   a Runtime and without one.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files (a schedule here is placeholder
// bytes: `place` reads no rule of it, and the fixture's root carries no `standard-pack check`).

use super::fixture::{
    BuildSpec, C1, FakeHost, STALLED_PID, code, live_root, run, speaks, tree, word, write_json,
};
use super::host::RunningImage;
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
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

/// The names of a composite line's steps, in order.
fn step_names(line: &Value) -> Vec<String> {
    line["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .map(|step| step["step"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The one entry of `game_root` whose name starts with `prefix`.
fn kept(game_root: &Path, prefix: &str) -> PathBuf {
    let found = fs::read_dir(game_root)
        .expect("the game root")
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "{prefix}: {found:?}");
    found[0].clone()
}

/// Writes into `game_root` the pack [`PACK`] and a `bundle.json` of before (placeholder bytes, as
/// `gate_434_lab3` writes them): the bytes of the pack's `control.json` and of `bundle.json`.
fn old_game_root(game_root: &Path) -> (Vec<u8>, Vec<u8>) {
    let control = br#"{"neutral": "the pack before"}"#.to_vec();
    let bundle = br#"{"neutral": "the bundle before"}"#.to_vec();
    fs::create_dir_all(game_root.join(PACK)).expect("the pack folder");
    fs::write(game_root.join(PACK).join("control.json"), &control).expect("the old pack");
    fs::write(game_root.join("bundle.json"), &bundle).expect("the old bundle.json");
    (control, bundle)
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
    assert_eq!(
        step_names(&placed),
        ["plan", "place", "schedule"],
        "{placed}"
    );
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

// S6: no Runtime runs. The game root holds the pack and a `bundle.json` of before; the bundle
// replaces both and adds the table and `schedule.json`. The answer's steps are `plan` and `place`;
// each item is placed and each replaced one kept as `.old-<ms>`; the journal `pack_place`, with the
// initiator, the same plan id as the answer's plan and the input `cold` among the inputs of a
// placement without its hold, has no S0 and one `pack_item` step per item (none waiting for idle
// instances) and `post_check`, no `pause` and no `release_hold`; the rows are `rerun` (pass),
// `standard_pack_check` (`not_evaluated`: the fixture's root carries no `actinglab.exe`),
// `check_config` (pass: the live build's check-config, run once, of the live configuration with
// `--install-root`) and `composition` (`not_evaluated`, `runtime` `stopped`). Nothing is asked of
// the fake Runtime. After each of the five steps `current.json` names the operation, so the
// watchdog's row 7a″ stands aside (no start on a half-placed game root).
#[test]
fn s6_cold_place_places_the_items_and_keeps_the_replaced_ones() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), "cold", &host);
    let (old_control, old_bundle) = old_game_root(&game_root);
    let zip = temp.path().join("neutral-cold.zip");
    let new_control = br#"{"neutral": 2}"#;
    let new_bundle = write_bundle(&zip, new_control, Some(SCHEDULE));
    let unfinished = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&unfinished);
    let watched = root.clone();
    host.on_step.replace(Some(Box::new(move |_step: usize| {
        seen.borrow_mut()
            .push(super::unfinished_operation(&watched));
    })));
    let logged = host.log.borrow().len();

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    assert_eq!(top_value(&placed, "operation"), "install_pack_place");
    assert_eq!(step_names(&placed), ["plan", "place"], "{placed}");
    assert_eq!(placed["instances"], json!([]), "{placed}");

    // The items placed, each replaced one kept beside its place.
    assert_eq!(
        fs::read(game_root.join(PACK).join("control.json")).expect("the placed pack"),
        new_control
    );
    assert_eq!(
        fs::read(game_root.join("bundle.json")).expect("the placed bundle.json"),
        new_bundle
    );
    assert_eq!(
        fs::read(game_root.join("tables").join(format!("{TABLE}.json"))).expect("the table"),
        table_bytes()
    );
    assert_eq!(
        fs::read(game_root.join("schedule.json")).expect("the placed schedule.json"),
        SCHEDULE
    );
    assert_eq!(
        fs::read(kept(&game_root, &format!("{PACK}.old-")).join("control.json"))
            .expect("the kept pack"),
        old_control
    );
    assert_eq!(
        fs::read(kept(&game_root, "bundle.json.old-")).expect("the kept bundle.json"),
        old_bundle
    );

    // The journal: no hold, the items and the checks.
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(journal["kind"], "pack_place");
    assert_eq!(journal["initiator"], WORD.as_str());
    assert_eq!(journal["plan_id"], placed["plan"]["plan_id"]);
    assert_eq!(journal["s0"], Value::Null, "{journal}");
    // Fix 1: and `bound`, the instances the configuration binds to the game (none here).
    assert_eq!(
        keys(&journal["inputs"]),
        [
            "bound",
            "cold",
            "commit",
            "config_sha256",
            "game",
            "zip",
            "zip_sha256"
        ]
    );
    assert_eq!(journal["inputs"]["cold"], json!(true));
    assert_eq!(journal["inputs"]["bound"], json!([]));
    assert_eq!(
        step_kinds(&journal),
        [
            "pack_item",
            "pack_item",
            "pack_item",
            "pack_item",
            "post_check"
        ]
    );
    for step in journal["steps"].as_array().expect("the steps") {
        assert!(step["intent"].get("first").is_none(), "{step}");
    }
    let rows = check_rows(&journal)
        .iter()
        .map(|row| {
            format!(
                "{} {}",
                row["id"].as_str().unwrap_or_default(),
                row["result"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            "rerun pass",
            "standard_pack_check not_evaluated",
            "check_config pass",
            "composition not_evaluated"
        ]
    );
    assert_eq!(
        check_row(&journal, "composition")["values"],
        json!({"runtime": "stopped"})
    );
    assert_eq!(
        check_row(&journal, "check_config")["values"]["install_root"],
        json!(true)
    );
    let checked = host.log.borrow()[logged..]
        .iter()
        .filter(|line| line.contains("check-config"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(checked.len(), 1, "{checked:?}");
    assert!(
        checked[0].starts_with("live actingcommand-actingd.exe check-config --config ")
            && checked[0].contains(" --install-root "),
        "{checked:?}"
    );

    // Nothing asked of the Runtime; the watchdog stood aside while the operation was unfinished.
    assert!(
        host.runtime.calls.borrow().is_empty(),
        "{:?}",
        host.runtime.calls.borrow()
    );
    let unfinished = unfinished.borrow();
    let named = Some(Some(operation_id));
    assert_eq!(unfinished.len(), 5, "{unfinished:?}");
    assert!(
        unfinished.iter().all(|read| *read == named),
        "{unfinished:?}"
    );
}

// S6: no Runtime answers. An actingd under the root (a Runtime starting, say) refuses
// `runtime_running` with its `pid`; the state root's owner lock held refuses `owner_lock_held`.
// Each refusal comes before anything is written: the game root as it was, no journal, no
// `current.json`. Fix 1: the second case pins a window. The machine's status read answers "no
// Runtime" only while the owner lock is not held; a held lock whose Runtime does not answer is
// `processes_unreadable` at that read, before this path is chosen. So on a real installation
// `owner_lock_held` comes only from a Runtime that took the lock between the status read and the
// check under the writer lock; the fake Runtime's status answers "none runs" without reading the
// lock, which is how this test reaches that window.
#[test]
fn s6_cold_place_refuses_a_running_actingd_or_a_held_owner_lock() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), "refused", &host);
    old_game_root(&game_root);
    let zip = temp.path().join("neutral-refused.zip");
    write_bundle(&zip, CONTROL, Some(SCHEDULE));
    let unchanged = tree(&game_root);
    let before = journals(&root);
    let nothing_written = || {
        assert_eq!(tree(&game_root), unchanged);
        assert_eq!(journals(&root).len(), before.len());
        assert!(!root.join("install").join("current.json").exists());
    };

    host.processes.replace(vec![RunningImage {
        pid: 41,
        path: format!(
            "{}\\runtime\\actingcommand-actingd.exe",
            super::files::plain(&root)
        ),
        mcp_serve: false,
        parent_pid: None,
    }]);
    let (refused, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(top_value(&refused, "operation"), "install_pack_place");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "runtime_running",
        "{refused}"
    );
    assert_eq!(top_value(&refused, "pid"), &json!(41), "{refused}");
    nothing_written();
    host.processes.replace(Vec::new());

    let mut owner = fs::File::create(temp.path().join("refused-state").join("owner.lock"))
        .expect("the owner journal");
    owner
        .write_all(format!("{}\n", owner_record()).as_bytes())
        .expect("write the owner journal");
    owner.lock().expect("hold the owner lock");
    let (refused, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "owner_lock_held",
        "{refused}"
    );
    drop(owner);
    nothing_written();
    assert!(host.runtime.calls.borrow().is_empty());
}

// S6: no Runtime runs, and the live build's check-config refuses the live configuration. The
// placement ends `post_check_failed` (`count` 1, `game`, its operation id) after the items are
// placed: the failed row is `check_config` with check-config's answer, no hold is kept, and `next`
// names `undo` of the operation alone; the answer's steps are `plan`. That `undo` restores the game
// root: without the four placed items, set aside beside their places and kept, it is the game root
// of before.
#[test]
fn s6_a_failed_check_names_undo_and_undo_restores_the_game_root() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), "failed", &host);
    old_game_root(&game_root);
    let unchanged = tree(&game_root);
    let zip = temp.path().join("neutral-failed.zip");
    let new_control = br#"{"neutral": 2}"#;
    write_bundle(&zip, new_control, Some(SCHEDULE));
    host.programs
        .refuse_check_config
        .borrow_mut()
        .push("live".to_owned());

    let before = journals(&root);
    let (refused, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "post_check_failed",
        "{refused}"
    );
    assert_eq!(top_value(&refused, "count"), &json!(1), "{refused}");
    assert_eq!(top_value(&refused, "game"), GAME, "{refused}");
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(
        top_value(&refused, "install_operation_id"),
        operation_id.as_str(),
        "{refused}"
    );
    assert_eq!(
        refused["checks"]["failed"],
        json!(["check_config"]),
        "{refused}"
    );
    let failed = refused["checks"]["checks"]
        .as_array()
        .expect("the rows")
        .iter()
        .find(|row| row["id"] == "check_config")
        .cloned()
        .expect("the row check_config");
    assert_eq!(failed["result"], "fail", "{failed}");
    assert_eq!(
        failed["values"]["answer"]["outcome"]["code"], "config_decode_failed",
        "{failed}"
    );
    assert_eq!(
        refused["next"],
        json!({"undo": ["undo", operation_id]}),
        "{refused}"
    );
    assert!(refused.get("hold").is_none(), "{refused}");
    assert_eq!(step_names(&refused), ["plan"], "{refused}");
    assert!(journal["finished_at_unix_ms"].is_u64(), "{journal}");
    assert!(!root.join("install").join("current.json").exists());
    assert_eq!(
        fs::read(game_root.join(PACK).join("control.json")).expect("the placed pack"),
        new_control
    );

    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    let after = tree(&game_root);
    let restored = after
        .iter()
        .filter(|(path, _)| !path.contains(".rolledback-"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(restored, unchanged);
    assert_eq!(after.len() - restored.len(), 4, "{:?}", after.keys());
    assert!(host.runtime.calls.borrow().is_empty());
}

// S6: two roots whose game roots hold the same files of before, and one bundle. Planned with the
// Runtime stopped and with it running, the plan id is the same; placed with it running on one root
// (its journal begins with `pause`) and stopped on the other (its journal begins with
// `pack_item`), both journals carry that plan id and the same plan document.
#[test]
fn s6_the_same_bundle_has_the_same_plan_id_hot_and_cold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (hot_root, hot_game_root) = prepared(temp.path(), "hot", &host);
    let (cold_root, cold_game_root) = prepared(temp.path(), "cold", &host);
    old_game_root(&hot_game_root);
    old_game_root(&cold_game_root);
    let zip = temp.path().join("neutral-both.zip");
    write_bundle(&zip, CONTROL, None);
    let plan_only = json!({"bundle": word(&zip), "plan": true});

    let (cold_plan, exit) = rescue_line(&host, &cold_root, "place", plan_only.clone());
    assert_eq!(exit, 0, "{cold_plan}");
    boot(&host, &[("neutral.a", GAME)]);
    let (hot_plan, exit) = rescue_line(&host, &hot_root, "place", plan_only);
    assert_eq!(exit, 0, "{hot_plan}");
    let plan_id = cold_plan["plan"]["plan_id"].clone();
    assert!(plan_id.is_string(), "{cold_plan}");
    assert_eq!(hot_plan["plan"]["plan_id"], plan_id);

    let hot_before = journals(&hot_root);
    let (hot_placed, exit) = place(&host, &hot_root, &zip);
    assert_eq!(exit, 0, "{hot_placed}");
    let (_, hot_journal) = new_journal(&hot_root, &hot_before);
    host.runtime.running.set(false);
    let cold_before = journals(&cold_root);
    let (cold_placed, exit) = place(&host, &cold_root, &zip);
    assert_eq!(exit, 0, "{cold_placed}");
    let (_, cold_journal) = new_journal(&cold_root, &cold_before);
    assert_eq!(hot_journal["plan_id"], plan_id);
    assert_eq!(cold_journal["plan_id"], plan_id);
    assert_eq!(cold_journal["plan"], hot_journal["plan"]);
    assert_eq!(step_kinds(&hot_journal)[0], "pause");
    assert_eq!(step_kinds(&cold_journal)[0], "pack_item");
}

/// Fix 1: the configuration of `root` binds each `(alias, game)` of `instances`, as rows of its
/// `policy.facts.instances` (placeholder server and host): the rows check-config and the Runtime's
/// scheduler workbench read the games in use from.
fn bind_in_configuration(root: &Path, instances: &[(&str, &str)]) {
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration")).expect("JSON");
    let rows = instances
        .iter()
        .map(|(alias, game)| {
            json!({
                "instance_id": alias,
                "game_id": game,
                "server_id": "neutral-server",
                "host_id": "neutral-host",
            })
        })
        .collect::<Vec<_>>();
    config["policy"] = json!({"facts": {"instances": rows}});
    write_json(&config_path, &config);
}

/// Fix 1: a note as check-config joins it to its `check_config_passed`, in the wire form: a
/// problem of the root of `game` (`standard_pack_root_invalid`, as check-config notes a broken
/// game root or a `schedule.json` the composition refuses).
fn note_naming(game: &str) -> Value {
    json!({
        "code": "standard_pack_root_invalid",
        "category": "error",
        "relation": "note",
        "values": {"operation": "standard_pack_read", "game": game},
    })
}

/// An actingd running under `root` with `pid` (a Runtime starting, before it holds its owner
/// lock).
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

/// The journal's `post_check` step.
fn check_step_of(journal: &Value) -> Value {
    journal["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .find(|step| step["kind"] == "post_check")
        .cloned()
        .unwrap_or_else(|| panic!("no post_check step: {journal}"))
}

/// Fix 1, as Workflow #454 closes the post-start checks: the `post_check` step of an operation
/// that ended `post_check_failed` is `done` with the evidence a passed check records (`game`,
/// `checks`, `failed`), exactly the answer's `checks`, its `failed` the rows `failed`.
fn assert_closed(journal: &Value, refused: &Value, failed: &[&str]) {
    let step = check_step_of(journal);
    assert_eq!(step["state"], "done", "{step}");
    assert_eq!(
        keys(&step["evidence"]),
        ["checks", "failed", "game"],
        "{step}"
    );
    assert_eq!(step["evidence"]["game"], GAME, "{step}");
    assert_eq!(step["evidence"]["failed"], json!(failed), "{step}");
    assert_eq!(step["evidence"], refused["checks"], "{step}");
}

/// The files of `game_root` but those an `undo` set aside (`.rolledback-<ms2>`).
fn restored(game_root: &Path) -> BTreeMap<String, Vec<u8>> {
    tree(game_root)
        .into_iter()
        .filter(|(path, _)| !path.contains(".rolledback-"))
        .collect()
}

// S6 fix 1, point 1: no Runtime runs; the configuration binds an instance to the game, and the
// live build's check-config passes with two notes, one naming another game and one naming the
// placed game. The row `check_config` fails: check-config's answer is `check_config_passed`, and
// `game_notes` holds the note of the placed game alone; `bound` names the instance and there is
// no `composition` member. The operation ends `post_check_failed` (`count` 1, the failed row
// `check_config`) with the items placed and `next` naming `undo` of the operation alone.
#[test]
fn s6_fix1_a_note_naming_the_placed_game_fails_check_config() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), "noted", &host);
    bind_in_configuration(&root, &[("neutral.a", GAME)]);
    host.programs
        .check_config_notes
        .borrow_mut()
        .extend([note_naming(OTHER_GAME), note_naming(GAME)]);
    let zip = temp.path().join("neutral-noted.zip");
    let new_control = br#"{"neutral": 2}"#;
    write_bundle(&zip, new_control, Some(SCHEDULE));

    let before = journals(&root);
    let (refused, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "post_check_failed",
        "{refused}"
    );
    assert_eq!(top_value(&refused, "count"), &json!(1), "{refused}");
    assert_eq!(
        refused["checks"]["failed"],
        json!(["check_config"]),
        "{refused}"
    );
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(
        journal["inputs"]["bound"],
        json!(["neutral.a"]),
        "{journal}"
    );
    let checked = check_row(&journal, "check_config");
    assert_eq!(checked["result"], "fail", "{checked}");
    assert_eq!(
        checked["values"]["answer"]["outcome"]["code"], "check_config_passed",
        "{checked}"
    );
    let noted = checked["values"]["game_notes"]
        .as_array()
        .expect("the notes naming the game");
    assert_eq!(noted.len(), 1, "{checked}");
    assert_eq!(
        noted[0]["outcome"]["code"], "standard_pack_root_invalid",
        "{checked}"
    );
    assert_eq!(noted[0]["outcome"]["values"]["game"], GAME, "{checked}");
    assert_eq!(
        checked["values"]["bound"],
        json!(["neutral.a"]),
        "{checked}"
    );
    assert!(checked["values"].get("composition").is_none(), "{checked}");
    assert_eq!(
        refused["next"],
        json!({"undo": ["undo", operation_id]}),
        "{refused}"
    );
    assert_eq!(
        fs::read(game_root.join(PACK).join("control.json")).expect("the placed pack"),
        new_control
    );
}

// S6 fix 1, point 1: no Runtime runs, and the configuration binds its one instance to another
// game. The input `bound` is empty; the row `check_config` passes with check-config's answer,
// no note of the game, `bound` `[]` and `composition` `not_previewed` (check-config did not
// preview the game's composition); the answer's `next` names the start, where the composition is
// judged (`composition_at_start`, the module's `up`).
#[test]
fn s6_fix1_no_bound_instance_says_the_composition_was_not_previewed() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "unbound-cold", &host);
    bind_in_configuration(&root, &[("neutral.c", OTHER_GAME)]);
    let zip = temp.path().join("neutral-unbound-cold.zip");
    write_bundle(&zip, CONTROL, Some(SCHEDULE));

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    assert_eq!(
        placed["next"],
        json!({"composition_at_start": ["up"]}),
        "{placed}"
    );
    let (_, journal) = new_journal(&root, &before);
    assert_eq!(journal["inputs"]["bound"], json!([]), "{journal}");
    let checked = check_row(&journal, "check_config");
    assert_eq!(checked["result"], "pass", "{checked}");
    assert_eq!(
        checked["values"]["composition"], "not_previewed",
        "{checked}"
    );
    assert_eq!(checked["values"]["bound"], json!([]), "{checked}");
    assert_eq!(checked["values"]["game_notes"], json!([]), "{checked}");
    assert_eq!(
        checked["values"]["answer"]["outcome"]["code"], "check_config_passed",
        "{checked}"
    );
}

// S6 fix 1, point 1: no Runtime runs, the configuration binds an instance to the game (and one to
// another game), and check-config passes with no note. The input `bound` names the game's
// instance alone; the row `check_config` passes with the members `answer`, `bound`, `game_notes`
// (empty), `install_root` and `packs`, and no `composition`; the operation is done.
#[test]
fn s6_fix1_a_bound_instance_and_no_note_passes_check_config() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "bound-cold", &host);
    bind_in_configuration(&root, &[("neutral.a", GAME), ("neutral.c", OTHER_GAME)]);
    let zip = temp.path().join("neutral-bound-cold.zip");
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
    let checked = check_row(&journal, "check_config");
    assert_eq!(checked["result"], "pass", "{checked}");
    assert_eq!(
        keys(&checked["values"]),
        ["answer", "bound", "game_notes", "install_root", "packs"],
        "{checked}"
    );
    assert_eq!(
        checked["values"]["bound"],
        json!(["neutral.a"]),
        "{checked}"
    );
    assert_eq!(checked["values"]["game_notes"], json!([]), "{checked}");
}

// S6 fix 1, point 5: no Runtime runs, and the live build's check-config is still running at its
// deadline (the fake leaves it running, pid 4242). The row `check_config` fails with
// `check_config_unanswered` (`install_command_refused` of the placement, the child's `pid` and the
// deadline's key); the operation ends `post_check_failed` (`count` 1) and `next` names `undo`.
#[test]
fn s6_fix1_a_check_config_that_does_not_answer_fails_its_row() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "stalled", &host);
    let zip = temp.path().join("neutral-stalled.zip");
    write_bundle(&zip, CONTROL, Some(SCHEDULE));
    host.programs.stall.borrow_mut().push("check-config");

    let before = journals(&root);
    let (refused, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "post_check_failed",
        "{refused}"
    );
    assert_eq!(top_value(&refused, "count"), &json!(1), "{refused}");
    assert_eq!(
        refused["checks"]["failed"],
        json!(["check_config"]),
        "{refused}"
    );
    let (operation_id, journal) = new_journal(&root, &before);
    let checked = check_row(&journal, "check_config");
    assert_eq!(checked["result"], "fail", "{checked}");
    let failure = &checked["values"]["failure"]["outcome"];
    assert_eq!(failure["code"], "install_command_refused", "{checked}");
    assert_eq!(
        failure["values"]["install_refusal"], "check_config_unanswered",
        "{checked}"
    );
    assert_eq!(
        failure["values"]["operation"], "install_pack_place",
        "{checked}"
    );
    assert_eq!(failure["values"]["pid"], json!(STALLED_PID), "{checked}");
    assert_eq!(
        failure["values"]["config_key"], "install.child.check_config_timeout_ms",
        "{checked}"
    );
    assert_eq!(
        refused["next"],
        json!({"undo": ["undo", operation_id]}),
        "{refused}"
    );
    assert!(journal["finished_at_unix_ms"].is_u64(), "{journal}");
}

// S6 fix 1, point 2: a placement begun while no Runtime ran ended `post_check_failed` (the live
// build's check-config refusing). Its `undo` takes the writer lock and applies the stopped rule
// again before it moves anything: an actingd under the root refuses `runtime_running` (`install
// undo`, its `pid`), the state root's owner lock held `owner_lock_held`; each leaves the game root
// and the journal as the placement left them. With the Runtime stopped again the `undo` is done
// and the game root is the one of before, the placed items set aside beside it.
#[test]
fn s6_fix1_undo_of_a_stopped_placement_refuses_a_running_runtime() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), "undone", &host);
    old_game_root(&game_root);
    let unchanged = tree(&game_root);
    let zip = temp.path().join("neutral-undone.zip");
    write_bundle(&zip, br#"{"neutral": 2}"#, Some(SCHEDULE));
    host.programs
        .refuse_check_config
        .borrow_mut()
        .push("live".to_owned());
    let before = journals(&root);
    let (refused, exit) = place(&host, &root, &zip);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "post_check_failed",
        "{refused}"
    );
    let (operation_id, _) = new_journal(&root, &before);
    let placed = tree(&game_root);
    let recorded = fs::read(journal_path(&root, &operation_id)).expect("the journal");
    let nothing_moved = || {
        assert_eq!(tree(&game_root), placed);
        assert_eq!(
            fs::read(journal_path(&root, &operation_id)).expect("the journal"),
            recorded
        );
    };
    let undo = json!({"op": operation_id});

    host.processes.replace(vec![actingd_image(&root, 41)]);
    let (undo_refused, exit) = rescue_line(&host, &root, "undo", undo.clone());
    assert_eq!(exit, 1, "{undo_refused}");
    assert_eq!(
        top_code(&undo_refused),
        "install_command_refused",
        "{undo_refused}"
    );
    assert_eq!(
        top_value(&undo_refused, "operation"),
        "install_undo",
        "{undo_refused}"
    );
    assert_eq!(
        top_value(&undo_refused, "install_refusal"),
        "runtime_running",
        "{undo_refused}"
    );
    assert_eq!(
        top_value(&undo_refused, "pid"),
        &json!(41),
        "{undo_refused}"
    );
    nothing_moved();
    host.processes.replace(Vec::new());

    let owner = hold_owner_lock(&temp.path().join("undone-state"));
    let (undo_refused, exit) = rescue_line(&host, &root, "undo", undo.clone());
    assert_eq!(exit, 1, "{undo_refused}");
    assert_eq!(
        top_value(&undo_refused, "install_refusal"),
        "owner_lock_held",
        "{undo_refused}"
    );
    drop(owner);
    nothing_moved();

    let (undone, exit) = rescue_line(&host, &root, "undo", undo);
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(top_code(&undone), "install_command_done", "{undone}");
    assert_eq!(restored(&game_root), unchanged);
    assert!(host.runtime.calls.borrow().is_empty());
}

// S6 fix 1, point 3: a placement begun while no Runtime ran stopped after its first item (the
// fake host's crash), `current.json` naming it. Its `op resume` takes the writer lock and applies
// the stopped rule again before any step runs: the state root's owner lock held refuses
// `owner_lock_held` (`install resume`), an actingd under the root `runtime_running` (its `pid`);
// each leaves the game root and the journal as the crash left them. With the Runtime stopped
// again the operation goes on and is done: every item placed, the checks passed, `current.json`
// retired.
#[test]
fn s6_fix1_resume_of_a_stopped_placement_refuses_a_running_runtime() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), "resumed", &host);
    old_game_root(&game_root);
    let zip = temp.path().join("neutral-resumed.zip");
    let new_control = br#"{"neutral": 2}"#;
    let new_bundle = write_bundle(&zip, new_control, Some(SCHEDULE));
    // The first item is step 1.
    host.crash_after.set(Some(1));
    let before = journals(&root);
    let (stopped, exit) = place(&host, &root, &zip);
    assert_ne!(exit, 0, "{stopped}");
    assert_eq!(
        top_code(&stopped),
        "install_operation_interrupted",
        "{stopped}"
    );
    let (operation_id, _) = new_journal(&root, &before);
    let halfway = tree(&game_root);
    let recorded = fs::read(journal_path(&root, &operation_id)).expect("the journal");
    let nothing_moved = || {
        assert_eq!(tree(&game_root), halfway);
        assert_eq!(
            fs::read(journal_path(&root, &operation_id)).expect("the journal"),
            recorded
        );
        assert!(root.join("install").join("current.json").exists());
    };
    let resume = json!({"op": operation_id});

    let owner = hold_owner_lock(&temp.path().join("resumed-state"));
    let (refused, exit) = rescue_line(&host, &root, "op resume", resume.clone());
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(
        top_value(&refused, "operation"),
        "install_resume",
        "{refused}"
    );
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "owner_lock_held",
        "{refused}"
    );
    drop(owner);
    nothing_moved();

    host.processes.replace(vec![actingd_image(&root, 41)]);
    let (refused, exit) = rescue_line(&host, &root, "op resume", resume.clone());
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "runtime_running",
        "{refused}"
    );
    assert_eq!(top_value(&refused, "pid"), &json!(41), "{refused}");
    nothing_moved();
    host.processes.replace(Vec::new());

    let (resumed, exit) = rescue_line(&host, &root, "op resume", resume);
    assert_eq!(exit, 0, "{resumed}");
    assert_eq!(
        fs::read(game_root.join(PACK).join("control.json")).expect("the placed pack"),
        new_control
    );
    assert_eq!(
        fs::read(game_root.join("bundle.json")).expect("the placed bundle.json"),
        new_bundle
    );
    assert_eq!(
        fs::read(game_root.join("schedule.json")).expect("the placed schedule.json"),
        SCHEDULE
    );
    assert!(!root.join("install").join("current.json").exists());
    let journal: Value =
        serde_json::from_slice(&fs::read(journal_path(&root, &operation_id)).expect("the journal"))
            .expect("journal JSON");
    assert_eq!(check_row(&journal, "check_config")["result"], "pass");
    assert!(host.runtime.calls.borrow().is_empty());
}

// S6 fix 1, points 2 and 3, the path that does not change: a placement begun with the Runtime
// running (the fake Runtime, whose actingd runs under the root) stopped after its first item; its
// `op resume` goes on while that actingd runs and is done, and its `undo` is done while it runs
// too, the game root again the one of before beside the placed items set aside.
#[test]
fn s6_fix1_undo_and_resume_of_a_running_placement_are_unchanged() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), "running", &host);
    old_game_root(&game_root);
    let unchanged = tree(&game_root);
    boot(&host, &[("neutral.a", GAME)]);
    let zip = temp.path().join("neutral-running.zip");
    write_bundle(&zip, br#"{"neutral": 2}"#, None);
    // The hold is step 1, the first item step 2.
    host.crash_after.set(Some(2));
    let before = journals(&root);
    let (stopped, exit) = place(&host, &root, &zip);
    assert_ne!(exit, 0, "{stopped}");
    assert_eq!(
        top_code(&stopped),
        "install_operation_interrupted",
        "{stopped}"
    );
    let (operation_id, journal) = new_journal(&root, &before);
    assert!(journal["inputs"].get("cold").is_none(), "{journal}");
    assert_eq!(step_kinds(&journal)[0], "pause");

    let (resumed, exit) = rescue_line(&host, &root, "op resume", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{resumed}");
    assert_eq!(top_code(&resumed), "install_command_done", "{resumed}");
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(top_code(&undone), "install_command_done", "{undone}");
    assert_eq!(restored(&game_root), unchanged);
}

// S6 fix 1, point 4 (as Workflow #454 closes the post-start checks, `gate_454_postcheck`): a
// failed check closes its `post_check` step `done` with what it read, in the write that ends the
// operation. With the Runtime running, a placed file changed after the last item fails `rerun`;
// with it stopped, the live build's check-config refusing fails `check_config`. Each operation
// ends `post_check_failed`, and its step is `done` with `game`, `checks` and `failed` (the failed
// row alone), exactly the answer's `checks`.
#[test]
fn s6_fix1_a_failed_check_closes_its_step_with_what_it_read() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (hot_root, hot_game_root) = prepared(temp.path(), "hot-failed", &host);
    let (cold_root, _) = prepared(temp.path(), "cold-failed", &host);
    let zip = temp.path().join("neutral-failed-check.zip");
    write_bundle(&zip, CONTROL, None);

    boot(&host, &[("neutral.a", GAME)]);
    let changed = hot_game_root.join(PACK).join("control.json");
    host.on_step.replace(Some(Box::new(move |step: usize| {
        // The hold is step 1, the three items steps 2 to 4.
        if step == 4 {
            fs::write(&changed, b"changed after the placement").expect("change a placed file");
        }
    })));
    let hot_before = journals(&hot_root);
    let (hot_refused, exit) = place(&host, &hot_root, &zip);
    host.on_step.replace(None);
    assert_eq!(exit, 1, "{hot_refused}");
    assert_eq!(
        top_value(&hot_refused, "install_refusal"),
        "post_check_failed",
        "{hot_refused}"
    );
    let (_, hot_journal) = new_journal(&hot_root, &hot_before);
    assert_eq!(step_kinds(&hot_journal)[0], "pause");
    assert_closed(&hot_journal, &hot_refused, &["rerun"]);

    host.runtime.running.set(false);
    host.programs
        .refuse_check_config
        .borrow_mut()
        .push("live".to_owned());
    let cold_before = journals(&cold_root);
    let (cold_refused, exit) = place(&host, &cold_root, &zip);
    assert_eq!(exit, 1, "{cold_refused}");
    assert_eq!(
        top_value(&cold_refused, "install_refusal"),
        "post_check_failed",
        "{cold_refused}"
    );
    let (_, cold_journal) = new_journal(&cold_root, &cold_before);
    assert_eq!(step_kinds(&cold_journal)[0], "pack_item");
    assert_closed(&cold_journal, &cold_refused, &["check_config"]);
}

// S6 fix 1, point 4: a check that passes keeps its record, with the Runtime running and with it
// stopped: the `post_check` step `done` with `game`, `checks` and `failed` (empty), the evidence
// the answer's `place` step carries as its `checks`.
#[test]
fn s6_fix1_a_passed_check_keeps_its_record() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (hot_root, _) = prepared(temp.path(), "hot-passed", &host);
    let (cold_root, _) = prepared(temp.path(), "cold-passed", &host);
    let zip = temp.path().join("neutral-passed-check.zip");
    write_bundle(&zip, CONTROL, None);

    boot(&host, &[("neutral.a", GAME)]);
    let hot_before = journals(&hot_root);
    let (hot_placed, exit) = place(&host, &hot_root, &zip);
    assert_eq!(exit, 0, "{hot_placed}");
    let (_, hot_journal) = new_journal(&hot_root, &hot_before);
    host.runtime.running.set(false);
    let cold_before = journals(&cold_root);
    let (cold_placed, exit) = place(&host, &cold_root, &zip);
    assert_eq!(exit, 0, "{cold_placed}");
    let (_, cold_journal) = new_journal(&cold_root, &cold_before);

    for (placed, journal) in [(&hot_placed, &hot_journal), (&cold_placed, &cold_journal)] {
        let step = check_step_of(journal);
        assert_eq!(step["state"], "done", "{step}");
        assert_eq!(
            keys(&step["evidence"]),
            ["checks", "failed", "game"],
            "{step}"
        );
        assert_eq!(step["evidence"]["failed"], json!([]), "{step}");
        assert_eq!(step["evidence"]["game"], GAME, "{step}");
        assert_eq!(placed["steps"][1]["step"], "place", "{placed}");
        assert_eq!(
            placed["steps"][1]["result"]["checks"], step["evidence"],
            "{placed}"
        );
    }
}
