// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #444 NOTICES-2 (register §1.335; contracts/install-cli.md, "The Lab's rescue module";
// contracts/package-reference.md, "Notices"; coordinator work order
// `rd-1113/brief-444-notices-2.md`): the rescue module's `place` takes a bundle zip's notices
// directory `notices/` as one item of kind `notices`, placed first, held to the contract crate's
// rules before anything is held or written, and set aside when a later bundle carries none.
// - `notices_are_placed_first_and_read_back_unchanged`: a bundle that carries the directory and
//   declares `package` 10 is planned with the item `new` and first, placed (the files on disk are
//   the zip's bytes), planned again with every item `unchanged`, and undone;
// - `notices_replace_unchanged_retire_and_undo`: a changed text replaces the directory, an
//   unchanged one is left while another item changes, a bundle without it retires it (set aside,
//   the plan showing the item); each `undo` puts back what was there;
// - `notices_absent_bundle_plans_as_before`: a bundle without the directory on a root without it
//   plans, journals and places as before NOTICES-2: the plan document holds exactly the members
//   and items the earlier code writes, its id is the earlier formula over it, and the steps are
//   the same;
// - `notices_refusals_come_before_the_hold`: `notice_undeclared` and one refusal per rule family,
//   each `pack_invalid` caused by `standard_pack_root_invalid` naming its member, before the hold
//   with nothing written;
// - `notices_edge_is_checked_before_the_layout`: a bundle that fails both the `package` edge and
//   the layout is refused `interface_edge`.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files (a notice file is `NOTICE.md`).

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
/// The `package` range a bundle that carries the notices directory declares.
const NOTICES_PACKAGE: [u64; 2] = [10, 10];
/// The `package` range a bundle without it declares: [`speaks`]'s.
const EARLIER_PACKAGE: [u64; 2] = [3, 5];

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

/// One neutral bundle zip of [`GAME`]: its packs with their `control.json`, the table [`TABLE`],
/// the files of its notices directory by their path below it, the `package` range its
/// `interfaces.json` declares, further members, and the member whose size its central directory
/// understates.
struct Bundle {
    packs: Vec<(String, Vec<u8>)>,
    notices: Vec<(String, Vec<u8>)>,
    package: [u64; 2],
    extra: Vec<(String, Vec<u8>)>,
    understated: Option<String>,
}

impl Bundle {
    /// One pack [`PACK`] whose `control.json` holds `control`, no notices directory, declaring
    /// [`EARLIER_PACKAGE`].
    fn new(control: &[u8]) -> Self {
        Self {
            packs: vec![(PACK.to_owned(), control.to_vec())],
            notices: Vec::new(),
            package: EARLIER_PACKAGE,
            extra: Vec::new(),
            understated: None,
        }
    }

    /// With the notices directory's `files` (their path below it and their bytes), declaring
    /// [`NOTICES_PACKAGE`].
    fn with_notices(mut self, files: Vec<(&str, Vec<u8>)>) -> Self {
        self.notices = files
            .into_iter()
            .map(|(path, bytes)| (path.to_owned(), bytes))
            .collect();
        self.package = NOTICES_PACKAGE;
        self
    }

    /// Declaring `package` instead.
    fn declaring(mut self, package: [u64; 2]) -> Self {
        self.package = package;
        self
    }

    /// With one more pack `package_id`, its `control.json` holding `control`.
    fn with_pack(mut self, package_id: &str, control: &[u8]) -> Self {
        self.packs.push((package_id.to_owned(), control.to_vec()));
        self
    }

    /// With one more member `name` that no layout knows.
    fn with_member(mut self, name: &str, bytes: &[u8]) -> Self {
        self.extra.push((name.to_owned(), bytes.to_vec()));
        self
    }

    /// With the size the central directory declares for `member` rewritten to 1 byte.
    fn understating(mut self, member: &str) -> Self {
        self.understated = Some(member.to_owned());
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
            ("bundle.json".to_owned(), self.index()),
            ("interfaces.json".to_owned(), interfaces),
        ];
        for (package_id, control) in &self.packs {
            entries.push((format!("packs/{package_id}/control.json"), control.clone()));
        }
        entries.push((format!("tables/{TABLE}.json"), table_bytes()));
        for (relative, bytes) in &self.notices {
            entries.push((format!("notices/{relative}"), bytes.clone()));
        }
        entries.extend(self.extra.iter().cloned());
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
        if let Some(member) = &self.understated {
            understate(path, member);
        }
    }
}

/// Rewrites the uncompressed size the central directory of the zip at `path` declares for
/// `member` to 1 byte; its local header and its bytes stay, so the member holds more than the
/// zip declares.
fn understate(path: &Path, member: &str) {
    let mut bytes = fs::read(path).expect("the bundle");
    // A central directory header: its signature, the uncompressed size at 24, the name at 46.
    let header = bytes
        .windows(46 + member.len())
        .position(|window| window.starts_with(b"PK\x01\x02") && window.ends_with(member.as_bytes()))
        .expect("the member's central directory header");
    bytes[header + 24..header + 28].copy_from_slice(&1_u32.to_le_bytes());
    fs::write(path, bytes).expect("the understated bundle");
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

/// The files of a notices directory as `tree` lists them.
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

/// A root whose configuration names the standard-pack root `<base>\notices-packs`, adopted, with
/// the empty game root of [`GAME`] in it; the installer declares [`INSTALLER_PACKAGE`], and the
/// fake Runtime runs one instance `neutral.a` bound to [`GAME`]. The root and the game root.
fn prepared(base: &Path, host: &FakeHost) -> (PathBuf, PathBuf) {
    let root = live_root(base, "notices", &BuildSpec::new(C1));
    let packs = base.join("notices-packs");
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
const NOTICE: &[u8] = b"Placeholder notice.\n";
const OTHER: &[u8] = b"Second placeholder text.\r\n";

// A bundle that carries the notices directory and declares `package` 10: planned with the item
// `notices` `new` and first; placed, its files on disk the zip's bytes, its step the operation's
// first; planned again with every item `unchanged`; `undo` sets the placed directory aside.
#[test]
fn notices_are_placed_first_and_read_back_unchanged() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let files = [("NOTICE.md", NOTICE), ("texts/OTHER.txt", OTHER)];
    let zip = temp.path().join("neutral-notices.zip");
    Bundle::new(CONTROL)
        .with_notices(vec![
            ("NOTICE.md", NOTICE.to_vec()),
            ("texts/OTHER.txt", OTHER.to_vec()),
        ])
        .write(&zip);

    // The plan: the notices directory first, new, at `<game root>\notices`.
    let (planned, exit) = place(&host, &root, &zip, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "notices notices new",
            "neutral_scores table new",
            "neutral.pack pack new",
            "bundle.json root new"
        ],
        "{planned}"
    );
    assert_eq!(
        planned["plan"]["items"][0]["dst"],
        json!(super::files::plain(&game_root.join("notices")))
    );
    assert!(calls_of(&host, "pause ").is_empty());

    // The placement: the item's step is the first, placed byte for byte.
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    assert_eq!(items(&placed)[0], "notices notices new");
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
            "post_check",
            "release_hold"
        ]
    );
    let first = &item_intents(&journal)[0];
    assert_eq!(first["item"], "notices", "{first}");
    assert_eq!(first["kind"], "notices", "{first}");
    assert_eq!(first["state"], "new", "{first}");
    assert_eq!(first["first"], json!(true), "{first}");
    assert_eq!(tree(&game_root.join("notices")), files_of(&files));

    // Planned again: every item unchanged, the notices directory too.
    let (again, exit) = place(&host, &root, &zip, true);
    assert_eq!(exit, 0, "{again}");
    assert_eq!(
        items(&again),
        [
            "notices notices unchanged",
            "neutral_scores table unchanged",
            "neutral.pack pack unchanged",
            "bundle.json root unchanged"
        ],
        "{again}"
    );

    // `undo`: the placed directory set aside, nothing in its place, as before the placement.
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert!(!game_root.join("notices").exists());
    assert_eq!(
        tree(&kept(&game_root, "notices.rolledback-")),
        files_of(&files)
    );
}

// `replace`, `unchanged` and `retire`, each undone: a changed text replaces the directory (the
// replaced one kept as `.old-<ms>`); an unchanged directory is left while the pack changes; a
// bundle that carries none sets it aside, the plan showing the item `retire`. Each `undo` puts
// back what was there.
#[test]
fn notices_replace_unchanged_retire_and_undo() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let notices = game_root.join("notices");
    let changed: &[u8] = b"Placeholder notice, changed.\n";
    let first_files = [("NOTICE.md", NOTICE), ("texts/OTHER.txt", OTHER)];
    let changed_files = [("NOTICE.md", changed), ("texts/OTHER.txt", OTHER)];
    let first = temp.path().join("neutral-first.zip");
    Bundle::new(CONTROL)
        .with_notices(vec![
            ("NOTICE.md", NOTICE.to_vec()),
            ("texts/OTHER.txt", OTHER.to_vec()),
        ])
        .write(&first);
    let (placed, exit) = place(&host, &root, &first, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(tree(&notices), files_of(&first_files));

    // `replace`: one text changed; the directory in place holds the new text, the replaced one is
    // kept; `undo` puts it back.
    let replacing = temp.path().join("neutral-replacing.zip");
    Bundle::new(CONTROL)
        .with_notices(vec![
            ("NOTICE.md", changed.to_vec()),
            ("texts/OTHER.txt", OTHER.to_vec()),
        ])
        .write(&replacing);
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &replacing, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(
        items(&placed),
        [
            "notices notices replace",
            "neutral_scores table unchanged",
            "neutral.pack pack unchanged",
            "bundle.json root unchanged"
        ],
        "{placed}"
    );
    assert_eq!(tree(&notices), files_of(&changed_files));
    assert_eq!(
        tree(&kept(&game_root, "notices.old-")),
        files_of(&first_files)
    );
    let (operation_id, _) = new_journal(&root, &before);
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(tree(&notices), files_of(&first_files));
    assert_eq!(
        tree(&kept(&game_root, "notices.rolledback-")),
        files_of(&changed_files)
    );

    // `unchanged`: the same texts while the pack changes; no step touches the directory, and
    // `undo` leaves it as it is.
    let unchanged = temp.path().join("neutral-unchanged.zip");
    Bundle::new(br#"{"neutral": 2}"#)
        .with_notices(vec![
            ("NOTICE.md", NOTICE.to_vec()),
            ("texts/OTHER.txt", OTHER.to_vec()),
        ])
        .write(&unchanged);
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &unchanged, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(
        items(&placed),
        [
            "notices notices unchanged",
            "neutral_scores table unchanged",
            "neutral.pack pack replace",
            "bundle.json root unchanged"
        ],
        "{placed}"
    );
    let (operation_id, journal) = new_journal(&root, &before);
    let touched = item_intents(&journal)
        .iter()
        .map(|intent| intent["item"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(touched, [PACK]);
    assert_eq!(tree(&notices), files_of(&first_files));
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(tree(&notices), files_of(&first_files));
    assert_eq!(
        fs::read(game_root.join(PACK).join("control.json")).expect("the pack again"),
        CONTROL
    );

    // `retire`: a bundle without the directory; the plan shows the item, the directory is set
    // aside as `.old-<ms>` and kept, and `undo` puts it back.
    let retiring = temp.path().join("neutral-retiring.zip");
    Bundle::new(CONTROL).write(&retiring);
    let (planned, exit) = place(&host, &root, &retiring, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "notices notices retire",
            "neutral_scores table unchanged",
            "neutral.pack pack unchanged",
            "bundle.json root unchanged"
        ],
        "{planned}"
    );
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &retiring, false);
    assert_eq!(exit, 0, "{placed}");
    assert!(!notices.exists());
    assert_eq!(
        tree(&kept(&game_root, "notices.old-")),
        files_of(&first_files)
    );
    let (operation_id, journal) = new_journal(&root, &before);
    let retired = &item_intents(&journal)[0];
    assert_eq!(retired["item"], "notices", "{retired}");
    assert_eq!(retired["state"], "retire", "{retired}");
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(tree(&notices), files_of(&first_files));
}

// A bundle without the directory on a root without it, as before NOTICES-2. The work order asks
// for the plan id this bundle has on main before the slice; it cannot be computed without a build
// here (the id covers the zip's SHA-256, which the zip writer's bytes decide), so the test holds
// the document instead: exactly the members and items the earlier code writes (no `notices`
// item), each digest as that code computes it, the plan id the earlier formula over it (`plan-`
// and 16 hex digits of the SHA-256 of its canonical form), and the same steps.
#[test]
fn notices_absent_bundle_plans_as_before() {
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
        "package": EARLIER_PACKAGE,
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
        assert_ne!(intent["kind"], "notices", "{intent}");
    }
    assert!(!game_root.join("notices").exists());
}

// `notice_undeclared` and one refusal per rule family, each `pack_invalid` of `place` caused by
// `standard_pack_root_invalid` with the rule's word, the member it names below the zip, an empty
// pointer and the bound with the value found; each in the plan, before the hold: no pause, no
// journal, nothing in the game root.
#[test]
fn notices_refusals_come_before_the_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let notice = || vec![("NOTICE.md", NOTICE.to_vec())];
    let many = (0..65)
        .map(|index| format!("NOTICE-{index:02}.md"))
        .collect::<Vec<_>>();
    let cases = [
        (
            "a bundle that carries the directory declares package 9",
            Bundle::new(CONTROL)
                .with_notices(notice())
                .declaring([9, 9]),
            "interfaces.json",
            "notice_undeclared",
            json!({}),
        ),
        (
            "a name of a refused suffix",
            Bundle::new(CONTROL).with_notices(vec![("NOTICE.html", NOTICE.to_vec())]),
            "notices/NOTICE.html",
            "notice_name",
            json!({}),
        ),
        (
            "a size the zip declares below what the member holds",
            Bundle::new(CONTROL)
                .with_notices(vec![("NOTICE.md", vec![b'a'; 2_097_153])])
                .understating("notices/NOTICE.md"),
            "notices/NOTICE.md",
            "notice_bytes",
            json!({"limit_bytes": 2_097_152, "actual_bytes": 2_097_153}),
        ),
        (
            "65 files",
            Bundle::new(CONTROL).with_notices(
                many.iter()
                    .map(|path| (path.as_str(), b"a".to_vec()))
                    .collect(),
            ),
            "notices",
            "notice_count",
            json!({"limit_count": 64, "count": 65}),
        ),
        (
            "8 MiB and one byte together",
            Bundle::new(CONTROL).with_notices(vec![
                ("a.md", vec![b'a'; 2_097_152]),
                ("b.md", vec![b'a'; 2_097_152]),
                ("c.md", vec![b'a'; 2_097_152]),
                ("d.md", vec![b'a'; 2_097_152]),
                ("e.md", b"a".to_vec()),
            ]),
            "notices",
            "notice_total_bytes",
            json!({"limit_bytes": 8_388_608, "actual_bytes": 8_388_609}),
        ),
        (
            "a control character",
            Bundle::new(CONTROL).with_notices(vec![("NOTICE.md", b"a\x00b".to_vec())]),
            "notices/NOTICE.md",
            "notice_not_text",
            json!({}),
        ),
        (
            "a pack named like the directory",
            Bundle::new(CONTROL)
                .with_pack("notices", CONTROL)
                .with_notices(notice()),
            "bundle.json",
            "notice_name",
            json!({"package_id": "notices"}),
        ),
        (
            "one directory's name in two cases",
            Bundle::new(CONTROL).with_notices(vec![
                ("A/NOTICE.md", NOTICE.to_vec()),
                ("a/OTHER.md", OTHER.to_vec()),
            ]),
            "notices/a/OTHER.md",
            "notice_name",
            json!({}),
        ),
    ];
    let before = journals(&root);
    for (index, (case, bundle, member, problem, bound)) in cases.into_iter().enumerate() {
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
        let cause = values_of(&refused, "standard_pack_root_invalid")
            .unwrap_or_else(|| panic!("{case}: no standard_pack_root_invalid in {refused}"));
        let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
        let path = super::files::join_relative(&canonical, member);
        assert_eq!(cause["standard_pack_problem"], problem, "{case}: {cause}");
        assert_eq!(
            cause["path"],
            json!(path.to_string_lossy()),
            "{case}: {cause}"
        );
        assert_eq!(cause["json_pointer"], "", "{case}: {cause}");
        assert_eq!(cause["game"], GAME, "{case}: {cause}");
        for key in [
            "package_id",
            "limit_count",
            "count",
            "limit_bytes",
            "actual_bytes",
        ] {
            assert_eq!(cause[key], bound[key], "{case}: {key} in {cause}");
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

// The `package` edge before the layout: a bundle whose declared maximum lies above the installer's
// range and that carries a member no layout knows is refused `interface_edge` (before NOTICES-2,
// `pack_invalid` at the member); the same bundle within the range is still refused `pack_invalid`
// at that member. Nothing is held or written.
#[test]
fn notices_edge_is_checked_before_the_layout() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let before = journals(&root);

    let later = temp.path().join("neutral-later.zip");
    Bundle::new(CONTROL)
        .with_member("unknown/member.txt", b"x")
        .declaring([11, 11])
        .write(&later);
    let (refused, exit) = place(&host, &root, &later, false);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(top_value(&refused, "install_refusal"), "interface_edge");
    let canonical = fs::canonicalize(&later).expect("the canonical bundle");
    assert_eq!(
        top_value(&refused, "path"),
        &json!(canonical.join("interfaces.json").to_string_lossy())
    );

    let within = temp.path().join("neutral-within.zip");
    Bundle::new(CONTROL)
        .with_member("unknown/member.txt", b"x")
        .declaring(NOTICES_PACKAGE)
        .write(&within);
    let (refused, exit) = place(&host, &root, &within, false);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_value(&refused, "install_refusal"), "pack_invalid");
    let canonical = fs::canonicalize(&within).expect("the canonical bundle");
    assert_eq!(
        top_value(&refused, "path"),
        &json!(canonical.join("unknown/member.txt").to_string_lossy())
    );

    assert!(calls_of(&host, "pause ").is_empty());
    assert_eq!(journals(&root).len(), before.len());
    assert!(tree(&game_root).is_empty());
}
