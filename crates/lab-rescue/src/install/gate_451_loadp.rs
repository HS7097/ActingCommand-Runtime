// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #451 LOAD-P (register §1.338; contracts/install-cli.md, "The Lab's rescue module";
// contracts/package-reference.md, "Standard package root", `loading.json`; coordinator work order
// `rd-1113/brief-451-loadp.md`): the rescue module's `place` takes a bundle zip's loading templates
// directory `loading/`, exactly the templates the zip's own `loading.json` declares, as one item of
// kind `loading`, placed after the packs and before the root files, held to the declaration before
// anything is held or written, and set aside when a later bundle carries neither the file nor the
// directory.
// - `loading_templates_are_placed_after_the_packs_and_read_back_unchanged`: a bundle that carries
//   `loading.json` and its templates is planned with the item `new` after the packs and before the
//   root files, placed (the files on disk are the zip's bytes), planned again with every item
//   `unchanged`, and undone (the placed directory set aside);
// - `loading_templates_replace_unchanged_retire_and_undo`: a changed template replaces the
//   directory, an unchanged one is left while another item changes, a bundle without the file and
//   the directory retires both (set aside, the plan showing the items); each `undo` puts back what
//   was there;
// - `loading_absent_bundle_plans_as_before`: a bundle without the directory on a root without it
//   plans, journals and places as before LOAD-P: the plan document holds exactly the members and
//   items the earlier code writes, its id is the earlier formula over it, and the steps are the
//   same;
// - `loading_refusals_come_before_the_hold`: a member the file does not declare, a deeper path, a
//   member of a zip without `loading.json` (each `pack_invalid` naming the member), a declared
//   template the zip lacks (`loading_template_invalid`) and a pack named like the directory in two
//   cases (`loading_pack_name`), each before the hold with nothing written.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files. A template here is placeholder bytes:
// `place` reads the declaration and the names only, and what a template shows is `standard-pack
// check`'s, which the fixture's root does not carry (its row is `not_evaluated`).

use super::fixture::{
    BuildSpec, C1, FakeHost, code, live_root, run, sha256_hex, speaks, tree, word, write_json,
};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The initiator the tests run the module as.
const WORD: Initiator = Initiator::LabRescue;
/// The neutral game, pack and table of the bundles (made-up ids, no real game).
const GAME: &str = "neutral-game";
const PACK: &str = "neutral.pack";
const TABLE: &str = "neutral_scores";
/// The `package` range the installer of these tests declares, as this build's declaration does.
const INSTALLER_PACKAGE: [u64; 2] = [3, 10];
/// The `package` range every bundle here declares, with or without the directory: [`speaks`]'s.
const BUNDLE_PACKAGE: [u64; 2] = [3, 5];

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

/// The bytes of the `loading.json` of [`GAME`] a bundle with the directory carries: two template
/// targets, `loading/first` reading `loading/first.png` and `loading/second` reading
/// `loading/second.png` (`/anchors/0` and `/anchors/1`), each the one required target of a page.
fn declaration() -> Vec<u8> {
    let region = json!({"x": 0, "y": 0, "width": 8, "height": 8});
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.loading.v1",
        "game": GAME,
        "coordinate_space": {"width": 64, "height": 24},
        "max_ms": 30000,
        "anchors": [
            {"id": "loading/first", "template": "loading/first.png", "region": region},
            {"id": "loading/second", "template": "loading/second.png", "region": region}
        ],
        "pages": [
            {"id": "first_wait", "required": ["loading/first"]},
            {"id": "second_wait", "required": ["loading/second"]}
        ]
    }))
    .expect("JSON")
}

/// One neutral bundle zip of [`GAME`]: its packs with their `control.json`, the table [`TABLE`],
/// its `loading.json` when it carries one, and the members of its loading templates directory by
/// their path below it.
struct Bundle {
    packs: Vec<(String, Vec<u8>)>,
    loading: Option<Vec<u8>>,
    templates: Vec<(String, Vec<u8>)>,
}

impl Bundle {
    /// One pack [`PACK`] whose `control.json` holds `control`, neither `loading.json` nor the
    /// directory.
    fn new(control: &[u8]) -> Self {
        Self {
            packs: vec![(PACK.to_owned(), control.to_vec())],
            loading: None,
            templates: Vec::new(),
        }
    }

    /// With [`declaration`] as its `loading.json` and the directory's `templates` (their path below
    /// it and their bytes).
    fn with_loading(mut self, templates: &[(&str, &[u8])]) -> Self {
        self.loading = Some(declaration());
        for (path, bytes) in templates {
            self = self.with_template(path, bytes);
        }
        self
    }

    /// With one more member `loading/<path>` holding `bytes`.
    fn with_template(mut self, path: &str, bytes: &[u8]) -> Self {
        self.templates.push((path.to_owned(), bytes.to_vec()));
        self
    }

    /// With one more pack `package_id`, its `control.json` holding `control`.
    fn with_pack(mut self, package_id: &str, control: &[u8]) -> Self {
        self.packs.push((package_id.to_owned(), control.to_vec()));
        self
    }

    /// The bytes of its `bundle.json`.
    fn index(&self) -> Vec<u8> {
        let packs = self
            .packs
            .iter()
            .map(|(package_id, control)| {
                json!({
                    "package_id": package_id,
                    "server": "neutral-server",
                    "entry_task_id": "neutral.entry",
                    "path": format!("packs/{package_id}"),
                    "file_count": 1,
                    "byte_count": control.len(),
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": packs,
            "tables": [TABLE],
        }))
        .expect("JSON")
    }

    /// Writes the zip to `path`, every member stored, its `interfaces.json` declaring
    /// [`BUNDLE_PACKAGE`].
    fn write(&self, path: &Path) {
        let mut declared = speaks();
        declared["package"] = json!(BUNDLE_PACKAGE);
        let interfaces = serde_json::to_vec(&json!({
            "schema_version": "actingcommand.component-interfaces.v1",
            "speaks": declared,
        }))
        .expect("JSON");
        let mut entries = vec![
            ("bundle.json".to_owned(), self.index()),
            ("interfaces.json".to_owned(), interfaces),
        ];
        for (package_id, control) in &self.packs {
            entries.push((format!("packs/{package_id}/control.json"), control.clone()));
        }
        entries.push((format!("tables/{TABLE}.json"), table_bytes()));
        if let Some(loading) = &self.loading {
            entries.push(("loading.json".to_owned(), loading.clone()));
        }
        for (relative, bytes) in &self.templates {
            entries.push((format!("loading/{relative}"), bytes.clone()));
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

/// The kinds of a journal's steps, in order.
fn step_kinds(journal: &Value) -> Vec<String> {
    journal["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .map(|step| step["kind"].as_str().unwrap_or_default().to_owned())
        .collect()
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

/// The files of a templates directory as `tree` lists them.
fn files_of(files: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .map(|(path, bytes)| ((*path).to_owned(), bytes.to_vec()))
        .collect()
}

/// The values of the first outcome `code` the line carries, as its top or as a link at any depth.
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

/// A root whose configuration names the standard-pack root `<base>\loadp-packs`, adopted, with the
/// empty game root of [`GAME`] in it; the installer declares [`INSTALLER_PACKAGE`], and the fake
/// Runtime runs one instance `neutral.a` bound to [`GAME`]. The root and the game root.
fn prepared(base: &Path, host: &FakeHost) -> (PathBuf, PathBuf) {
    let root = live_root(base, "loadp", &BuildSpec::new(C1));
    let packs = base.join("loadp-packs");
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

const CONTROL: &[u8] = br#"{"neutral": 1}"#;
const FIRST: &[u8] = b"placeholder template one";
const SECOND: &[u8] = b"placeholder template two";

// A bundle that carries `loading.json` and its two templates: planned with the item `loading`
// `new` after the packs and before the root files, at `<game root>\loading`; placed, its step
// after the packs' and before the root files', the files on disk the zip's bytes; planned again
// with every item `unchanged`; `undo` sets the placed directory aside.
#[test]
fn loading_templates_are_placed_after_the_packs_and_read_back_unchanged() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let files = [("first.png", FIRST), ("second.png", SECOND)];
    let zip = temp.path().join("neutral-loading.zip");
    Bundle::new(CONTROL).with_loading(&files).write(&zip);

    // The plan: the templates directory after the pack, new, before the root files.
    let (planned, exit) = place(&host, &root, &zip, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral_scores table new",
            "neutral.pack pack new",
            "loading loading new",
            "bundle.json root new",
            "loading.json root new"
        ],
        "{planned}"
    );
    assert_eq!(
        planned["plan"]["items"][2]["dst"],
        json!(super::files::plain(&game_root.join("loading")))
    );
    assert!(calls_of(&host, "pause ").is_empty());

    // The placement: the item's step after the pack's and before the root files', placed byte for
    // byte.
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(journal["kind"], "pack_place");
    assert_eq!(
        step_kinds(&journal),
        [
            "pause",
            "pack_item",
            "pack_item",
            "pack_item",
            "pack_item",
            "pack_item",
            "post_check",
            "release_hold"
        ]
    );
    assert_eq!(
        touched(&journal),
        [TABLE, PACK, "loading", "bundle.json", "loading.json"]
    );
    let intent = &item_intents(&journal)[2];
    assert_eq!(intent["kind"], "loading", "{intent}");
    assert_eq!(intent["state"], "new", "{intent}");
    assert_eq!(tree(&game_root.join("loading")), files_of(&files));
    assert_eq!(
        fs::read(game_root.join("loading.json")).expect("the placed loading.json"),
        declaration()
    );

    // Planned again: every item unchanged, the templates directory too.
    let (again, exit) = place(&host, &root, &zip, true);
    assert_eq!(exit, 0, "{again}");
    assert_eq!(
        items(&again),
        [
            "neutral_scores table unchanged",
            "neutral.pack pack unchanged",
            "loading loading unchanged",
            "bundle.json root unchanged",
            "loading.json root unchanged"
        ],
        "{again}"
    );

    // `undo`: the placed directory set aside, nothing in its place, as before the placement.
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert!(!game_root.join("loading").exists());
    assert_eq!(
        tree(&kept(&game_root, "loading.rolledback-")),
        files_of(&files)
    );
}

// `replace`, `unchanged` and `retire`, each undone: a changed template replaces the directory (the
// replaced one kept as `.old-<ms>`); an unchanged directory is left while the pack changes; a
// bundle that carries neither `loading.json` nor the directory sets both aside, the plan showing
// the items `retire`. Each `undo` puts back what was there.
#[test]
fn loading_templates_replace_unchanged_retire_and_undo() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let loading = game_root.join("loading");
    let changed: &[u8] = b"placeholder template two, changed";
    let first_files = [("first.png", FIRST), ("second.png", SECOND)];
    let changed_files = [("first.png", FIRST), ("second.png", changed)];
    let first = temp.path().join("neutral-first.zip");
    Bundle::new(CONTROL)
        .with_loading(&first_files)
        .write(&first);
    let (placed, exit) = place(&host, &root, &first, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(tree(&loading), files_of(&first_files));

    // `replace`: one template changed; the directory in place holds the new bytes, the replaced
    // one is kept; `undo` puts it back.
    let replacing = temp.path().join("neutral-replacing.zip");
    Bundle::new(CONTROL)
        .with_loading(&changed_files)
        .write(&replacing);
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &replacing, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(
        items(&placed),
        [
            "neutral_scores table unchanged",
            "neutral.pack pack unchanged",
            "loading loading replace",
            "bundle.json root unchanged",
            "loading.json root unchanged"
        ],
        "{placed}"
    );
    assert_eq!(tree(&loading), files_of(&changed_files));
    assert_eq!(
        tree(&kept(&game_root, "loading.old-")),
        files_of(&first_files)
    );
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(touched(&journal), ["loading"]);
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(tree(&loading), files_of(&first_files));
    assert_eq!(
        tree(&kept(&game_root, "loading.rolledback-")),
        files_of(&changed_files)
    );

    // `unchanged`: the same templates while the pack changes; no step touches the directory, and
    // `undo` leaves it as it is.
    let unchanged = temp.path().join("neutral-unchanged.zip");
    Bundle::new(br#"{"neutral": 2}"#)
        .with_loading(&first_files)
        .write(&unchanged);
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &unchanged, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(
        items(&placed),
        [
            "neutral_scores table unchanged",
            "neutral.pack pack replace",
            "loading loading unchanged",
            "bundle.json root unchanged",
            "loading.json root unchanged"
        ],
        "{placed}"
    );
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(touched(&journal), [PACK]);
    assert_eq!(tree(&loading), files_of(&first_files));
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(tree(&loading), files_of(&first_files));
    assert_eq!(
        fs::read(game_root.join(PACK).join("control.json")).expect("the pack again"),
        CONTROL
    );

    // `retire`: a bundle without `loading.json` and the directory; the plan shows both items, the
    // directory before the file; both are set aside as `.old-<ms>` and kept, and `undo` puts them
    // back.
    let retiring = temp.path().join("neutral-retiring.zip");
    Bundle::new(CONTROL).write(&retiring);
    let (planned, exit) = place(&host, &root, &retiring, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral_scores table unchanged",
            "neutral.pack pack unchanged",
            "loading loading retire",
            "loading.json root retire",
            "bundle.json root unchanged"
        ],
        "{planned}"
    );
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &retiring, false);
    assert_eq!(exit, 0, "{placed}");
    assert!(!loading.exists());
    assert!(!game_root.join("loading.json").exists());
    assert_eq!(
        tree(&kept(&game_root, "loading.old-")),
        files_of(&first_files)
    );
    assert_eq!(
        fs::read(kept(&game_root, "loading.json.old-")).expect("the kept loading.json"),
        declaration()
    );
    let (operation_id, journal) = new_journal(&root, &before);
    assert_eq!(touched(&journal), ["loading", "loading.json"]);
    let retired = &item_intents(&journal)[0];
    assert_eq!(retired["kind"], "loading", "{retired}");
    assert_eq!(retired["state"], "retire", "{retired}");
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(tree(&loading), files_of(&first_files));
    assert_eq!(
        fs::read(game_root.join("loading.json")).expect("loading.json put back"),
        declaration()
    );
}

// A bundle without the directory on a root without it, as before LOAD-P. The plan id this bundle
// has on main before the slice cannot be computed without a build here (the id covers the zip's
// SHA-256, which the zip writer's bytes decide), so the test holds the document instead, as
// NOTICES-2's `notices_absent_bundle_plans_as_before` does: exactly the members and items the
// earlier code writes (no `loading` item), each digest as that code computes it, the plan id the
// earlier formula over it (`plan-` and 16 hex digits of the SHA-256 of its canonical form), and
// the same steps.
#[test]
fn loading_absent_bundle_plans_as_before() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let bundle = Bundle::new(CONTROL);
    let zip = temp.path().join("neutral-bundle.zip");
    bundle.write(&zip);

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let listing = format!("control.json\n{}\n", sha256_hex(CONTROL));
    let expected = json!({
        "game": GAME,
        "zip_sha256": sha256_hex(&fs::read(&zip).expect("the bundle")),
        "package": BUNDLE_PACKAGE,
        "items": [
            {"item": TABLE, "kind": "table", "state": "new", "digest": sha256_hex(&table_bytes())},
            {"item": PACK, "kind": "pack", "state": "new", "digest": sha256_hex(listing.as_bytes())},
            {"item": "bundle.json", "kind": "root", "state": "new",
                "digest": sha256_hex(&bundle.index())},
        ],
    });
    let (_, journal) = new_journal(&root, &before);
    assert_eq!(journal["plan"], expected, "{journal}");
    let canonical = actingcommand_runtime_config::canonical_json(&expected);
    let plan_id = format!("plan-{}", &sha256_hex(canonical.as_bytes())[..16]);
    assert_eq!(journal["plan_id"], json!(plan_id));
    assert_eq!(placed["plan"]["plan_id"], json!(plan_id));
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
    // Each item step's intent has only the members the earlier code writes.
    let known = [
        "item", "kind", "state", "dst", "digest", "members", "zip", "part", "old", "first",
    ];
    for intent in item_intents(&journal) {
        let members = intent.as_object().expect("an intent");
        assert!(
            members
                .keys()
                .all(|member| known.contains(&member.as_str())),
            "{intent}"
        );
        assert_ne!(intent["kind"], "loading", "{intent}");
    }
    assert!(!game_root.join("loading").exists());
}

// Each refusal of the directory's rules, in the plan, before the hold: a member `loading.json`
// does not declare, a path deeper below the directory and a member of a zip without
// `loading.json` are `pack_invalid` naming the member (`<zip>\loading/<path>`, as a member no
// layout knows is named), with no cause; a template the file declares that the zip lacks is
// `pack_invalid` caused by `standard_pack_root_invalid` `loading_template_invalid` at the missing
// member below the zip and the pointer of the target that reads it; a pack named `loading` or
// `Loading` beside the directory is `loading_pack_name` at `<zip>\bundle.json` with its
// `package_id`. No pause, no journal, nothing in the game root.
#[test]
fn loading_refusals_come_before_the_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let both: [(&str, &[u8]); 2] = [("first.png", FIRST), ("second.png", SECOND)];
    let cases = [
        (
            "a template loading.json does not declare",
            Bundle::new(CONTROL)
                .with_loading(&both)
                .with_template("third.png", FIRST),
            "loading/third.png",
            Value::Null,
        ),
        (
            "a path deeper below the directory",
            Bundle::new(CONTROL)
                .with_loading(&both)
                .with_template("deep/first.png", FIRST),
            "loading/deep/first.png",
            Value::Null,
        ),
        (
            "the directory without loading.json",
            Bundle::new(CONTROL)
                .with_template("first.png", FIRST)
                .with_template("second.png", SECOND),
            "loading/first.png",
            Value::Null,
        ),
        (
            "a declared template the zip lacks",
            Bundle::new(CONTROL).with_loading(&both[..1]),
            "loading/second.png",
            json!({
                "standard_pack_problem": "loading_template_invalid",
                "json_pointer": "/anchors/1/template",
            }),
        ),
        (
            "a pack named like the directory",
            Bundle::new(CONTROL)
                .with_pack("loading", CONTROL)
                .with_loading(&both),
            "bundle.json",
            json!({
                "standard_pack_problem": "loading_pack_name",
                "json_pointer": "",
                "package_id": "loading",
            }),
        ),
        (
            "a pack named like the directory in another case",
            Bundle::new(CONTROL)
                .with_pack("Loading", CONTROL)
                .with_loading(&both),
            "bundle.json",
            json!({
                "standard_pack_problem": "loading_pack_name",
                "json_pointer": "",
                "package_id": "Loading",
            }),
        ),
    ];
    let before = journals(&root);
    for (index, (case, bundle, member, cause)) in cases.into_iter().enumerate() {
        let zip = temp.path().join(format!("neutral-refused-{index}.zip"));
        bundle.write(&zip);
        let (refused, exit) = place(&host, &root, &zip, false);
        assert_eq!(exit, 1, "{case}: {refused}");
        assert_eq!(
            top_code(&refused),
            "install_command_refused",
            "{case}: {refused}"
        );
        assert_eq!(
            top_value(&refused, "install_refusal"),
            "pack_invalid",
            "{case}"
        );
        assert_eq!(
            top_value(&refused, "operation"),
            "install_pack_place",
            "{case}"
        );
        let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
        let found = values_of(&refused, "standard_pack_root_invalid");
        if cause.is_null() {
            // Named as a member no layout knows: the member itself, no cause.
            assert!(found.is_none(), "{case}: {refused}");
            assert_eq!(
                top_value(&refused, "path"),
                &json!(canonical.join(member).to_string_lossy()),
                "{case}: {refused}"
            );
            continue;
        }
        let found =
            found.unwrap_or_else(|| panic!("{case}: no standard_pack_root_invalid in {refused}"));
        let path = super::files::join_relative(&canonical, member);
        assert_eq!(
            found["path"],
            json!(path.to_string_lossy()),
            "{case}: {found}"
        );
        assert_eq!(found["game"], GAME, "{case}: {found}");
        for key in ["standard_pack_problem", "json_pointer", "package_id"] {
            assert_eq!(found[key], cause[key], "{case}: {key} in {found}");
        }
    }
    // Nothing was held or written.
    assert!(
        calls_of(&host, "pause ").is_empty(),
        "{:?}",
        host.runtime.calls.borrow()
    );
    assert_eq!(journals(&root).len(), before.len());
    assert!(tree(&game_root).is_empty(), "{:?}", tree(&game_root).keys());
}
