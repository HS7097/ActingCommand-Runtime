// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR1 (register §1.535; contracts/standard-pack-goal-tables.md, "Who reads it";
// contracts/component-interfaces.md, the `package` row 28; coordinator work order
// `rd-1123/brief-455-rr1.md`): the rescue module's `place` takes a bundle zip's root file
// `goal-tables.json` as a root file, placed after `schedule.json` and before `goals.json` (whose
// records entries name its tables), and sets it aside, as the other root files the readers open,
// when a later bundle carries none; a bundle that carries it, or `goals.json` of version 3,
// declares `package` [28, 28], which an installer whose range ends below 28 refuses at its edge
// and which this build's installer asks of the bundle (`goal_tables_undeclared`,
// `goal_undeclared`), with nothing held or written.
// - `rr1_goal_tables_file_is_placed_before_goals_and_retired_when_a_later_bundle_carries_none`;
// - `rr1_a_bundle_below_28_with_either_file_is_refused_in_the_plan`.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files. `place` reads the files' names only;
// what they declare is `standard-pack check`'s, which the fixture's root does not carry.

use super::fixture::{BuildSpec, C1, FakeHost, code, live_root, run, speaks, word, write_json};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_contract::GOAL_TABLES_PACKAGE_REVISION;
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
/// The `package` range of the installer these tests run: up to the revision a bundle that
/// carries either file declares, this build's.
const INSTALLER_PACKAGE: [u64; 2] = [3, GOAL_TABLES_PACKAGE_REVISION];
/// The `package` range of an installer before this slice: up to the revision just below.
const EARLIER_INSTALLER_PACKAGE: [u64; 2] = [3, GOAL_TABLES_PACKAGE_REVISION - 1];
/// What a bundle that carries either file declares.
const TABLES_BUNDLE_PACKAGE: [u64; 2] =
    [GOAL_TABLES_PACKAGE_REVISION, GOAL_TABLES_PACKAGE_REVISION];
/// What a bundle without them declares: [`speaks`]'s.
const PLAIN_BUNDLE_PACKAGE: [u64; 2] = [3, 5];
const CONTROL: &[u8] = br#"{"neutral": 1}"#;

/// The bytes of a `goal-tables.json` of [`GAME`]: one made-up table. `place` reads its name only.
fn tables_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": GAME,
        "items": {"coin": {}},
        "tables": [{
            "name": "heroes",
            "progress": [{"fact_key": "inventory.list.neutral_heroes", "key_column": "hero",
                          "columns": ["grade"]}],
            "columns": [{"name": "grade", "read": ["grade"], "per": "class"}],
            "classes": {"common": {"grade": {"positions": [[0], [1]], "steps": [{"coin": 1}]}}},
            "keys": {"hero_a": {"class": "common"}}
        }]
    }))
    .expect("JSON")
}

/// The bytes of a `goals.json` of [`GAME`], version 3: one records entry on the table.
fn goals_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.goals.v3",
        "game": GAME,
        "entries": [{"name": "raise", "type": "records", "table": "heroes", "max_records": 4,
                     "title": {"en": "Whom to raise"}}]
    }))
    .expect("JSON")
}

/// One neutral bundle zip of [`GAME`]: the pack [`PACK`], its `goals.json` and its
/// `goal-tables.json` when it carries them, and the `package` range its `interfaces.json`
/// declares.
struct Bundle {
    goals: Option<Vec<u8>>,
    tables: Option<Vec<u8>>,
    package: [u64; 2],
}

impl Bundle {
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
        if let Some(tables) = &self.tables {
            entries.push(("goal-tables.json".to_owned(), tables.clone()));
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

/// The rescue row `name` with `params` on `root`, as [`WORD`]; its one line and exit code.
fn rescue_line(host: &FakeHost, root: &Path, name: &str, params: Value) -> (Value, u8) {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    let location = rescue::location_of(root, &config, &state_root);
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

/// `place` of the bundle `zip` on `root`, only its plan when `plan_only`; a placement with the
/// authorization it names (`fixture::authorized`).
fn place(host: &FakeHost, root: &Path, zip: &Path, plan_only: bool) -> (Value, u8) {
    let params = if plan_only {
        json!({"bundle": word(zip), "plan": true})
    } else {
        super::fixture::authorized(host, root, zip, json!({"bundle": word(zip)}))
    };
    rescue_line(host, root, "place", params)
}

/// The values of the first outcome with `code` anywhere in `line` (an outcome or one of its
/// causes).
fn values_of<'a>(line: &'a Value, wanted: &str) -> Option<&'a Value> {
    match line {
        Value::Object(members) => {
            if members.get("code").and_then(Value::as_str) == Some(wanted) {
                return members.get("values");
            }
            members.values().find_map(|inner| values_of(inner, wanted))
        }
        Value::Array(listed) => listed.iter().find_map(|inner| values_of(inner, wanted)),
        _ => None,
    }
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

/// The `item` of each `pack_item` step of the one journal written since `before`, in order.
fn touched_since(root: &Path, before: &[(String, Value)]) -> Vec<String> {
    let written = journals(root)
        .into_iter()
        .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
        .collect::<Vec<_>>();
    assert_eq!(written.len(), 1, "{written:?}");
    written[0].1["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .filter(|step| step["kind"] == "pack_item")
        .map(|step| {
            step["intent"]["item"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
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

/// A root whose configuration names the standard-pack root `<base>\rr1-packs`, adopted, with the
/// empty game root of [`GAME`] in it; the installer declares `installer`, and the fake Runtime
/// runs one instance `neutral.a` bound to [`GAME`]. The root and the game root.
fn prepared(base: &Path, host: &FakeHost, installer: [u64; 2]) -> (PathBuf, PathBuf) {
    let root = live_root(base, "rr1", &BuildSpec::new(C1));
    let packs = base.join("rr1-packs");
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

/// Whether the game root holds nothing.
fn empty(game_root: &Path) -> bool {
    fs::read_dir(game_root)
        .expect("the game root")
        .next()
        .is_none()
}

// A bundle that carries `goal-tables.json` and `goals.json` and declares [28, 28]: planned with
// both files `new`, `goal-tables.json` before `goals.json`; placed byte for byte, the journal's
// plan declaring what the bundle declares. A later bundle without them: `goals.json` and then
// `goal-tables.json` `retire` before the root files it places, each set aside as
// `<file>.old-<ms>` and kept; `undo` puts both back.
#[test]
fn rr1_goal_tables_file_is_placed_before_goals_and_retired_when_a_later_bundle_carries_none() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host, INSTALLER_PACKAGE);
    let first = temp.path().join("neutral-tables.zip");
    Bundle {
        goals: Some(goals_bytes()),
        tables: Some(tables_bytes()),
        package: TABLES_BUNDLE_PACKAGE,
    }
    .write(&first);

    let (planned, exit) = place(&host, &root, &first, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral.pack pack new",
            "bundle.json root new",
            "goal-tables.json root new",
            "goals.json root new"
        ],
        "{planned}"
    );
    assert!(calls_of(&host, "pause ").is_empty());

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &first, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(
        placed["outcome"]["code"], "install_command_done",
        "{placed}"
    );
    assert_eq!(
        touched_since(&root, &before),
        [PACK, "bundle.json", "goal-tables.json", "goals.json"]
    );
    let placed_journal = journals(&root)
        .into_iter()
        .find(|(id, _)| !before.iter().any(|(known, _)| known == id))
        .map(|(_, journal)| journal)
        .expect("the placement's journal");
    assert_eq!(
        placed_journal["plan"]["package"],
        json!(TABLES_BUNDLE_PACKAGE)
    );
    assert_eq!(
        fs::read(game_root.join("goal-tables.json")).expect("the placed goal-tables.json"),
        tables_bytes()
    );
    assert_eq!(
        fs::read(game_root.join("goals.json")).expect("the placed goals.json"),
        goals_bytes()
    );

    let later = temp.path().join("neutral-plain.zip");
    Bundle {
        goals: None,
        tables: None,
        package: PLAIN_BUNDLE_PACKAGE,
    }
    .write(&later);
    let (planned, exit) = place(&host, &root, &later, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral.pack pack unchanged",
            "goals.json root retire",
            "goal-tables.json root retire",
            "bundle.json root unchanged"
        ],
        "{planned}"
    );

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &later, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(
        placed["outcome"]["code"], "install_command_done",
        "{placed}"
    );
    assert_eq!(
        touched_since(&root, &before),
        ["goals.json", "goal-tables.json"]
    );
    let operation_id = journals(&root)
        .into_iter()
        .map(|(id, _)| id)
        .find(|id| !before.iter().any(|(known, _)| known == id))
        .expect("the retirement's operation");
    assert!(!game_root.join("goal-tables.json").exists());
    assert_eq!(
        fs::read(kept(&game_root, "goal-tables.json.old-")).expect("the kept goal-tables.json"),
        tables_bytes()
    );
    assert_eq!(
        fs::read(kept(&game_root, "goals.json.old-")).expect("the kept goals.json"),
        goals_bytes()
    );

    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(
        fs::read(game_root.join("goal-tables.json")).expect("goal-tables.json put back"),
        tables_bytes()
    );
}

// The `package` edge and the revision a bundle declares: an installer whose range ends at 27
// refuses a bundle that carries `goal-tables.json` and declares [28, 28] `interface_edge` at its
// `interfaces.json`, in the plan; this build's installer refuses a bundle that carries it and
// declares [3, 27], within its range, `pack_invalid` caused by `standard_pack_root_invalid` with
// `goal_tables_undeclared` at its `interfaces.json`, the pointer empty, and one that carries
// `goals.json` of version 3 and declares [14, 14], what a bundle of version 2 declared,
// `goal_undeclared`. Each in the plan: no pause, no journal, nothing in the game root.
#[test]
fn rr1_a_bundle_below_28_with_either_file_is_refused_in_the_plan() {
    let temp = TempDir::new().expect("tempdir");
    let earlier_host = FakeHost::new(&temp.path().join("earlier-work"));
    let earlier = temp.path().join("earlier");
    fs::create_dir_all(&earlier).expect("the earlier base");
    let (root, game_root) = prepared(&earlier, &earlier_host, EARLIER_INSTALLER_PACKAGE);
    let before = journals(&root);
    let zip = temp.path().join("neutral-tables.zip");
    Bundle {
        goals: None,
        tables: Some(tables_bytes()),
        package: TABLES_BUNDLE_PACKAGE,
    }
    .write(&zip);
    let (refused, exit) = place(&earlier_host, &root, &zip, false);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        refused["outcome"]["code"], "install_command_refused",
        "{refused}"
    );
    assert_eq!(
        refused["outcome"]["values"]["install_refusal"], "interface_edge",
        "{refused}"
    );
    let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
    assert_eq!(
        refused["outcome"]["values"]["path"],
        json!(canonical.join("interfaces.json").to_string_lossy())
    );
    assert!(calls_of(&earlier_host, "pause ").is_empty());
    assert_eq!(journals(&root).len(), before.len());
    assert!(empty(&game_root), "the game root holds nothing");

    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host, INSTALLER_PACKAGE);
    let before = journals(&root);
    for (name, bundle, word_expected) in [
        (
            "neutral-tables-undeclared.zip",
            Bundle {
                goals: None,
                tables: Some(tables_bytes()),
                package: [3, GOAL_TABLES_PACKAGE_REVISION - 1],
            },
            "goal_tables_undeclared",
        ),
        (
            "neutral-goals-at-14.zip",
            Bundle {
                goals: Some(goals_bytes()),
                tables: None,
                package: [14, 14],
            },
            "goal_undeclared",
        ),
    ] {
        let zip = temp.path().join(name);
        bundle.write(&zip);
        let (refused, exit) = place(&host, &root, &zip, false);
        assert_eq!(exit, 1, "{refused}");
        assert_eq!(
            refused["outcome"]["values"]["install_refusal"], "pack_invalid",
            "{refused}"
        );
        let cause = values_of(&refused, "standard_pack_root_invalid")
            .unwrap_or_else(|| panic!("no standard_pack_root_invalid in {refused}"));
        let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
        assert_eq!(cause["standard_pack_problem"], word_expected, "{cause}");
        assert_eq!(
            cause["path"],
            json!(canonical.join("interfaces.json").to_string_lossy()),
            "{cause}"
        );
        assert_eq!(cause["json_pointer"], "", "{cause}");
        assert_eq!(cause["game"], GAME, "{cause}");
    }
    assert!(calls_of(&host, "pause ").is_empty());
    assert_eq!(journals(&root).len(), before.len());
    assert!(empty(&game_root), "the game root holds nothing");
}
