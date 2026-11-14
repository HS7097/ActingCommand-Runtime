// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V1 (register §1.378; contracts/package-variables.md, "Interface";
// contracts/component-interfaces.md, the `package` row 14; coordinator work order
// `rd-1114/brief-463-v1-package-variables.md` and the coordinator's ruling of 11-14 that row 13 is
// Workflow #463 K1's): a bundle whose packages declare variables declares `package` [14, 14],
// which this build reads and an installer whose range ends below 14 refuses at its edge with
// nothing held or written.
// - `v1_the_build_declares_package_through_revision_14`: the declaration this build copies into
//   its manifests (`distribution/windows/component-interfaces.json`) reads `package` from 3 to the
//   contract crate's `VARIABLES_PACKAGE_REVISION`, 14;
// - `v1_a_bundle_at_revision_14_is_planned_here_and_refused_by_an_earlier_installer`: on an
//   installer of [3, 14] the bundle that declares [14, 14] is planned (its pack and `bundle.json`
//   new, exit 0, no pause); on one of [3, 13] the same bundle is refused `interface_edge` at its
//   `interfaces.json` before the hold: no pause, no journal, nothing in the game root;
// - fix round 1 (`rd-1114/brief-1723-fix1.md`, item 4),
//   `v1_a_bundle_whose_packs_declare_variables_below_revision_14_is_refused_variable_undeclared`:
//   on this build's installer, the same bundle declaring [3, 13] is refused `pack_invalid` caused
//   by `variable_undeclared` at its `interfaces.json`, naming the pack, before the hold, as a
//   bundle that carries `goals.json` and declares less than 12 is refused `goal_undeclared`.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep; every root is a temporary folder with neutral files. `place` reads of a pack's control
// only whether it declares `variables` (fix round 1): what the variables declare is admission's.

use super::fixture::{BuildSpec, C1, FakeHost, code, live_root, run, speaks, word, write_json};
use super::manifest::parse_declaration;
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_contract::{StandardPackProblem, VARIABLES_PACKAGE_REVISION};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The declaration this build copies into its Runtime and Tools manifests.
const OWN_DECLARATION: &str =
    include_str!("../../../../distribution/windows/component-interfaces.json");
/// The initiator the tests run the module as.
const WORD: Initiator = Initiator::LabRescue;
/// The neutral game and pack of the bundle (made-up ids, no real game).
const GAME: &str = "neutral-game";
const PACK: &str = "neutral.pack";
/// The `package` range of this build's installer: up to the revision a bundle whose packages
/// declare variables declares.
const INSTALLER_PACKAGE: [u64; 2] = [3, VARIABLES_PACKAGE_REVISION];
/// The `package` range of an installer before this slice: up to the revision just below.
const EARLIER_INSTALLER_PACKAGE: [u64; 2] = [3, VARIABLES_PACKAGE_REVISION - 1];
/// What a bundle whose packages declare variables declares.
const VARIABLES_BUNDLE_PACKAGE: [u64; 2] = [VARIABLES_PACKAGE_REVISION, VARIABLES_PACKAGE_REVISION];
/// Fix round 1: what a bundle whose packages declare variables and that does not declare their
/// revision declares, within this build's installer's range.
const UNDECLARED_BUNDLE_PACKAGE: [u64; 2] = [3, VARIABLES_PACKAGE_REVISION - 1];
/// A made-up control declaration with one variable; `place` reads only that it declares
/// `variables` (fix round 1).
const CONTROL: &[u8] = br#"{"package_id": "neutral.pack", "variables": [{"name": "extra", "type": "switch", "title": {"en": "Extra"}, "default": false}]}"#;

#[test]
fn v1_the_build_declares_package_through_revision_14() {
    let declared: Value =
        serde_json::from_str(OWN_DECLARATION).expect("the build's declaration is JSON");
    let speaks = parse_declaration(&declared).expect("the build's declaration follows the rules");
    assert_eq!(speaks.get("package"), Some(&[3, 14]));
    assert_eq!(
        speaks.get("package").map(|range| range[1]),
        Some(VARIABLES_PACKAGE_REVISION)
    );
}

/// Writes the neutral bundle zip of [`GAME`] to `path`: its `bundle.json`, an `interfaces.json`
/// that declares `package` `package` ([`VARIABLES_BUNDLE_PACKAGE`] for a bundle that declares
/// the revision), and the pack [`PACK`], every member stored.
fn write_bundle(path: &Path, package: [u64; 2]) {
    let mut declared = speaks();
    declared["package"] = json!(package);
    let interfaces = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.component-interfaces.v1",
        "speaks": declared,
    }))
    .expect("JSON");
    let index = serde_json::to_vec(&json!({
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
    .expect("JSON");
    let entries = [
        ("bundle.json".to_owned(), index),
        ("interfaces.json".to_owned(), interfaces),
        (format!("packs/{PACK}/control.json"), CONTROL.to_vec()),
    ];
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

/// `place` of the bundle `zip` on `root`, as [`WORD`], only its plan when `plan_only`; its one
/// line and exit code.
fn place(host: &FakeHost, root: &Path, zip: &Path, plan_only: bool) -> (Value, u8) {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    let location = rescue::location_of(root, &config, &state_root);
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

/// How many journals the root holds.
fn journal_count(root: &Path) -> usize {
    fs::read_dir(root.join("install").join("operations"))
        .expect("the journals")
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("op-"))
        .count()
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

/// A root whose configuration names the standard-pack root `<base>\v1-packs`, adopted, with the
/// empty game root of [`GAME`] in it; the installer declares `installer`, and the fake Runtime
/// runs one instance `neutral.a` bound to [`GAME`]. The root and the game root.
fn prepared(base: &Path, host: &FakeHost, installer: [u64; 2]) -> (PathBuf, PathBuf) {
    let root = live_root(base, "v1", &BuildSpec::new(C1));
    let packs = base.join("v1-packs");
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

// The `package` edge at revision 14, as `goals_bundle_is_refused_by_an_installer_before_revision_12`
// holds revision 12: this build's installer plans the bundle; an installer whose range ends at 13
// refuses it `interface_edge` at its `interfaces.json`, in the plan, and nothing is held or
// written.
#[test]
fn v1_a_bundle_at_revision_14_is_planned_here_and_refused_by_an_earlier_installer() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _game_root) = prepared(temp.path(), &host, INSTALLER_PACKAGE);
    let zip = temp.path().join("neutral-variables.zip");
    write_bundle(&zip, VARIABLES_BUNDLE_PACKAGE);
    let (planned, exit) = place(&host, &root, &zip, true);
    assert_eq!(exit, 0, "{planned}");
    assert_eq!(
        items(&planned),
        ["neutral.pack pack new", "bundle.json root new"],
        "{planned}"
    );
    assert!(calls_of(&host, "pause ").is_empty());

    let earlier = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&earlier.path().join("work"));
    let (root, game_root) = prepared(earlier.path(), &host, EARLIER_INSTALLER_PACKAGE);
    let before = journal_count(&root);
    let zip = earlier.path().join("neutral-variables.zip");
    write_bundle(&zip, VARIABLES_BUNDLE_PACKAGE);
    let (refused, exit) = place(&host, &root, &zip, false);
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
    assert!(
        calls_of(&host, "pause ").is_empty(),
        "{:?}",
        host.runtime.calls.borrow()
    );
    assert_eq!(journal_count(&root), before);
    assert!(
        fs::read_dir(&game_root)
            .expect("the game root")
            .next()
            .is_none(),
        "the game root holds nothing"
    );
}

/// The values of the first outcome with `code` anywhere in `line` (an outcome or one of its
/// causes).
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

// Fix round 1 (`rd-1114/brief-1723-fix1.md`, item 4), as
// `goals_bundle_that_does_not_declare_revision_12_is_refused_goal_undeclared` holds
// `goal_undeclared`: on this build's installer ([3, 14]) the bundle whose pack declares variables
// and that declares [3, 13], within the installer's range, is refused `pack_invalid` caused by
// `standard_pack_root_invalid` with `variable_undeclared` at its `interfaces.json`, the pointer
// empty, naming the pack, in the plan: no pause, no journal, nothing in the game root.
#[test]
fn v1_a_bundle_whose_packs_declare_variables_below_revision_14_is_refused_variable_undeclared() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host, INSTALLER_PACKAGE);
    let before = journal_count(&root);
    let zip = temp.path().join("neutral-undeclared.zip");
    write_bundle(&zip, UNDECLARED_BUNDLE_PACKAGE);
    let (refused, exit) = place(&host, &root, &zip, false);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        refused["outcome"]["code"], "install_command_refused",
        "{refused}"
    );
    assert_eq!(
        refused["outcome"]["values"]["install_refusal"], "pack_invalid",
        "{refused}"
    );
    let cause = values_of(&refused, "standard_pack_root_invalid")
        .unwrap_or_else(|| panic!("no standard_pack_root_invalid in {refused}"));
    let canonical = fs::canonicalize(&zip).expect("the canonical bundle");
    assert_eq!(
        cause["standard_pack_problem"],
        StandardPackProblem::VariableUndeclared.as_str(),
        "{cause}"
    );
    assert_eq!(
        cause["path"],
        json!(canonical.join("interfaces.json").to_string_lossy()),
        "{cause}"
    );
    assert_eq!(cause["json_pointer"], "", "{cause}");
    assert_eq!(cause["game"], GAME, "{cause}");
    assert_eq!(cause["package_id"], PACK, "{cause}");
    assert!(
        calls_of(&host, "pause ").is_empty(),
        "{:?}",
        host.runtime.calls.borrow()
    );
    assert_eq!(journal_count(&root), before);
    assert!(
        fs::read_dir(&game_root)
            .expect("the game root")
            .next()
            .is_none(),
        "the game root holds nothing"
    );
}
