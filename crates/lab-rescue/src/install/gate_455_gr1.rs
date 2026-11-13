// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R1 (register §1.350; contracts/standard-pack-goals.md, "Who reads it";
// contracts/component-interfaces.md, the `package` row 12; coordinator work order
// `rd-1114/brief-455-gr1.md`): the rescue module's `place` takes a bundle zip's root file
// `goals.json` as the last root file and sets it aside, as the other root files the readers open,
// when a later bundle carries none; a bundle that carries it declares `package` [12, 12], which an
// installer whose range ends below 12 refuses at its edge with nothing held or written.
// - `goals_file_is_placed_last_and_retired_when_a_later_bundle_carries_none`: a bundle that
//   carries `goals.json` and declares [12, 12] is planned with the item `goals.json` `new` after
//   `bundle.json`, placed (the file on disk the zip's bytes, the journal's plan declaring
//   [12, 12]); a later bundle without it plans the item `retire` before the root files it places,
//   sets it aside as `goals.json.old-<ms>` and keeps it; `undo` puts it back;
// - `goals_bundle_is_refused_by_an_installer_before_revision_12`: on an installer whose `package`
//   range is [3, 11], the same bundle is refused `interface_edge` at its `interfaces.json` before
//   the hold: no pause, no journal, nothing in the game root.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files. `place` reads the file's name only;
// what it declares is `standard-pack check`'s, which the fixture's root does not carry (its row is
// `not_evaluated`).

use super::fixture::{BuildSpec, C1, FakeHost, code, live_root, run, speaks, word, write_json};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_contract::GOALS_PACKAGE_REVISION;
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The initiator the tests run the module as.
const WORD: Initiator = Initiator::LabRescue;
/// The neutral game and pack of the bundles (made-up ids, no real game).
const GAME: &str = "neutral-game";
const PACK: &str = "neutral.pack";
/// The `package` range of this build's installer: up to the revision a goals bundle declares.
const INSTALLER_PACKAGE: [u64; 2] = [3, GOALS_PACKAGE_REVISION];
/// The `package` range of an installer before this slice.
const EARLIER_INSTALLER_PACKAGE: [u64; 2] = [3, 11];
/// What a bundle that carries `goals.json` declares.
const GOALS_BUNDLE_PACKAGE: [u64; 2] = [GOALS_PACKAGE_REVISION, GOALS_PACKAGE_REVISION];
/// What a bundle without it declares: [`speaks`]'s.
const PLAIN_BUNDLE_PACKAGE: [u64; 2] = [3, 5];
const CONTROL: &[u8] = br#"{"neutral": 1}"#;

/// The bytes of a `goals.json` of [`GAME`]: one made-up heading.
fn goals_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.goals.v1",
        "game": GAME,
        "entries": [{"name": "stock", "type": "heading", "title": {"en": "Stock"}}]
    }))
    .expect("JSON")
}

/// One neutral bundle zip of [`GAME`]: the pack [`PACK`], its `goals.json` when it carries one,
/// and the `package` range its `interfaces.json` declares.
struct Bundle {
    goals: Option<Vec<u8>>,
    package: [u64; 2],
}

impl Bundle {
    /// A bundle that carries [`goals_bytes`] and declares [`GOALS_BUNDLE_PACKAGE`].
    fn carrying_goals() -> Self {
        Self {
            goals: Some(goals_bytes()),
            package: GOALS_BUNDLE_PACKAGE,
        }
    }

    /// A bundle without `goals.json` that declares [`PLAIN_BUNDLE_PACKAGE`].
    fn plain() -> Self {
        Self {
            goals: None,
            package: PLAIN_BUNDLE_PACKAGE,
        }
    }

    /// The bytes of its `bundle.json`.
    fn index() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": [{
                "package_id": PACK,
                "server": "neutral-server",
                "entry_task_id": "neutral.entry",
                "path": format!("packs/{PACK}"),
                "file_count": 1,
                "byte_count": CONTROL.len(),
            }],
        }))
        .expect("JSON")
    }

    /// Writes the zip to `path`, every member stored.
    fn write(&self, path: &Path) {
        let mut declared = speaks();
        declared["package"] = json!(self.package);
        let interfaces = serde_json::to_vec(&json!({
            "schema_version": "actingcommand.component-interfaces.v1",
            "speaks": declared,
        }))
        .expect("JSON");
        let mut entries = vec![
            ("bundle.json".to_owned(), Self::index()),
            ("interfaces.json".to_owned(), interfaces),
            (format!("packs/{PACK}/control.json"), CONTROL.to_vec()),
        ];
        if let Some(goals) = &self.goals {
            entries.push(("goals.json".to_owned(), goals.clone()));
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

/// `place` of the bundle `zip` on `root`, only its plan when `plan_only`.
fn place(host: &FakeHost, root: &Path, zip: &Path, plan_only: bool) -> (Value, u8) {
    let params = if plan_only {
        json!({"bundle": word(zip), "plan": true})
    } else {
        json!({"bundle": word(zip)})
    };
    rescue_line(host, root, "place", params)
}

fn top_code(line: &Value) -> &str {
    line["outcome"]["code"].as_str().unwrap_or_default()
}

fn top_value<'a>(line: &'a Value, key: &str) -> &'a Value {
    &line["outcome"]["values"][key]
}

/// The items of a `place` line's plan, each `<item> <kind> <state>`, in the plan's order.
fn items(line: &Value) -> Vec<String> {
    line["plan"]["items"]
        .as_array()
        .expect("the plan's items")
        .iter()
        .map(|item| {
            format!(
                "{} {} {}",
                item["item"].as_str().unwrap_or_default(),
                item["kind"].as_str().unwrap_or_default(),
                item["state"].as_str().unwrap_or_default()
            )
        })
        .collect()
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

/// The intents of a journal's `pack_item` steps, in order.
fn item_intents(journal: &Value) -> Vec<Value> {
    journal["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .filter(|step| step["kind"] == "pack_item")
        .map(|step| step["intent"].clone())
        .collect()
}

/// The `item` of each intent, in order.
fn touched(journal: &Value) -> Vec<String> {
    item_intents(journal)
        .iter()
        .map(|intent| intent["item"].as_str().unwrap_or_default().to_owned())
        .collect()
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

/// A root whose configuration names the standard-pack root `<base>\gr1-packs`, adopted, with the
/// empty game root of [`GAME`] in it; the installer declares `installer`, and the fake Runtime
/// runs one instance `neutral.a` bound to [`GAME`]. The root and the game root.
fn prepared(base: &Path, host: &FakeHost, installer: [u64; 2]) -> (PathBuf, PathBuf) {
    let root = live_root(base, "gr1", &BuildSpec::new(C1));
    let packs = base.join("gr1-packs");
    let game_root = packs.join(GAME);
    fs::create_dir_all(&game_root).expect("the game root");
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration")).expect("JSON");
    config["standard_packs"] = json!({"root": packs});
    write_json(&config_path, &config);
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    host.installer_package.set(Some(installer));
    host.runtime.boot(&["neutral.a"]);
    for instance in host.runtime.instances.borrow_mut().iter_mut() {
        instance.game = Some(GAME.to_owned());
    }
    (root, game_root)
}

// A bundle that carries `goals.json` and declares [12, 12]: planned with `goals.json` `new`, the
// last root file; placed byte for byte, the journal's plan declaring [12, 12]. A later bundle
// without it: `goals.json` `retire` before the root files it places, set aside as
// `goals.json.old-<ms>` and kept; `undo` puts it back.
#[test]
fn goals_file_is_placed_last_and_retired_when_a_later_bundle_carries_none() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host, INSTALLER_PACKAGE);
    let first = temp.path().join("neutral-goals.zip");
    Bundle::carrying_goals().write(&first);

    // The plan: within this installer's range, the file new and the last root file.
    let (planned, exit) = place(&host, &root, &first, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral.pack pack new",
            "bundle.json root new",
            "goals.json root new"
        ],
        "{planned}"
    );
    assert!(calls_of(&host, "pause ").is_empty());

    // The placement: the file's step the last, the file on disk the zip's bytes.
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &first, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let (_, journal) = new_journal(&root, &before);
    assert_eq!(journal["kind"], "pack_place");
    assert_eq!(journal["plan"]["package"], json!(GOALS_BUNDLE_PACKAGE));
    assert_eq!(touched(&journal), [PACK, "bundle.json", "goals.json"]);
    let intent = &item_intents(&journal)[2];
    assert_eq!(intent["kind"], "root", "{intent}");
    assert_eq!(intent["state"], "new", "{intent}");
    assert_eq!(
        fs::read(game_root.join("goals.json")).expect("the placed goals.json"),
        goals_bytes()
    );

    // A later bundle without the file: planned `retire` before the root files it places.
    let later = temp.path().join("neutral-plain.zip");
    Bundle::plain().write(&later);
    let (planned, exit) = place(&host, &root, &later, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral.pack pack unchanged",
            "goals.json root retire",
            "bundle.json root unchanged"
        ],
        "{planned}"
    );

    // Placed: set aside as `goals.json.old-<ms>` and kept, its bytes unchanged.
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &later, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(touched(&journal), ["goals.json"]);
    let retired = &item_intents(&journal)[0];
    assert_eq!(retired["kind"], "root", "{retired}");
    assert_eq!(retired["state"], "retire", "{retired}");
    assert!(!game_root.join("goals.json").exists());
    assert_eq!(
        fs::read(kept(&game_root, "goals.json.old-")).expect("the kept goals.json"),
        goals_bytes()
    );

    // `undo` puts it back.
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(
        fs::read(game_root.join("goals.json")).expect("goals.json put back"),
        goals_bytes()
    );
}

// The `package` edge, as `notices_edge_is_checked_before_the_layout` holds it: an installer whose
// range ends at 11 refuses a bundle that carries `goals.json` and declares [12, 12]
// `interface_edge` at its `interfaces.json`, in the plan; nothing is held or written.
#[test]
fn goals_bundle_is_refused_by_an_installer_before_revision_12() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host, EARLIER_INSTALLER_PACKAGE);
    let before = journals(&root);

    let zip = temp.path().join("neutral-goals.zip");
    Bundle::carrying_goals().write(&zip);
    let (refused, exit) = place(&host, &root, &zip, false);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(top_value(&refused, "install_refusal"), "interface_edge");
    let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
    assert_eq!(
        top_value(&refused, "path"),
        &json!(canonical.join("interfaces.json").to_string_lossy())
    );

    assert!(
        calls_of(&host, "pause ").is_empty(),
        "{:?}",
        host.runtime.calls.borrow()
    );
    assert_eq!(journals(&root).len(), before.len());
    assert!(
        fs::read_dir(&game_root)
            .expect("the game root")
            .next()
            .is_none(),
        "the game root holds nothing"
    );
}
