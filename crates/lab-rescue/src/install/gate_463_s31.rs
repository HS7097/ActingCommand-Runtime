// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-1 (register §1.408; contracts/install-cli.md, "The Lab's rescue module", "The
// packages a placement makes enter or leave scheduling"; coordinator work order
// `rd-1115/brief-463-s31-place-plan.md`; design `rd-1115/pack-stage3-design.md` §4.2, S3-1): the
// rescue module's `place --plan` answers in its plan's `schedule_changes` each package of the
// bundle's game whose part in scheduling or whose content the placement changes (`before`,
// `after`, `content`), and the sorted package ids that `enters`, `leaves` and changes
// `content_only`, the installed game root against the zip's root, both classified by the
// contract crate's own functions:
// - `s31_a_new_pack_with_a_rule_enters`: a pack the installed root does not hold, named by a rule
//   of the zip, enters from `absent`;
// - `s31_a_lifted_hold_enters_from_held`: a held pack that the zip names by a rule and holds no
//   more enters, its `before` `held` with the entry's reason and reference;
// - `s31_an_added_hold_leaves`: a scheduled pack that the zip holds leaves;
// - `s31_a_replaced_pack_with_its_rule_unchanged_is_content_only`: a scheduled pack whose content
//   the zip replaces, its rule unchanged, changes only its content;
// - `s31_a_new_interrupt_handler_enters_by_its_role`: a pack that the zip's `interrupts.json`
//   makes a handler enters by that role, the feature package of `pages.json` unchanged;
// - `s31_a_games_first_root_enters_every_scheduled_pack`: on an empty game root every pack a rule
//   names enters, a held one and one in no set do not;
// - `s31_an_unchanged_bundle_lists_nothing`: a zip that is the installed root, every item
//   unchanged, lists no package.
// Each plan leaves the game root as it was and pauses nothing. The machine is the fake host of
// `fixture.rs` with its fake Runtime: no device, no sleep, no clock read; every root is a
// temporary folder with neutral files (made-up ids, no real game).

use super::fixture::{
    BuildSpec, C1, FakeHost, REPOSITORY, code, live_root, run, tree, word, write_json,
};
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
/// The neutral game and its one server.
const GAME: &str = "neutral-game";
const SERVER: &str = "neutral-server";
/// The `package` range the installer of these tests declares, and the one every bundle declares
/// (`package` alone, as a resource repository's release writes it).
const INSTALLER_PACKAGE: [u64; 2] = [3, 10];
const RELEASED: [u64; 2] = [3, 3];
/// The packs of the roots.
const KEPT: &str = "neutral.kept";
const ADDED: &str = "neutral.added";
const WAITING: &str = "neutral.waiting";
const FEATURE: &str = "neutral.feature";
const HANDLER: &str = "neutral.handler";
const LOOSE: &str = "neutral.loose";
/// Two contents of a pack's `control.json`.
const CONTROL: &[u8] = br#"{"neutral": 1}"#;
const CONTROL_REPLACED: &[u8] = br#"{"neutral": 2}"#;
/// A held entry's reason and reference.
const REASON: &str = "not yet run on a device";
const REFERENCE: &str = "board-reference-1";

/// One game root's content, as a bundle zip carries it and as an installed game root holds it.
#[derive(Default)]
struct Layout {
    /// Each pack's id and its `control.json`, in `bundle.json`'s order.
    packs: Vec<(&'static str, &'static [u8])>,
    /// `schedule.json`'s rules, each `(rule id, package id)`.
    rules: Vec<(&'static str, &'static str)>,
    /// `schedule.json`'s held packages, each with [`REASON`] and [`REFERENCE`].
    held: Vec<&'static str>,
    /// `pages.json`'s one feature package, when the root holds the file.
    feature: Option<&'static str>,
    /// `interrupts.json`'s one handler of the overlay page, when the root holds the file (beside
    /// `pages.json`).
    handler: Option<&'static str>,
}

impl Layout {
    fn pack(mut self, package_id: &'static str, control: &'static [u8]) -> Self {
        self.packs.push((package_id, control));
        self
    }

    fn rule(mut self, rule_id: &'static str, package_id: &'static str) -> Self {
        self.rules.push((rule_id, package_id));
        self
    }

    fn held(mut self, package_id: &'static str) -> Self {
        self.held.push(package_id);
        self
    }

    fn feature(mut self, package_id: &'static str) -> Self {
        self.feature = Some(package_id);
        self
    }

    fn handler(mut self, package_id: &'static str) -> Self {
        self.handler = Some(package_id);
        self
    }

    /// The root files, each `(name, bytes)`: `bundle.json`, `schedule.json`, and `pages.json` and
    /// `interrupts.json` when given.
    fn root_files(&self) -> Vec<(&'static str, Vec<u8>)> {
        let to_bytes = |value: Value| serde_json::to_vec(&value).expect("JSON");
        let packs = self
            .packs
            .iter()
            .map(|(package_id, control)| {
                json!({
                    "package_id": package_id,
                    "server": SERVER,
                    "entry_task_id": format!("{package_id}.entry"),
                    "path": format!("packs/{package_id}"),
                    "file_count": 1,
                    "byte_count": control.len(),
                })
            })
            .collect::<Vec<_>>();
        let rules = self
            .rules
            .iter()
            .map(|(rule_id, package_id)| json!({"id": rule_id, "package_id": package_id}))
            .collect::<Vec<_>>();
        let held = self
            .held
            .iter()
            .map(|package_id| {
                json!({"package_id": package_id, "reason": REASON, "reference": REFERENCE})
            })
            .collect::<Vec<_>>();
        let mut files = vec![
            (
                "bundle.json",
                to_bytes(json!({
                    "schema_version": "actingcommand.bundle.v4",
                    "game": GAME,
                    "packs": packs,
                })),
            ),
            (
                "schedule.json",
                to_bytes(json!({
                    "schema_version": "actingcommand.schedule.v1",
                    "rules": rules,
                    "held": held,
                })),
            ),
        ];
        if let Some(feature) = self.feature {
            files.push((
                "pages.json",
                to_bytes(json!({
                    "schema_version": "actingcommand.pages.v1",
                    "game": GAME,
                    "features": [feature],
                    "pages": [
                        {"id": "home", "kind": "page"},
                        {"id": "notice", "kind": "overlay"}
                    ],
                })),
            ));
        }
        if let Some(handler) = self.handler {
            files.push((
                "interrupts.json",
                to_bytes(json!({
                    "schema_version": "actingcommand.interrupts.v1",
                    "game": GAME,
                    "interrupts": [
                        {"page": "notice", "handlers": [handler], "then": "resume", "max_per_run": 1}
                    ],
                })),
            ));
        }
        files
    }

    /// Lays the root out as an installed game root: each pack's `<game root>\<id>\control.json`,
    /// each root file.
    fn lay_out(&self, game_root: &Path) {
        for (package_id, control) in &self.packs {
            fs::create_dir_all(game_root.join(package_id)).expect("a pack folder");
            fs::write(game_root.join(package_id).join("control.json"), control).expect("a pack");
        }
        for (name, bytes) in self.root_files() {
            fs::write(game_root.join(name), bytes).expect("a root file");
        }
    }

    /// Writes the bundle zip of the root to `path`, every member stored: the root files, its
    /// `interfaces.json` declaring [`RELEASED`] and each pack's `packs/<id>/control.json`.
    fn write_zip(&self, path: &Path) {
        let mut entries = self
            .root_files()
            .into_iter()
            .map(|(name, bytes)| (name.to_owned(), bytes))
            .collect::<Vec<_>>();
        entries.push((
            "interfaces.json".to_owned(),
            serde_json::to_vec(&json!({
                "schema_version": "actingcommand.component-interfaces.v1",
                "speaks": {"package": RELEASED},
                "validated_with": {"repository": REPOSITORY, "commit": C1},
            }))
            .expect("JSON"),
        ));
        for (package_id, control) in &self.packs {
            entries.push((format!("packs/{package_id}/control.json"), control.to_vec()));
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

/// A root whose configuration names the standard-pack root `<base>\s31-packs`, adopted, with the
/// empty game root of [`GAME`] in it; the installer declares [`INSTALLER_PACKAGE`], and the fake
/// Runtime runs one instance `neutral.a` bound to [`GAME`]. The root and the game root.
fn prepared(base: &Path, host: &FakeHost) -> (PathBuf, PathBuf) {
    let root = live_root(base, "s31", &BuildSpec::new(C1));
    let packs = base.join("s31-packs");
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

/// `place <zip> --plan` on `root` as [`WORD`]: its one line, after asserting that it exits 0,
/// that the game root `game_root` is byte for byte as it was and that nothing was paused.
fn plan(host: &FakeHost, root: &Path, game_root: &Path, zip: &Path) -> Value {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    let location = rescue::location_of(root, &config, &state_root);
    let context = Context {
        host,
        location: &location,
        initiator: WORD,
    };
    let mut params = Map::new();
    params.insert("bundle".to_owned(), json!(word(zip)));
    params.insert("plan".to_owned(), json!(true));
    let before = tree(game_root);
    let (line, exit) = match rescue::answer_with(&context, "place", &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("place: a line, not {other:?}"),
    };
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(tree(game_root), before, "the plan changed the game root");
    assert!(
        host.runtime
            .calls
            .borrow()
            .iter()
            .all(|call| !call.starts_with("pause ")),
        "{:?}",
        host.runtime.calls.borrow()
    );
    line
}

/// The plan's `schedule_changes` of placing `after` on a game root laid out as `before` (an empty
/// game root when `None`).
fn schedule_changes(before: Option<&Layout>, after: &Layout) -> Value {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    if let Some(installed) = before {
        installed.lay_out(&game_root);
    }
    let zip = temp.path().join("neutral-bundle.zip");
    after.write_zip(&zip);
    plan(&host, &root, &game_root, &zip)["plan"]["schedule_changes"].clone()
}

// A pack the installed root does not hold, named by a rule of the zip: it enters, from `absent`,
// its content `new`; the pack both roots schedule with the same content is not listed.
#[test]
fn s31_a_new_pack_with_a_rule_enters() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.added", ADDED);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [{
                "package_id": ADDED,
                "before": {"state": "absent"},
                "after": {"state": "scheduled"},
                "content": "new",
            }],
            "enters": [ADDED],
            "leaves": [],
            "content_only": [],
        })
    );
}

// A held pack that the zip names by a rule and holds no more: it enters, its `before` `held` with
// the entry's reason and reference, its content unchanged.
#[test]
fn s31_a_lifted_hold_enters_from_held() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(WAITING, CONTROL)
        .rule("rule.kept", KEPT)
        .held(WAITING);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(WAITING, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.waiting", WAITING);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [{
                "package_id": WAITING,
                "before": {"state": "held", "reason": REASON, "reference": REFERENCE},
                "after": {"state": "scheduled"},
                "content": "unchanged",
            }],
            "enters": [WAITING],
            "leaves": [],
            "content_only": [],
        })
    );
}

// A scheduled pack that the zip holds, its rule kept: it leaves, its `after` `held` with the
// entry's reason and reference; nothing enters.
#[test]
fn s31_an_added_hold_leaves() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(WAITING, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.waiting", WAITING);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(WAITING, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.waiting", WAITING)
        .held(WAITING);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [{
                "package_id": WAITING,
                "before": {"state": "scheduled"},
                "after": {"state": "held", "reason": REASON, "reference": REFERENCE},
                "content": "unchanged",
            }],
            "enters": [],
            "leaves": [WAITING],
            "content_only": [],
        })
    );
}

// A scheduled pack whose `control.json` the zip replaces, its rule unchanged: scheduled before and
// after, its content `replace`, listed in `content_only` alone.
#[test]
fn s31_a_replaced_pack_with_its_rule_unchanged_is_content_only() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL_REPLACED)
        .rule("rule.kept", KEPT);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [{
                "package_id": KEPT,
                "before": {"state": "scheduled"},
                "after": {"state": "scheduled"},
                "content": "replace",
            }],
            "enters": [],
            "leaves": [],
            "content_only": [KEPT],
        })
    );
}

// A new pack that the zip's new `interrupts.json` makes a handler: it enters by that role
// (`maintenance_role`, its role `interrupts.json`), no rule naming it; the feature package of
// `pages.json`, which both roots hold, and the scheduled pack are not listed.
#[test]
fn s31_a_new_interrupt_handler_enters_by_its_role() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FEATURE, CONTROL)
        .rule("rule.kept", KEPT)
        .feature(FEATURE);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FEATURE, CONTROL)
        .pack(HANDLER, CONTROL)
        .rule("rule.kept", KEPT)
        .feature(FEATURE)
        .handler(HANDLER);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [{
                "package_id": HANDLER,
                "before": {"state": "absent"},
                "after": {"state": "maintenance_role", "roles": ["interrupts.json"]},
                "content": "new",
            }],
            "enters": [HANDLER],
            "leaves": [],
            "content_only": [],
        })
    );
}

// A game's first root (the game root empty): every pack a rule names enters; the held pack and the
// pack in no set are listed, from `absent`, and do not.
#[test]
fn s31_a_games_first_root_enters_every_scheduled_pack() {
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, CONTROL)
        .pack(WAITING, CONTROL)
        .pack(LOOSE, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.added", ADDED)
        .held(WAITING);
    assert_eq!(
        schedule_changes(None, &after),
        json!({
            "packages": [
                {
                    "package_id": ADDED,
                    "before": {"state": "absent"},
                    "after": {"state": "scheduled"},
                    "content": "new",
                },
                {
                    "package_id": KEPT,
                    "before": {"state": "absent"},
                    "after": {"state": "scheduled"},
                    "content": "new",
                },
                {
                    "package_id": LOOSE,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified"},
                    "content": "new",
                },
                {
                    "package_id": WAITING,
                    "before": {"state": "absent"},
                    "after": {"state": "held", "reason": REASON, "reference": REFERENCE},
                    "content": "new",
                },
            ],
            "enters": [ADDED, KEPT],
            "leaves": [],
            "content_only": [],
        })
    );
}

// A zip that is the installed root (a scheduled, a held, a feature and a handler pack): every item
// of the plan unchanged, and `schedule_changes` lists nothing.
#[test]
fn s31_an_unchanged_bundle_lists_nothing() {
    let layout = || {
        Layout::default()
            .pack(KEPT, CONTROL)
            .pack(WAITING, CONTROL)
            .pack(FEATURE, CONTROL)
            .pack(HANDLER, CONTROL)
            .rule("rule.kept", KEPT)
            .held(WAITING)
            .feature(FEATURE)
            .handler(HANDLER)
    };
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    layout().lay_out(&game_root);
    let zip = temp.path().join("neutral-bundle.zip");
    layout().write_zip(&zip);
    let line = plan(&host, &root, &game_root, &zip);
    let states = line["plan"]["items"]
        .as_array()
        .expect("the plan's items")
        .iter()
        .map(|item| item["state"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(states.len(), 8, "{line}");
    assert!(states.iter().all(|state| *state == "unchanged"), "{line}");
    assert_eq!(
        line["plan"]["schedule_changes"],
        json!({"packages": [], "enters": [], "leaves": [], "content_only": []}),
        "{line}"
    );
}
