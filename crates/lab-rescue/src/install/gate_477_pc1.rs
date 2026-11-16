// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 PC-1 (register §1.438; contracts/pack-catalog.md, "Build and place"; coordinator
// work order `rd-1116/brief-477-pc1-pack-catalog.md`, the tests of build and place): the rescue
// module's `place` takes a bundle zip's root file `pack-catalog.json` as the last root file, with
// its digest in the plan, places it byte for byte and sets it aside when a later bundle carries
// none; a game root that still holds `ui.json`, which no program reads any more, has it set aside
// by the placement (kept beside, `undo` puts it back); a bundle whose catalog is wrong in its
// business part is refused `pack_invalid` caused by `standard_pack_root_invalid` at its pointer,
// before anything is held or written; fix round 1: so is a bundle that still carries `ui.json`,
// `pack_invalid` naming that member.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files.

use super::files;
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
/// The neutral game and pack of the bundles (made-up ids, no real game).
const GAME: &str = "neutral-game";
const PACK: &str = "neutral.pack";
/// The `package` range of the installer these tests run, and what each bundle declares.
const INSTALLER_PACKAGE: [u64; 2] = [3, 5];
const BUNDLE_PACKAGE: [u64; 2] = [3, 5];
const CONTROL: &[u8] = br#"{"neutral": 1}"#;
/// A `ui.json` an earlier build placed.
const RETIRED_UI: &[u8] = br#"{"schema_version": "actingcommand.ui.v1"}"#;

/// The bytes of a `pack-catalog.json` of [`GAME`] whose one entry names `package_id`, with a
/// display and an empty business part (the bundle carries no `schedule.json`, so no pool).
fn catalog_bytes(package_id: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.pack-catalog.v1",
        "game": GAME,
        "groups": [{"id": "daily", "title": {"en": "Daily"}}],
        "packs": [{
            "package_id": package_id,
            "title": {"en": "Neutral"},
            "summary": {"en": "A neutral pack."},
            "group": "daily",
            "consumes": [],
            "produces": []
        }]
    }))
    .expect("JSON")
}

/// One neutral bundle zip of [`GAME`]: the pack [`PACK`] and its `pack-catalog.json` when it
/// carries one; fix round 1: and a `ui.json` when it carries one, as a zip built before 0.12.68
/// would.
struct Bundle {
    catalog: Option<Vec<u8>>,
    ui: Option<&'static [u8]>,
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
        declared["package"] = json!(BUNDLE_PACKAGE);
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
        if let Some(catalog) = &self.catalog {
            entries.push(("pack-catalog.json".to_owned(), catalog.clone()));
        }
        if let Some(ui) = self.ui {
            entries.push(("ui.json".to_owned(), ui.to_vec()));
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

fn top_code(line: &Value) -> &str {
    line["outcome"]["code"].as_str().unwrap_or_default()
}

/// The values of the first outcome with `code` anywhere in `line`.
fn values_of<'a>(line: &'a Value, code: &str) -> Option<&'a Value> {
    match line {
        Value::Object(members) => {
            if members.get("code").and_then(Value::as_str) == Some(code) {
                return members.get("values");
            }
            members.values().find_map(|inner| values_of(inner, code))
        }
        Value::Array(items) => items.iter().find_map(|inner| values_of(inner, code)),
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

/// The one journal written since `before`: its operation id and the journal.
fn new_journal(root: &Path, before: &[(String, Value)]) -> (String, Value) {
    let mut written = journals(root)
        .into_iter()
        .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
        .collect::<Vec<_>>();
    assert_eq!(written.len(), 1, "{written:?}");
    written.remove(0)
}

/// The `item` of each `pack_item` step's intent of a journal, in order.
fn touched(journal: &Value) -> Vec<String> {
    journal["steps"]
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

/// A root whose configuration names the standard-pack root `<base>\pc1-packs`, adopted, with the
/// empty game root of [`GAME`] in it; the installer declares [`INSTALLER_PACKAGE`], and the fake
/// Runtime runs one instance `neutral.a` bound to [`GAME`]. The root and the game root.
fn prepared(base: &Path, host: &FakeHost) -> (PathBuf, PathBuf) {
    let root = live_root(base, "pc1", &BuildSpec::new(C1));
    let packs = base.join("pc1-packs");
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
    host.runtime.boot(&["neutral.a"]);
    for instance in host.runtime.instances.borrow_mut().iter_mut() {
        instance.game = Some(GAME.to_owned());
    }
    (root, game_root)
}

// A bundle that carries `pack-catalog.json`, on a game root that still holds the `ui.json` an
// earlier build placed: planned with `ui.json` `retire` before the root files it places and
// `pack-catalog.json` `new`, the last root file; placed, the catalog on disk the zip's bytes and its
// item's digest in the journal's plan the SHA-256 of those bytes, `ui.json` set aside as
// `ui.json.old-<ms>` and kept. A later bundle without the catalog plans it `retire` and sets it
// aside; `undo` puts it back.
#[test]
fn pc1_the_catalog_is_placed_with_its_digest_and_retired_and_an_installed_ui_json_is_set_aside() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    fs::write(game_root.join("ui.json"), RETIRED_UI).expect("an installed ui.json");
    let catalog = catalog_bytes(PACK);
    let first = temp.path().join("neutral-catalog.zip");
    Bundle {
        catalog: Some(catalog.clone()),
        ui: None,
    }
    .write(&first);

    let (planned, exit) = place(&host, &root, &first, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral.pack pack new",
            "ui.json root retire",
            "bundle.json root new",
            "pack-catalog.json root new"
        ],
        "{planned}"
    );
    assert!(calls_of(&host, "pause ").is_empty());

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &first, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let (_, journal) = new_journal(&root, &before);
    assert_eq!(
        touched(&journal),
        [PACK, "ui.json", "bundle.json", "pack-catalog.json"]
    );
    let item = journal["plan"]["items"]
        .as_array()
        .expect("the plan's items")
        .iter()
        .find(|item| item["item"] == "pack-catalog.json")
        .cloned()
        .expect("the catalog's item");
    assert_eq!(item["kind"], "root", "{item}");
    assert_eq!(
        item["digest"],
        json!(files::sha256_bytes(&catalog)),
        "{item}"
    );
    assert_eq!(
        fs::read(game_root.join("pack-catalog.json")).expect("the placed catalog"),
        catalog
    );
    assert!(!game_root.join("ui.json").exists());
    assert_eq!(
        fs::read(kept(&game_root, "ui.json.old-")).expect("the kept ui.json"),
        RETIRED_UI
    );

    let later = temp.path().join("neutral-plain.zip");
    Bundle {
        catalog: None,
        ui: None,
    }
    .write(&later);
    let (planned, exit) = place(&host, &root, &later, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral.pack pack unchanged",
            "pack-catalog.json root retire",
            "bundle.json root unchanged"
        ],
        "{planned}"
    );
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &later, false);
    assert_eq!(exit, 0, "{placed}");
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(touched(&journal), ["pack-catalog.json"]);
    assert!(!game_root.join("pack-catalog.json").exists());
    assert_eq!(
        fs::read(kept(&game_root, "pack-catalog.json.old-")).expect("the kept catalog"),
        catalog
    );

    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(
        fs::read(game_root.join("pack-catalog.json")).expect("the catalog put back"),
        catalog
    );
}

// A bundle whose `pack-catalog.json` is wrong in its business part (its one entry names a package
// the bundle does not register) is refused in the plan, `pack_invalid` caused by
// `standard_pack_root_invalid` with `package_not_listed` at the entry's `package_id`, `path` the
// zip's member: no pause, no journal, nothing in the game root, as a root file every reader
// refuses is.
#[test]
fn pc1_a_bundle_whose_catalog_is_wrong_in_its_business_part_is_refused_before_the_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let before = journals(&root);
    let zip = temp.path().join("neutral-wrong.zip");
    Bundle {
        catalog: Some(catalog_bytes("neutral.absent")),
        ui: None,
    }
    .write(&zip);

    let (refused, exit) = place(&host, &root, &zip, false);

    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(
        refused["outcome"]["values"]["install_refusal"],
        "pack_invalid"
    );
    let cause = values_of(&refused, "standard_pack_root_invalid")
        .unwrap_or_else(|| panic!("no standard_pack_root_invalid in {refused}"));
    let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
    assert_eq!(
        cause["standard_pack_problem"], "package_not_listed",
        "{cause}"
    );
    assert_eq!(cause["json_pointer"], "/packs/0/package_id", "{cause}");
    assert_eq!(
        cause["path"],
        json!(canonical.join("pack-catalog.json").to_string_lossy()),
        "{cause}"
    );
    assert_eq!(cause["game"], GAME, "{cause}");
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

// Fix round 1, the review's missing test: a bundle zip that still carries `ui.json` (as one built
// before 0.12.68 would), which no layout knows any more, is refused in the plan, `pack_invalid`
// with `path` the zip's member `ui.json`: no pause, no journal, nothing in the game root.
#[test]
fn pc1_a_bundle_that_carries_ui_json_is_refused_in_the_plan_before_the_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let before = journals(&root);
    let zip = temp.path().join("neutral-ui.zip");
    Bundle {
        catalog: None,
        ui: Some(RETIRED_UI),
    }
    .write(&zip);

    let (refused, exit) = place(&host, &root, &zip, false);

    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(
        refused["outcome"]["values"]["install_refusal"],
        "pack_invalid"
    );
    let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
    assert_eq!(
        refused["outcome"]["values"]["path"],
        json!(canonical.join("ui.json").to_string_lossy()),
        "{refused}"
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
