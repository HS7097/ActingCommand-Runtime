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
//   makes a handler enters by that role (`handler`, fix round 2), the feature package of
//   `pages.json` unchanged;
// - `s31_a_games_first_root_enters_every_scheduled_pack`: on an empty game root every pack a rule
//   names enters, a held one and one in no set do not;
// - `s31_an_unchanged_bundle_lists_nothing`: a zip that is the installed root, every item
//   unchanged, lists no package.
// Fix round 2 (coordinator work order `rd-1115/brief-463-s31-fix2.md`, the review's items 1 to 6):
// `enters` is exactly the packages the scheduler dispatches, or uses in a supporting role, after
// the placement and did not before:
// - `s31_fix2_an_installed_root_without_schedule_json_counts_nothing_before` (item 1): the
//   installed root composes nothing, so the zip's scheduled pack and its role package whose role
//   is unchanged both enter;
// - `s31_fix2_a_zip_without_schedule_json_enters_nothing` (item 1): the zip composes nothing, so
//   its new handler does not enter, and the packs used before leave;
// - `s31_fix2_a_refused_installed_schedule_counts_nothing_before` (item 2): the installed
//   `schedule.json` the reader refuses makes that side unread, and every pack the zip schedules
//   or gives a role enters, its role unchanged or not;
// - `s31_fix2_a_refused_zip_schedule_answers_no_list` (item 3): the zip's `schedule.json` the
//   reader refuses makes that side unread: `packages` and `unread`, and none of the three lists;
// - `s31_fix2_a_schedule_the_composer_refuses_is_unread` (item 4): a rule of the zip naming a
//   package the zip does not carry is the composer's refusal, and the side is unread as in item 3;
// - `s31_fix2_a_role_gained_inside_one_file_enters` (item 5): a `startup` entry that gains
//   `return_home`, an edge package added to the recovery chain and a handler that also becomes
//   `then.run` enter, each with readiness's words;
// - `s31_fix2_a_pack_the_zip_no_longer_carries_leaves` (item 6): a scheduled pack and a pack in no
//   set that the zip no longer carries leave, with no `content`.
// Fix round 3 (coordinator work order `rd-1115/brief-463-s31-fix3.md`, the second review): a
// package that is a layer of the prerequisite chain (`control.json`'s `prerequisite_package_id`)
// of a package that is scheduled or holds a role is used, with the plan's own role word
// `prerequisite`:
// - `s31_fix3_a_new_prerequisite_of_a_new_scheduled_pack_enters_with_it`: both new packs enter;
// - `s31_fix3_an_unclassified_pack_that_becomes_a_prerequisite_enters`: the scheduled pack whose
//   `control.json` now names it is `content_only`, the prerequisite enters;
// - `s31_fix3_the_prerequisite_of_a_held_pack_is_not_used`: nothing enters;
// - `s31_fix3_a_chain_two_deep_enters_whole`: the scheduled pack and both layers enter;
// - `s31_fix3_a_prerequisite_whose_owner_leaves_leaves`: the pack that loses its rule and its
//   prerequisite both leave;
// - `s31_fix3_an_installed_root_the_reader_refuses_counts_nothing_before` (the second review's
//   point C): an installed `bundle.json` the root reader refuses leaves the installed side unread
//   as a whole, and every pack the zip schedules or gives a role enters.
// Workflow #463 S3-11 (the review of S3-1's third round, point 3; tests only):
// - `s311_the_chain_of_a_package_holding_only_a_role_is_counted`: a package that only holds a role
//   (`startup`) and is not scheduled starts a chain: its new prerequisite enters with it;
// - `s311_the_fourth_layer_of_a_chain_is_not_counted`: of a chain four deep, the first three
//   layers enter with the role `prerequisite`, the fourth is listed with no role and does not.
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
const STARTER: &str = "neutral.starter";
const EDGE: &str = "neutral.edge";
/// Fix round 3: the first and the second layer of a prerequisite chain.
const FIRST: &str = "neutral.first";
const SECOND: &str = "neutral.second";
/// Workflow #463 S3-11: the third layer of a chain, and a fourth, which is not counted.
const THIRD: &str = "neutral.third";
const FOURTH: &str = "neutral.fourth";
/// A package no bundle carries.
const GHOST: &str = "neutral.ghost";
/// Two contents of a pack's `control.json`.
const CONTROL: &[u8] = br#"{"neutral": 1}"#;
const CONTROL_REPLACED: &[u8] = br#"{"neutral": 2}"#;
/// Fix round 3: a pack's `control.json` that names [`FIRST`], and one that names [`SECOND`], as
/// its prerequisite package.
const NEEDS_FIRST: &[u8] = br#"{"prerequisite_package_id": "neutral.first"}"#;
const NEEDS_SECOND: &[u8] = br#"{"prerequisite_package_id": "neutral.second"}"#;
/// Workflow #463 S3-11: a `control.json` that names [`THIRD`], and one that names [`FOURTH`].
const NEEDS_THIRD: &[u8] = br#"{"prerequisite_package_id": "neutral.third"}"#;
const NEEDS_FOURTH: &[u8] = br#"{"prerequisite_package_id": "neutral.fourth"}"#;
/// Fix round 3: a `bundle.json` that is no JSON document.
const BUNDLE_BROKEN: &[u8] = b"{";
/// A held entry's reason and reference.
const REASON: &str = "not yet run on a device";
const REFERENCE: &str = "board-reference-1";
/// A `schedule.json` that is no JSON document.
const SCHEDULE_BROKEN: &[u8] = b"{";

/// What a root's `schedule.json` is.
#[derive(Default, Clone, Copy)]
enum ScheduleFile {
    /// The layout's rules and held entries.
    #[default]
    Written,
    /// None: the root holds no `schedule.json`.
    Absent,
    /// These bytes.
    Bytes(&'static [u8]),
}

/// One game root's content, as a bundle zip carries it and as an installed game root holds it.
#[derive(Default)]
struct Layout {
    /// Each pack's id and its `control.json`, in `bundle.json`'s order.
    packs: Vec<(&'static str, &'static [u8])>,
    /// `bundle.json`'s `maintenance` entries, each `(package id, uses)` of [`SERVER`].
    maintenance: Vec<(&'static str, &'static [&'static str])>,
    /// `schedule.json`'s rules, each `(rule id, package id)`.
    rules: Vec<(&'static str, &'static str)>,
    /// `schedule.json`'s held packages, each with [`REASON`] and [`REFERENCE`].
    held: Vec<&'static str>,
    /// What `schedule.json` is.
    schedule: ScheduleFile,
    /// `pages.json`'s one feature package, when the root holds the file.
    feature: Option<&'static str>,
    /// `interrupts.json`'s one handler of the overlay page, when the root holds the file (beside
    /// `pages.json`).
    handler: Option<&'static str>,
    /// That interrupt's one `then.run` package (`resume` without one).
    then_run: Option<&'static str>,
    /// `navigation.json`'s one edge's package (from the page to the overlay), when the root holds
    /// the file (beside `pages.json`, with [`Layout::chain`]).
    edge: Option<&'static str>,
    /// `navigation.json`'s recovery chain.
    chain: Vec<&'static str>,
}

impl Layout {
    fn pack(mut self, package_id: &'static str, control: &'static [u8]) -> Self {
        self.packs.push((package_id, control));
        self
    }

    fn maintenance(mut self, package_id: &'static str, uses: &'static [&'static str]) -> Self {
        self.maintenance.push((package_id, uses));
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

    fn schedule(mut self, schedule: ScheduleFile) -> Self {
        self.schedule = schedule;
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

    fn then_run(mut self, package_id: &'static str) -> Self {
        self.then_run = Some(package_id);
        self
    }

    fn edge(mut self, package_id: &'static str) -> Self {
        self.edge = Some(package_id);
        self
    }

    fn chain(mut self, package_id: &'static str) -> Self {
        self.chain.push(package_id);
        self
    }

    /// The root files, each `(name, bytes)`: `bundle.json`, `schedule.json` (as
    /// [`Layout::schedule`] says), and `pages.json`, `interrupts.json` and `navigation.json` when
    /// given.
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
        let mut bundle = json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": packs,
        });
        if !self.maintenance.is_empty() {
            bundle["maintenance"] = json!(
                self.maintenance
                    .iter()
                    .map(|(package_id, uses)| {
                        json!({"package_id": package_id, "server": SERVER, "uses": uses})
                    })
                    .collect::<Vec<_>>()
            );
        }
        let mut files = vec![("bundle.json", to_bytes(bundle))];
        match self.schedule {
            ScheduleFile::Written => files.push((
                "schedule.json",
                to_bytes(json!({
                    "schema_version": "actingcommand.schedule.v1",
                    "rules": rules,
                    "held": held,
                })),
            )),
            ScheduleFile::Absent => {}
            ScheduleFile::Bytes(bytes) => files.push(("schedule.json", bytes.to_vec())),
        }
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
            let then = match self.then_run {
                Some(package_id) => json!({"run": [package_id]}),
                None => json!("resume"),
            };
            files.push((
                "interrupts.json",
                to_bytes(json!({
                    "schema_version": "actingcommand.interrupts.v1",
                    "game": GAME,
                    "interrupts": [
                        {"page": "notice", "handlers": [handler], "then": then, "max_per_run": 1}
                    ],
                })),
            ));
        }
        if let Some(edge) = self.edge {
            files.push((
                "navigation.json",
                to_bytes(json!({
                    "schema_version": "actingcommand.navigation.v1",
                    "game": GAME,
                    "edges": [{"from": "home", "to": "notice", "package_id": edge}],
                    "recovery_chain": self.chain,
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
/// game root when `None`), with the game root and the zip's canonical path, which the plan's
/// failures name.
fn planned(before: Option<&Layout>, after: &Layout) -> (Value, PathBuf, PathBuf) {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    if let Some(installed) = before {
        installed.lay_out(&game_root);
    }
    let zip = temp.path().join("neutral-bundle.zip");
    after.write_zip(&zip);
    let changes = plan(&host, &root, &game_root, &zip)["plan"]["schedule_changes"].clone();
    let canonical = fs::canonicalize(&zip).expect("the zip's canonical path");
    (changes, game_root, canonical)
}

/// The plan's `schedule_changes` of [`planned`] alone.
fn schedule_changes(before: Option<&Layout>, after: &Layout) -> Value {
    planned(before, after).0
}

/// `changes` without its member `unread`, and that member with each failure as its outcome's code
/// and values, `[{root, code, values}]` (`null` when there is none).
fn without_unread(mut changes: Value) -> (Value, Value) {
    let unread = changes
        .as_object_mut()
        .and_then(|members| members.remove("unread"))
        .unwrap_or(Value::Null);
    let sides = unread.as_array().map(|sides| {
        sides
            .iter()
            .map(|side| {
                json!({
                    "root": side["root"],
                    "code": side["failure"]["outcome"]["code"],
                    "values": side["failure"]["outcome"]["values"],
                })
            })
            .collect::<Vec<_>>()
    });
    (changes, json!(sides))
}

/// The values of the `standard_pack_root_invalid` of the root file at `path` with `problem` at
/// `pointer`.
fn root_invalid(path: &Path, problem: &str, pointer: &str) -> Value {
    json!({
        "game": GAME,
        "path": path.to_string_lossy(),
        "standard_pack_problem": problem,
        "json_pointer": pointer,
    })
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
// (`maintenance_role`, its role `handler`, readiness's word since fix round 2), no rule naming it;
// the feature package of `pages.json`, which both roots hold, and the scheduled pack are not
// listed.
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
                "after": {"state": "maintenance_role", "roles": ["handler"]},
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

// Fix round 2, item 1: the installed root holds no `schedule.json`, so the scheduler composes
// nothing of it. The zip's rule makes its pack enter from `unclassified`, and the feature package,
// whose role `features` both roots give it, enters too: the scheduler starts using it.
#[test]
fn s31_fix2_an_installed_root_without_schedule_json_counts_nothing_before() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FEATURE, CONTROL)
        .feature(FEATURE)
        .schedule(ScheduleFile::Absent);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FEATURE, CONTROL)
        .rule("rule.kept", KEPT)
        .feature(FEATURE);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": FEATURE,
                    "before": {"state": "maintenance_role", "roles": ["features"]},
                    "after": {"state": "maintenance_role", "roles": ["features"]},
                    "content": "unchanged",
                },
                {
                    "package_id": KEPT,
                    "before": {"state": "unclassified"},
                    "after": {"state": "scheduled"},
                    "content": "unchanged",
                },
            ],
            "enters": [FEATURE, KEPT],
            "leaves": [],
            "content_only": [],
        })
    );
}

// Fix round 2, item 1: the zip carries no `schedule.json`, so the scheduler composes nothing of
// it. Its new handler is shown with its role and does not enter; the scheduled pack and the
// feature package, used before, leave.
#[test]
fn s31_fix2_a_zip_without_schedule_json_enters_nothing() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FEATURE, CONTROL)
        .rule("rule.kept", KEPT)
        .feature(FEATURE);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FEATURE, CONTROL)
        .pack(HANDLER, CONTROL)
        .feature(FEATURE)
        .handler(HANDLER)
        .schedule(ScheduleFile::Absent);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": FEATURE,
                    "before": {"state": "maintenance_role", "roles": ["features"]},
                    "after": {"state": "maintenance_role", "roles": ["features"]},
                    "content": "unchanged",
                },
                {
                    "package_id": HANDLER,
                    "before": {"state": "absent"},
                    "after": {"state": "maintenance_role", "roles": ["handler"]},
                    "content": "new",
                },
                {
                    "package_id": KEPT,
                    "before": {"state": "scheduled"},
                    "after": {"state": "unclassified"},
                    "content": "unchanged",
                },
            ],
            "enters": [],
            "leaves": [FEATURE, KEPT],
            "content_only": [],
        })
    );
}

// Fix round 2, item 2: the installed `schedule.json` is one the contract crate's reader refuses,
// so the installed side is unread and counts nothing before: the zip's scheduled pack and the
// feature package, its role unchanged, both enter; the side is named under `unread` with the
// reader's refusal at the installed file.
#[test]
fn s31_fix2_a_refused_installed_schedule_counts_nothing_before() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FEATURE, CONTROL)
        .rule("rule.kept", KEPT)
        .feature(FEATURE)
        .schedule(ScheduleFile::Bytes(SCHEDULE_BROKEN));
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FEATURE, CONTROL)
        .rule("rule.kept", KEPT)
        .feature(FEATURE);
    let (changes, game_root, _) = planned(Some(&before), &after);
    let (listed, unread) = without_unread(changes);
    assert_eq!(
        listed,
        json!({
            "packages": [
                {
                    "package_id": FEATURE,
                    "before": {"state": "unread", "roles": ["features"]},
                    "after": {"state": "maintenance_role", "roles": ["features"]},
                    "content": "unchanged",
                },
                {
                    "package_id": KEPT,
                    "before": {"state": "unread"},
                    "after": {"state": "scheduled"},
                    "content": "unchanged",
                },
            ],
            "enters": [FEATURE, KEPT],
            "leaves": [],
            "content_only": [],
        })
    );
    assert_eq!(
        unread,
        json!([{
            "root": "installed",
            "code": "standard_pack_root_invalid",
            "values": root_invalid(&game_root.join("schedule.json"), "schedule_invalid", ""),
        }])
    );
}

// Fix round 2, item 3: the zip's `schedule.json` is one the contract crate's reader refuses, so
// the zip's side is unread: the answer lists its packages, `unread` with the reader's refusal at
// the zip's file, and none of `enters`, `leaves` and `content_only`.
#[test]
fn s31_fix2_a_refused_zip_schedule_answers_no_list() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, CONTROL)
        .schedule(ScheduleFile::Bytes(SCHEDULE_BROKEN));
    let (changes, _, zip) = planned(Some(&before), &after);
    let (listed, unread) = without_unread(changes);
    assert_eq!(
        listed,
        json!({
            "packages": [
                {
                    "package_id": ADDED,
                    "before": {"state": "absent"},
                    "after": {"state": "unread"},
                    "content": "new",
                },
                {
                    "package_id": KEPT,
                    "before": {"state": "scheduled"},
                    "after": {"state": "unread"},
                    "content": "unchanged",
                },
            ],
        })
    );
    assert_eq!(
        unread,
        json!([{
            "root": "bundle",
            "code": "standard_pack_root_invalid",
            "values": root_invalid(&zip.join("schedule.json"), "schedule_invalid", ""),
        }])
    );
}

// Fix round 2, item 4: the zip's `schedule.json` is one the reader takes, but its third rule names
// a package the zip does not carry, which the composer's own reference check refuses: the zip's
// side is unread as in item 3, the refusal `rule_package_unregistered` at that rule's package.
#[test]
fn s31_fix2_a_schedule_the_composer_refuses_is_unread() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.added", ADDED)
        .rule("rule.ghost", GHOST);
    let (changes, _, zip) = planned(Some(&before), &after);
    let (listed, unread) = without_unread(changes);
    assert_eq!(
        listed,
        json!({
            "packages": [
                {
                    "package_id": ADDED,
                    "before": {"state": "absent"},
                    "after": {"state": "unread"},
                    "content": "new",
                },
                {
                    "package_id": KEPT,
                    "before": {"state": "scheduled"},
                    "after": {"state": "unread"},
                    "content": "unchanged",
                },
            ],
        })
    );
    assert_eq!(
        unread,
        json!([{
            "root": "bundle",
            "code": "standard_pack_root_invalid",
            "values": root_invalid(
                &zip.join("schedule.json"),
                "rule_package_unregistered",
                "/rules/2/package_id",
            ),
        }])
    );
}

// Fix round 2, item 5: roles gained inside one root file enter, each named by readiness's words:
// the `startup` entry of `bundle.json` that gains `return_home`, the edge package of
// `navigation.json` added to its recovery chain, and the handler of `interrupts.json` that also
// becomes its `then.run` package. The feature package, also in the chain before and after, and the
// scheduled pack are not listed.
#[test]
fn s31_fix2_a_role_gained_inside_one_file_enters() {
    let packs = || {
        Layout::default()
            .pack(KEPT, CONTROL)
            .pack(FEATURE, CONTROL)
            .pack(STARTER, CONTROL)
            .pack(EDGE, CONTROL)
            .pack(HANDLER, CONTROL)
            .rule("rule.kept", KEPT)
            .feature(FEATURE)
            .edge(EDGE)
            .chain(FEATURE)
            .handler(HANDLER)
    };
    let before = packs().maintenance(STARTER, &["startup"]);
    let after = packs()
        .maintenance(STARTER, &["startup", "return_home"])
        .chain(EDGE)
        .then_run(HANDLER);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": EDGE,
                    "before": {"state": "maintenance_role", "roles": ["edge"]},
                    "after": {"state": "maintenance_role", "roles": ["edge", "chain"]},
                    "content": "unchanged",
                },
                {
                    "package_id": HANDLER,
                    "before": {"state": "maintenance_role", "roles": ["handler"]},
                    "after": {"state": "maintenance_role", "roles": ["handler", "then_run"]},
                    "content": "unchanged",
                },
                {
                    "package_id": STARTER,
                    "before": {"state": "maintenance_role", "roles": ["startup"]},
                    "after": {
                        "state": "maintenance_role",
                        "roles": ["startup", "return_home"],
                    },
                    "content": "unchanged",
                },
            ],
            "enters": [EDGE, HANDLER, STARTER],
            "leaves": [],
            "content_only": [],
        })
    );
}

// Fix round 2, item 6: the zip no longer carries a scheduled pack and a pack in no set: both
// leave, `after` `absent` and no `content` (their folders stay; only `bundle.json` stops listing
// them); the pack the zip keeps is not listed.
#[test]
fn s31_fix2_a_pack_the_zip_no_longer_carries_leaves() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, CONTROL)
        .pack(LOOSE, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.added", ADDED);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": ADDED,
                    "before": {"state": "scheduled"},
                    "after": {"state": "absent"},
                },
                {
                    "package_id": LOOSE,
                    "before": {"state": "unclassified"},
                    "after": {"state": "absent"},
                },
            ],
            "enters": [],
            "leaves": [ADDED, LOOSE],
            "content_only": [],
        })
    );
}

// Fix round 3: a new pack that a rule of the zip names and whose `control.json` names a new pack
// as its prerequisite: both enter, the prerequisite in no set of the classification
// (`unclassified`) with the plan's role `prerequisite`; the pack both roots schedule is not listed.
#[test]
fn s31_fix3_a_new_prerequisite_of_a_new_scheduled_pack_enters_with_it() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, NEEDS_FIRST)
        .pack(FIRST, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.added", ADDED);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": ADDED,
                    "before": {"state": "absent"},
                    "after": {"state": "scheduled"},
                    "content": "new",
                },
                {
                    "package_id": FIRST,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified", "roles": ["prerequisite"]},
                    "content": "new",
                },
            ],
            "enters": [ADDED, FIRST],
            "leaves": [],
            "content_only": [],
        })
    );
}

// Fix round 3: a pack in no set on both roots becomes the prerequisite of the scheduled pack, whose
// replaced `control.json` now names it: the prerequisite enters by the role `prerequisite`, and the
// scheduled pack, used in the same way before and after, is `content_only`.
#[test]
fn s31_fix3_an_unclassified_pack_that_becomes_a_prerequisite_enters() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(FIRST, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, NEEDS_FIRST)
        .pack(FIRST, CONTROL)
        .rule("rule.kept", KEPT);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": FIRST,
                    "before": {"state": "unclassified"},
                    "after": {"state": "unclassified", "roles": ["prerequisite"]},
                    "content": "unchanged",
                },
                {
                    "package_id": KEPT,
                    "before": {"state": "scheduled"},
                    "after": {"state": "scheduled"},
                    "content": "replace",
                },
            ],
            "enters": [FIRST],
            "leaves": [],
            "content_only": [KEPT],
        })
    );
}

// Fix round 3: the zip's new pack is named by a rule and held, and its `control.json` names a new
// pack as its prerequisite: the held pack is not dispatched, so its prerequisite is not used;
// both are listed from `absent` and nothing enters.
#[test]
fn s31_fix3_the_prerequisite_of_a_held_pack_is_not_used() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(WAITING, NEEDS_FIRST)
        .pack(FIRST, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.waiting", WAITING)
        .held(WAITING);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": FIRST,
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
            "enters": [],
            "leaves": [],
            "content_only": [],
        })
    );
}

// Fix round 3: a chain two deep, the zip's new scheduled pack naming the first layer and the first
// layer naming the second: the scheduled pack and both layers enter, each layer with the role
// `prerequisite` (the first holds no other role, so the chain is followed through it).
#[test]
fn s31_fix3_a_chain_two_deep_enters_whole() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, NEEDS_FIRST)
        .pack(FIRST, NEEDS_SECOND)
        .pack(SECOND, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.added", ADDED);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": ADDED,
                    "before": {"state": "absent"},
                    "after": {"state": "scheduled"},
                    "content": "new",
                },
                {
                    "package_id": FIRST,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified", "roles": ["prerequisite"]},
                    "content": "new",
                },
                {
                    "package_id": SECOND,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified", "roles": ["prerequisite"]},
                    "content": "new",
                },
            ],
            "enters": [ADDED, FIRST, SECOND],
            "leaves": [],
            "content_only": [],
        })
    );
}

// Fix round 3: the installed scheduled pack whose `control.json` names a prerequisite loses its
// rule in the zip: it leaves, and its prerequisite, used before only as that, leaves too.
#[test]
fn s31_fix3_a_prerequisite_whose_owner_leaves_leaves() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, NEEDS_FIRST)
        .pack(FIRST, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.added", ADDED);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, NEEDS_FIRST)
        .pack(FIRST, CONTROL)
        .rule("rule.kept", KEPT);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": ADDED,
                    "before": {"state": "scheduled"},
                    "after": {"state": "unclassified"},
                    "content": "unchanged",
                },
                {
                    "package_id": FIRST,
                    "before": {"state": "unclassified", "roles": ["prerequisite"]},
                    "after": {"state": "unclassified"},
                    "content": "unchanged",
                },
            ],
            "enters": [],
            "leaves": [ADDED, FIRST],
            "content_only": [],
        })
    );
}

// Fix round 3 (the second review's point C, `Side::unreadable`): the installed `bundle.json` is no
// JSON document, which the root reader refuses, so the installed side cannot be read at all: each
// pack is `unread` before without roles, the zip's scheduled pack and its feature package enter,
// the pack in no set is listed and does not, `leaves` and `content_only` are empty, and the side is
// named under `unread` with the reader's refusal at the installed file.
#[test]
fn s31_fix3_an_installed_root_the_reader_refuses_counts_nothing_before() {
    let layout = || {
        Layout::default()
            .pack(KEPT, CONTROL)
            .pack(FEATURE, CONTROL)
            .pack(LOOSE, CONTROL)
            .rule("rule.kept", KEPT)
            .feature(FEATURE)
    };
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, game_root) = prepared(temp.path(), &host);
    layout().lay_out(&game_root);
    fs::write(game_root.join("bundle.json"), BUNDLE_BROKEN).expect("the installed bundle.json");
    let zip = temp.path().join("neutral-bundle.zip");
    layout().write_zip(&zip);
    let line = plan(&host, &root, &game_root, &zip);
    let (listed, unread) = without_unread(line["plan"]["schedule_changes"].clone());
    assert_eq!(
        listed,
        json!({
            "packages": [
                {
                    "package_id": FEATURE,
                    "before": {"state": "unread"},
                    "after": {"state": "maintenance_role", "roles": ["features"]},
                    "content": "unchanged",
                },
                {
                    "package_id": KEPT,
                    "before": {"state": "unread"},
                    "after": {"state": "scheduled"},
                    "content": "unchanged",
                },
                {
                    "package_id": LOOSE,
                    "before": {"state": "unread"},
                    "after": {"state": "unclassified"},
                    "content": "unchanged",
                },
            ],
            "enters": [FEATURE, KEPT],
            "leaves": [],
            "content_only": [],
        }),
        "{line}"
    );
    assert_eq!(
        unread,
        json!([{
            "root": "installed",
            "code": "standard_pack_root_invalid",
            "values": root_invalid(&game_root.join("bundle.json"), "malformed", ""),
        }]),
        "{line}"
    );
}

// Workflow #463 S3-11 (register §1.418; coordinator work order rd-1115/brief-463-s311-trial-run.md,
// the review of S3-1's third round, its point 3; tests only): a package that only holds a role and
// is not scheduled (a `startup` entry of `bundle.json`) starts a prerequisite chain: its new
// prerequisite enters with the plan's role `prerequisite`, beside the package entering by its role.
#[test]
fn s311_the_chain_of_a_package_holding_only_a_role_is_counted() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(STARTER, NEEDS_FIRST)
        .pack(FIRST, CONTROL)
        .rule("rule.kept", KEPT)
        .maintenance(STARTER, &["startup"]);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": FIRST,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified", "roles": ["prerequisite"]},
                    "content": "new",
                },
                {
                    "package_id": STARTER,
                    "before": {"state": "absent"},
                    "after": {"state": "maintenance_role", "roles": ["startup"]},
                    "content": "new",
                },
            ],
            "enters": [FIRST, STARTER],
            "leaves": [],
            "content_only": [],
        })
    );
}

// Workflow #463 S3-11 (the same point 3; tests only): a chain four deep from a new scheduled pack:
// its first three layers enter with the role `prerequisite` (the kernel's depth, three layers), and
// the fourth is listed from `absent` with no role and does not enter.
#[test]
fn s311_the_fourth_layer_of_a_chain_is_not_counted() {
    let before = Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT);
    let after = Layout::default()
        .pack(KEPT, CONTROL)
        .pack(ADDED, NEEDS_FIRST)
        .pack(FIRST, NEEDS_SECOND)
        .pack(SECOND, NEEDS_THIRD)
        .pack(THIRD, NEEDS_FOURTH)
        .pack(FOURTH, CONTROL)
        .rule("rule.kept", KEPT)
        .rule("rule.added", ADDED);
    assert_eq!(
        schedule_changes(Some(&before), &after),
        json!({
            "packages": [
                {
                    "package_id": ADDED,
                    "before": {"state": "absent"},
                    "after": {"state": "scheduled"},
                    "content": "new",
                },
                {
                    "package_id": FIRST,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified", "roles": ["prerequisite"]},
                    "content": "new",
                },
                {
                    "package_id": FOURTH,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified"},
                    "content": "new",
                },
                {
                    "package_id": SECOND,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified", "roles": ["prerequisite"]},
                    "content": "new",
                },
                {
                    "package_id": THIRD,
                    "before": {"state": "absent"},
                    "after": {"state": "unclassified", "roles": ["prerequisite"]},
                    "content": "new",
                },
            ],
            "enters": [ADDED, FIRST, SECOND, THIRD],
            "leaves": [],
            "content_only": [],
        })
    );
}
