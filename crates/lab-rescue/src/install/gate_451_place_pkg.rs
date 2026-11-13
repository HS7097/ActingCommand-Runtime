// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #451 PLACE-PKG (register §1.339; contracts/install-cli.md, "The Lab's rescue module",
// `place`; contracts/component-interfaces.md, "Declaration", "Resource bundle"; coordinator work
// order `rd-1113/brief-451-place-pkg.md`): the rescue module's `place` reads a bundle's
// `interfaces.json` by the bundle's own rule (its schema, every value under `speaks` a pair,
// `speaks.package` named), and no longer by the rule of a Runtime's declaration, which names all
// six v1 interfaces; a resource repository's release declares `package` alone, beside
// `validated_with`.
// - `place_pkg_takes_a_bundle_that_declares_package_alone`: a bundle whose `interfaces.json` has
//   that shape is planned and placed, and the journalled plan document's `package` is the pair it
//   declares;
// - `place_pkg_six_interfaces_plan_as_package_alone`: the same bundle declaring all six v1
//   interfaces (the earlier gates' shape) plans identically: the same items, the same document but
//   for the zip's SHA-256, the same answer of `place --plan`;
// - `place_pkg_refusals_come_before_the_hold`: no `interfaces.json`, a wrong schema string,
//   `speaks` without `package`, a `package` that is no pair of integers, `min > max`, a
//   well-formed `package` beside a malformed value of another name (every value under `speaks` is
//   a pair, the parse rules of contracts/component-interfaces.md), a maximum above the installer's
//   range and one below it are each refused `interface_edge` at `<zip>\interfaces.json` with the
//   game, before the hold: no pause, no journal, nothing in the game root; a well-formed `package`
//   beside a well-formed name the reader does not know is planned as the released shape is;
// - `place_pkg_notices_bundle_declaring_package_alone`: a bundle that carries the notices
//   directory and declares `package` [3, 9] alone is `notice_undeclared` as before; declaring
//   [10, 10] alone it is planned and placed.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files. The bundles are built as
// `gate_444_notices2.rs` builds its own; that file is not changed.

use super::fixture::{
    BuildSpec, C1, FakeHost, REPOSITORY, code, live_root, run, speaks, tree, word, write_json,
};
use super::place::PLACE;
use super::state::Paths;
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
/// The schema of an interface declaration.
const SCHEMA: &str = "actingcommand.component-interfaces.v1";
/// The `package` range the installer of these tests declares, as this build's declaration does.
const INSTALLER_PACKAGE: [u64; 2] = [3, 10];
/// The `package` range a resource repository's release declares for a bundle without the notices
/// directory (contracts/resource-verify.md, the `bundle` step).
const RELEASED: [u64; 2] = [3, 3];
/// The `package` range it declares for a bundle that carries the directory.
const NOTICES_PACKAGE: [u64; 2] = [10, 10];
/// The bytes of the pack's `control.json`.
const CONTROL: &[u8] = br#"{"neutral": 1}"#;
/// The bytes of the one notice file.
const NOTICE: &[u8] = b"Placeholder notice.\n";

/// A bundle's declaration in the shape a resource repository's release writes it (`resource
/// verify`'s `bundle` step): the schema, `speaks` naming `package` alone with the value `package`
/// (any JSON value, so that a malformed one can be written), and `validated_with` (a neutral
/// repository and commit).
fn released(package: Value) -> Value {
    json!({
        "schema_version": SCHEMA,
        "speaks": {"package": package},
        "validated_with": {"repository": REPOSITORY, "commit": C1},
    })
}

/// A declaration that names all six v1 interfaces and `runtime-outcomes`, as the bundles of the
/// earlier gates declare: the fixture's [`speaks`] with `package` set to `package`.
fn all_six(package: [u64; 2]) -> Value {
    let mut declared = speaks();
    declared["package"] = json!(package);
    json!({"schema_version": SCHEMA, "speaks": declared})
}

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

/// The bytes of every bundle's `bundle.json`: one pack [`PACK`] and the table [`TABLE`].
fn index_bytes() -> Vec<u8> {
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
        "tables": [TABLE],
    }))
    .expect("JSON")
}

/// One neutral bundle zip of [`GAME`]: its `bundle.json`, its `interfaces.json` (none when
/// `interfaces` is `None`; written LF-terminated, as a release writes it), the pack [`PACK`] with
/// its `control.json`, the table [`TABLE`] and the files of its notices directory.
struct Bundle {
    interfaces: Option<Value>,
    notices: Vec<(String, Vec<u8>)>,
}

impl Bundle {
    /// Declaring `interfaces`, without the notices directory.
    fn new(interfaces: Option<Value>) -> Self {
        Self {
            interfaces,
            notices: Vec::new(),
        }
    }

    /// With one more file of the notices directory, `path` below it.
    fn with_notice(mut self, path: &str, bytes: &[u8]) -> Self {
        self.notices.push((path.to_owned(), bytes.to_vec()));
        self
    }

    /// Writes the zip to `path`, every member stored.
    fn write(&self, path: &Path) {
        let mut entries = vec![("bundle.json".to_owned(), index_bytes())];
        if let Some(interfaces) = &self.interfaces {
            let mut bytes = serde_json::to_vec(interfaces).expect("JSON");
            bytes.push(b'\n');
            entries.push(("interfaces.json".to_owned(), bytes));
        }
        entries.push((format!("packs/{PACK}/control.json"), CONTROL.to_vec()));
        entries.push((format!("tables/{TABLE}.json"), table_bytes()));
        for (relative, bytes) in &self.notices {
            entries.push((format!("notices/{relative}"), bytes.clone()));
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

/// `place` of the bundle `zip` on `root` as [`WORD`], only its plan when `plan_only`; its one line
/// and exit code.
fn place(host: &FakeHost, root: &Path, zip: &Path, plan_only: bool) -> (Value, u8) {
    let location = location(root);
    let context = Context {
        host,
        location: &location,
        initiator: WORD,
    };
    let params = if plan_only {
        json!({"bundle": word(zip), "plan": true})
    } else {
        json!({"bundle": word(zip)})
    };
    let params = match params {
        Value::Object(params) => params,
        _ => Map::new(),
    };
    match rescue::answer_with(&context, "place", &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("place: a line, not {other:?}"),
    }
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

/// The one journal written since `before`.
fn new_journal(root: &Path, before: &[(String, Value)]) -> Value {
    let mut written = journals(root)
        .into_iter()
        .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
        .collect::<Vec<_>>();
    assert_eq!(written.len(), 1, "{written:?}");
    written.remove(0).1
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

/// A plan document without its `zip_sha256`, which two zips of other bytes do not share.
fn without_zip(document: &Value) -> Value {
    let mut document = document.clone();
    let removed = document
        .as_object_mut()
        .expect("a plan document")
        .remove("zip_sha256");
    assert!(removed.is_some(), "no zip_sha256 in {document}");
    document
}

/// A root whose configuration names the standard-pack root `<base>\place-pkg-packs`, adopted,
/// with the empty game root of [`GAME`] in it; the installer declares [`INSTALLER_PACKAGE`], and
/// the fake Runtime runs one instance `neutral.a` bound to [`GAME`]. The root and the game root.
fn prepared(base: &Path, host: &FakeHost) -> (PathBuf, PathBuf) {
    let root = live_root(base, "place-pkg", &BuildSpec::new(C1));
    let packs = base.join("place-pkg-packs");
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

// A bundle whose `interfaces.json` is the shape a resource repository releases (`package` alone,
// beside `validated_with`): planned, placed (the pack and `bundle.json` on disk the zip's bytes),
// and the journalled plan document's `package` is the declared pair.
#[test]
fn place_pkg_takes_a_bundle_that_declares_package_alone() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let zip = temp.path().join("neutral-released.zip");
    Bundle::new(Some(released(json!(RELEASED)))).write(&zip);

    let (planned, exit) = place(&host, &root, &zip, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        [
            "neutral_scores table new",
            "neutral.pack pack new",
            "bundle.json root new"
        ],
        "{planned}"
    );
    assert!(calls_of(&host, "pause ").is_empty());

    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &zip, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let journal = new_journal(&root, &before);
    assert_eq!(journal["kind"], "pack_place", "{journal}");
    assert_eq!(journal["plan"]["package"], json!(RELEASED), "{journal}");
    assert_eq!(
        fs::read(game_root.join(PACK).join("control.json")).expect("the placed pack"),
        CONTROL
    );
    assert_eq!(
        fs::read(game_root.join("bundle.json")).expect("the placed index"),
        index_bytes()
    );
}

// The same bundle declaring all six v1 interfaces, as the earlier gates' bundles do, with the same
// `package` pair: `place` plans it identically, on the same root (the same items with their
// states, places, digests and members, the same plan document but for the zip's SHA-256, the same
// items in the answer of `place --plan`).
#[test]
fn place_pkg_six_interfaces_plan_as_package_alone() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), &host);
    let alone_zip = temp.path().join("neutral-alone.zip");
    Bundle::new(Some(released(json!(RELEASED)))).write(&alone_zip);
    let six_zip = temp.path().join("neutral-six.zip");
    Bundle::new(Some(all_six(RELEASED))).write(&six_zip);

    // The plans as `place` computes them.
    let paths = Paths::resolve(&root).expect("the root");
    let loaded = super::plan::load(&paths, PLACE).expect("the adopted root");
    let alone = super::place::plan(&host, &paths, &loaded, &alone_zip, None)
        .expect("the bundle that declares package alone is planned");
    let six = super::place::plan(&host, &paths, &loaded, &six_zip, None)
        .expect("the bundle that declares all six is planned");
    assert_eq!(
        alone.document["package"],
        json!(RELEASED),
        "{}",
        alone.document
    );
    assert_eq!(six.items, alone.items);
    assert_eq!(six.game, alone.game);
    assert_eq!(six.game_root, alone.game_root);
    assert_eq!(without_zip(&six.document), without_zip(&alone.document));

    // And as the module answers `place --plan`.
    let (alone_line, exit) = place(&host, &root, &alone_zip, true);
    assert_eq!(exit, 0, "{alone_line}");
    let (six_line, exit) = place(&host, &root, &six_zip, true);
    assert_eq!(exit, 0, "{six_line}");
    assert_eq!(six_line["plan"]["items"], alone_line["plan"]["items"]);
    assert_eq!(
        items(&alone_line),
        [
            "neutral_scores table new",
            "neutral.pack pack new",
            "bundle.json root new"
        ],
        "{alone_line}"
    );
}

// Each declaration the bundle's rule refuses (a malformed value under any name of `speaks`
// included), and each declared maximum outside the installer's `package` range: `interface_edge`
// of `place` at `<zip>\interfaces.json` with the game, in the plan, before the hold: no pause, no
// journal, nothing in the game root. A well-formed name the reader does not know beside `package`
// is planned as the released shape is.
#[test]
fn place_pkg_refusals_come_before_the_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let mut others = speaks();
    let removed = others
        .as_object_mut()
        .expect("the fixture's interfaces")
        .remove("package");
    assert!(removed.is_some(), "no package in {others}");
    let cases = [
        ("no interfaces.json", None),
        (
            "a wrong schema string",
            Some(json!({
                "schema_version": "actingcommand.component-interfaces.v2",
                "speaks": {"package": RELEASED},
                "validated_with": {"repository": REPOSITORY, "commit": C1},
            })),
        ),
        (
            "speaks without package",
            Some(json!({
                "schema_version": SCHEMA,
                "speaks": others,
                "validated_with": {"repository": REPOSITORY, "commit": C1},
            })),
        ),
        ("package of one integer", Some(released(json!([3])))),
        (
            "package of three integers",
            Some(released(json!([3, 3, 3]))),
        ),
        ("package a string", Some(released(json!("3")))),
        (
            "package a pair with a string",
            Some(released(json!([3, "3"]))),
        ),
        (
            "package a pair with a fraction",
            Some(released(json!([3, 3.5]))),
        ),
        (
            "package a pair with a negative",
            Some(released(json!([-1, 3]))),
        ),
        ("min above max", Some(released(json!([5, 3])))),
        (
            "a well-formed package beside a malformed value of another name",
            Some(json!({
                "schema_version": SCHEMA,
                "speaks": {"package": RELEASED, "neutral-interface": [1, "1"]},
                "validated_with": {"repository": REPOSITORY, "commit": C1},
            })),
        ),
        (
            "a maximum above the installer's range",
            Some(released(json!([11, 11]))),
        ),
        (
            "a maximum below the installer's range",
            Some(released(json!([2, 2]))),
        ),
    ];
    let before = journals(&root);
    for (index, (case, interfaces)) in cases.into_iter().enumerate() {
        let zip = temp.path().join(format!("neutral-refused-{index}.zip"));
        Bundle::new(interfaces).write(&zip);
        let (refused, exit) = place(&host, &root, &zip, false);
        assert_eq!(exit, 1, "{case}: {refused}");
        assert_eq!(
            top_code(&refused),
            "install_command_refused",
            "{case}: {refused}"
        );
        assert_eq!(
            top_value(&refused, "install_refusal"),
            "interface_edge",
            "{case}: {refused}"
        );
        assert_eq!(
            top_value(&refused, "operation"),
            "install_pack_place",
            "{case}"
        );
        let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
        assert_eq!(
            top_value(&refused, "path"),
            &json!(canonical.join("interfaces.json").to_string_lossy()),
            "{case}: {refused}"
        );
        assert_eq!(top_value(&refused, "game"), GAME, "{case}: {refused}");
    }
    // A well-formed `package` beside a second, well-formed name the reader does not know: planned
    // as the released shape is (only planned, so nothing is held or written).
    let real = temp.path().join("neutral-real.zip");
    Bundle::new(Some(released(json!(RELEASED)))).write(&real);
    let unknown = temp.path().join("neutral-unknown-name.zip");
    Bundle::new(Some(json!({
        "schema_version": SCHEMA,
        "speaks": {"package": RELEASED, "neutral-interface": [1, 2]},
        "validated_with": {"repository": REPOSITORY, "commit": C1},
    })))
    .write(&unknown);
    let (real_line, exit) = place(&host, &root, &real, true);
    assert_eq!(exit, 0, "{real_line}");
    let (unknown_line, exit) = place(&host, &root, &unknown, true);
    assert_eq!(exit, 0, "{unknown_line}");
    assert_eq!(
        top_code(&unknown_line),
        top_code(&real_line),
        "{unknown_line}"
    );
    assert_eq!(
        unknown_line["plan"]["items"], real_line["plan"]["items"],
        "{unknown_line}"
    );
    assert_eq!(
        items(&unknown_line),
        [
            "neutral_scores table new",
            "neutral.pack pack new",
            "bundle.json root new"
        ],
        "{unknown_line}"
    );
    // Nothing was held or written.
    assert!(
        calls_of(&host, "pause ").is_empty(),
        "{:?}",
        host.runtime.calls.borrow()
    );
    assert_eq!(journals(&root).len(), before.len());
    assert!(tree(&game_root).is_empty(), "{:?}", tree(&game_root).keys());
}

// A bundle that carries the notices directory and declares `package` alone: [3, 9] is within the
// installer's range and below the notices revision, so it is refused `notice_undeclared` as
// before (`pack_invalid` caused by `standard_pack_root_invalid` at `<zip>\interfaces.json`), before
// the hold; [10, 10] is taken: planned with the item `notices` first, placed, the journalled plan
// document's `package` the declared pair, the file on disk the zip's bytes.
#[test]
fn place_pkg_notices_bundle_declaring_package_alone() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    let before = journals(&root);

    let undeclared = temp.path().join("neutral-undeclared.zip");
    Bundle::new(Some(released(json!([3, 9]))))
        .with_notice("NOTICE.md", NOTICE)
        .write(&undeclared);
    let (refused, exit) = place(&host, &root, &undeclared, false);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "pack_invalid",
        "{refused}"
    );
    let cause = values_of(&refused, "standard_pack_root_invalid")
        .unwrap_or_else(|| panic!("no standard_pack_root_invalid in {refused}"));
    assert_eq!(
        cause["standard_pack_problem"], "notice_undeclared",
        "{cause}"
    );
    let canonical = fs::canonicalize(&undeclared).expect("the canonical bundle");
    assert_eq!(
        cause["path"],
        json!(canonical.join("interfaces.json").to_string_lossy()),
        "{cause}"
    );
    assert!(calls_of(&host, "pause ").is_empty());
    assert_eq!(journals(&root).len(), before.len());
    assert!(tree(&game_root).is_empty(), "{:?}", tree(&game_root).keys());

    let taken = temp.path().join("neutral-taken.zip");
    Bundle::new(Some(released(json!(NOTICES_PACKAGE))))
        .with_notice("NOTICE.md", NOTICE)
        .write(&taken);
    let (planned, exit) = place(&host, &root, &taken, true);
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
    let before = journals(&root);
    let (placed, exit) = place(&host, &root, &taken, false);
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    let journal = new_journal(&root, &before);
    assert_eq!(
        journal["plan"]["package"],
        json!(NOTICES_PACKAGE),
        "{journal}"
    );
    assert_eq!(
        tree(&game_root.join("notices")),
        BTreeMap::from([("NOTICE.md".to_owned(), NOTICE.to_vec())])
    );
}
