// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-2 (register §1.409; contracts/install-cli.md, "The Lab's rescue module", "The
// authorization a placement names"; coordinator work order
// `rd-1115/brief-463-s32-place-authorization.md`; design `rd-1115/pack-stage3-design.md` §4.2,
// S3-2, §2.3 and §2.6): the rescue module's `place` of a plan that changes something names the
// authorization it acts under (`--authorization`) and every package of the plan's `enters`
// (`--enables`), judged after the plan and before the hold, any pause and any write, with a
// Runtime and without one alike; the `pack_place` journal records both.
// - `s32_no_reference_is_refused_before_the_hold_hot_and_cold`: with the fake Runtime running an
//   instance bound to the game and with no Runtime, no `--authorization`, a blank one, and none
//   beside an `--enables` that is not `enters` are each `install_command_refused`
//   `place_authorization_missing` with the plan's id and the game, exit 1: nothing asked of the
//   Runtime (no pause), the root's `install` folder (no journal) and the game root byte for byte
//   as they were;
// - `s32_a_reference_out_of_bounds_is_refused_unusable`: one of 1025 bytes, one with a line feed
//   and one with a tab are `actingctl_usage_invalid` `unusable` naming `--authorization`, with
//   nothing done; one of 1024 bytes is taken, the next check answering;
// - `s32_enables_not_the_plans_enters_is_refused_hot_and_cold`: on both paths, with `enters` the
//   two packs the bundle adds, a reference beside no `--enables`, only one of the two, the two and
//   a third, one of them twice and another name in place of one, and (a bundle that only replaces
//   content) a name where nothing enters, are each `place_enables_mismatch` with the answer's
//   `expected` and `given`, both sorted and stated name by name, with nothing done;
// - `s32_an_unreadable_bundle_schedule_is_refused_enables_unknown`: on both paths, a zip whose
//   `schedule.json` the reader refuses: the plan answers `packages` and `unread` (the zip's side,
//   `schedule_invalid` at the zip's file) and none of the three lists, and the placement with a
//   reference, with or without `--enables`, is `place_enables_unknown`, the plan's
//   `schedule_changes` beside it; without a reference `place_authorization_missing` first; nothing
//   done;
// - `s32_a_content_only_placement_records_the_reference_and_no_package`: with a Runtime, a bundle
//   that replaces a pack's content, its rule unchanged, placed with a reference and no
//   `--enables`: done, the instance held and handed back, the journal's inputs `authorization`
//   (as given) and `enabled` `[]`, which `op status <op>` shows; the same journal without both
//   members (as an earlier build wrote it) loads and `op status` answers it without either;
// - `s32_a_cold_placement_records_the_packages_it_enables`: with no Runtime, a bundle that adds two
//   packs, each named by a rule, placed with a reference and `--enables` naming exactly the two
//   (in the other order): done, nothing asked of the Runtime, both packs placed, the journal's
//   inputs `cold`, `authorization` and `enabled`, the two sorted, which `op status <op>` shows;
// - `s32_the_plan_and_an_unchanged_bundle_need_neither`: `place --plan` without a reference
//   answers the plan (its `enters` the two added packs) and a bundle that changes nothing is
//   placed without one (`installation_changed` `false`), each with nothing done.
// Round 2 (coordinator work order `rd-1115/brief-463-s32-fix2.md`, after S3-1's fix round 2):
// - `s32_a_bundle_schedule_naming_a_package_it_lacks_is_refused_enables_unknown`: on both paths, a
//   zip whose `schedule.json` the reader takes but whose third rule names a package the zip does
//   not carry, which the composer's own reference check refuses: the plan answers `packages` and
//   `unread` (the zip's side, `rule_package_unregistered` at `/rules/2/package_id`) and none of the
//   three lists, and the placement with a reference and `--enables` is `place_enables_unknown`,
//   the plan's `schedule_changes` beside it: nothing written, no pause sent.
// The fixtures never depend on how the plan counts a root without `schedule.json` or an installed
// root that cannot be read: both roots of every scene hold a `schedule.json` and the installed one
// is read. The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no
// device, no sleep, no clock read; every root is a temporary folder with neutral files (made-up
// ids, no real game).

use super::fixture::{
    BuildSpec, C1, FakeHost, REPOSITORY, code, live_root, run, tree, word, write_json,
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
/// The neutral game and its one server.
const GAME: &str = "neutral-game";
const SERVER: &str = "neutral-server";
/// The instance the fake Runtime binds to the game when it runs.
const INSTANCE: &str = "neutral.a";
/// The `package` range the installer of these tests declares, and the one every bundle declares
/// (`package` alone, as a resource repository's release writes it).
const INSTALLER_PACKAGE: [u64; 2] = [3, 10];
const RELEASED: [u64; 2] = [3, 3];
/// The packs: one the installed root schedules, two a bundle adds (sorted: [`ADDED`] before
/// [`ALSO`], both before [`KEPT`]).
const KEPT: &str = "neutral.kept";
const ADDED: &str = "neutral.added";
const ALSO: &str = "neutral.also";
/// A package no root carries.
const GHOST: &str = "neutral.ghost";
/// Two contents of a pack's `control.json`, of one length (so `bundle.json` keeps its bytes).
const CONTROL: &[u8] = br#"{"neutral": 1}"#;
const CONTROL_REPLACED: &[u8] = br#"{"neutral": 2}"#;
/// A `schedule.json` the reader refuses: a member it does not know.
const UNREADABLE_SCHEDULE: &[u8] = br#"{"neutral": "schedule"}"#;
/// The reference of the authorization the placements name (a made-up board reference).
const REFERENCE: &str = "board-463-comment-neutral-1";

/// One game root's content, as a bundle zip carries it and as an installed game root holds it.
#[derive(Default)]
struct Layout {
    /// Each pack's id and its `control.json`, in `bundle.json`'s order.
    packs: Vec<(&'static str, &'static [u8])>,
    /// `schedule.json`'s rules, each `(rule id, package id)`.
    rules: Vec<(&'static str, &'static str)>,
    /// Bytes of `schedule.json` in place of those its rules give.
    schedule: Option<&'static [u8]>,
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

    fn schedule_bytes(mut self, bytes: &'static [u8]) -> Self {
        self.schedule = Some(bytes);
        self
    }

    /// The root files, each `(name, bytes)`: `bundle.json` and `schedule.json`.
    fn root_files(&self) -> Vec<(&'static str, Vec<u8>)> {
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
        let schedule = match self.schedule {
            Some(bytes) => bytes.to_vec(),
            None => {
                let rules = self
                    .rules
                    .iter()
                    .map(|(rule_id, package_id)| json!({"id": rule_id, "package_id": package_id}))
                    .collect::<Vec<_>>();
                serde_json::to_vec(&json!({
                    "schema_version": "actingcommand.schedule.v1",
                    "rules": rules,
                }))
                .expect("JSON")
            }
        };
        vec![
            (
                "bundle.json",
                serde_json::to_vec(&json!({
                    "schema_version": "actingcommand.bundle.v4",
                    "game": GAME,
                    "packs": packs,
                }))
                .expect("JSON"),
            ),
            ("schedule.json", schedule),
        ]
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

/// The installed game root of every scene: the pack [`KEPT`], named by a rule.
fn installed() -> Layout {
    Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT)
}

/// A bundle that adds the packs [`ADDED`] and [`ALSO`], each named by a rule: both enter.
fn entering() -> Layout {
    installed()
        .pack(ADDED, CONTROL)
        .pack(ALSO, CONTROL)
        .rule("rule.added", ADDED)
        .rule("rule.also", ALSO)
}

/// A bundle that replaces [`KEPT`]'s content, its rule unchanged: nothing enters.
fn content_only() -> Layout {
    Layout::default()
        .pack(KEPT, CONTROL_REPLACED)
        .rule("rule.kept", KEPT)
}

/// How the placement finds the Runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// The fake Runtime runs [`INSTANCE`], bound to the game: a placement would hold it.
    Running,
    /// No Runtime answers: a placement would place cold.
    Stopped,
}

const WAYS: [Way; 2] = [Way::Running, Way::Stopped];

/// The root's `install` folder and the game root, each by file.
type Files = (BTreeMap<String, Vec<u8>>, BTreeMap<String, Vec<u8>>);

/// One root, adopted, whose configuration names the standard-pack root `<temp>\s32-packs`, with
/// the game root laid out as [`installed`], and one bundle zip.
struct Scene {
    host: FakeHost,
    root: PathBuf,
    game_root: PathBuf,
    zip: PathBuf,
    _temp: TempDir,
}

/// The scene of `way` whose bundle is `after`; the installer declares [`INSTALLER_PACKAGE`].
fn staged(way: Way, after: &Layout) -> Scene {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), "s32", &BuildSpec::new(C1));
    let packs = temp.path().join("s32-packs");
    let game_root = packs.join(GAME);
    fs::create_dir_all(&game_root).expect("the game root");
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration")).expect("JSON");
    config["standard_packs"] = json!({"root": packs});
    write_json(&config_path, &config);
    let adopted = run(&host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    host.installer_package.set(Some(INSTALLER_PACKAGE));
    installed().lay_out(&game_root);
    if way == Way::Running {
        host.runtime.boot(&[INSTANCE]);
        for instance in host.runtime.instances.borrow_mut().iter_mut() {
            instance.game = Some(GAME.to_owned());
        }
    }
    let zip = temp.path().join("neutral-bundle.zip");
    after.write_zip(&zip);
    Scene {
        host,
        root,
        game_root,
        zip,
        _temp: temp,
    }
}

impl Scene {
    /// The rescue row `name` with `params` on the root, as [`WORD`]: its one line and exit code.
    fn line(&self, name: &str, params: Value) -> (Value, u8) {
        let config = self.root.join("actingd.config.json");
        let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
            .expect("the configured state root");
        let location = rescue::location_of(&self.root, &config, &state_root);
        let context = Context {
            host: &self.host,
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

    /// `place` of the scene's bundle, the members of `more` beside `bundle`.
    fn place(&self, more: Value) -> (Value, u8) {
        let mut params = json!({"bundle": word(&self.zip)});
        if let Value::Object(more) = more {
            for (name, value) in more {
                params[name.as_str()] = value;
            }
        }
        self.line("place", params)
    }

    /// `place --plan`: the plan, after asserting that it was answered.
    fn plan(&self) -> Value {
        let (line, exit) = self.place(json!({"plan": true}));
        assert_eq!(exit, 0, "{line}");
        assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
        line["plan"].clone()
    }

    fn files(&self) -> Files {
        (tree(&self.root.join("install")), tree(&self.game_root))
    }

    /// The zip's `schedule.json` as the plan's failures name it: below the zip's canonical path.
    fn zip_schedule(&self) -> PathBuf {
        fs::canonicalize(&self.zip)
            .expect("the canonical bundle")
            .join("schedule.json")
    }

    /// Every journal of the root, by operation id.
    fn journals(&self) -> Vec<(String, Value)> {
        let directory = self.root.join("install").join("operations");
        let mut found = fs::read_dir(&directory)
            .expect("the journals")
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".json")?.to_owned();
                id.starts_with("op-").then(|| {
                    let journal =
                        serde_json::from_slice(&fs::read(entry.path()).expect("a journal"))
                            .expect("journal JSON");
                    (id, journal)
                })
            })
            .collect::<Vec<_>>();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// The one journal written since `before`: its operation id and the journal.
    fn new_journal(&self, before: &[(String, Value)]) -> (String, Value) {
        let mut written = self
            .journals()
            .into_iter()
            .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
            .collect::<Vec<_>>();
        assert_eq!(written.len(), 1, "{written:?}");
        written.remove(0)
    }
}

/// A `schedule_changes` as the tests of an unread zip compare it: its member names, sorted, and
/// its `unread` with each failure as its outcome's code and values, `[{root, code, values}]`.
fn unread_answer(changes: &Value) -> (Vec<String>, Value) {
    let mut names = changes
        .as_object()
        .map(|members| members.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    names.sort();
    let sides = changes["unread"]
        .as_array()
        .map(|sides| {
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
        })
        .unwrap_or_default();
    (names, json!(sides))
}

/// The member names of a `schedule_changes` whose zip's side could not be read: `packages` and
/// `unread`, none of `enters`, `leaves` and `content_only`.
fn unread_names() -> Vec<String> {
    vec!["packages".to_owned(), "unread".to_owned()]
}

/// The one side `unread` names when the zip's `schedule.json` at `path` is refused with
/// `problem` at `pointer`: the zip's, `standard_pack_root_invalid`.
fn bundle_unread(path: &Path, problem: &str, pointer: &str) -> Value {
    json!([{
        "root": "bundle",
        "code": "standard_pack_root_invalid",
        "values": {
            "game": GAME,
            "path": path.to_string_lossy(),
            "standard_pack_problem": problem,
            "json_pointer": pointer,
        },
    }])
}

/// Nothing was done: the root's `install` folder (its journals, `current.json`, the writer lock)
/// and the game root are byte for byte `before`, nothing was asked of the fake Runtime (no
/// pause, no catalog read) and its instance is not paused.
fn assert_nothing_done(scene: &Scene, before: &Files, case: &str) {
    assert_eq!(
        &scene.files(),
        before,
        "{case}: the root or the game root changed"
    );
    assert!(
        scene.host.runtime.calls.borrow().is_empty(),
        "{case}: {:?}",
        scene.host.runtime.calls.borrow()
    );
    assert_eq!(scene.host.runtime.pause_of(INSTANCE), None, "{case}");
}

/// `line`, exit `exit`, refuses the placement of the plan `plan_id`: `install_command_refused`
/// of `install_pack_place` with `refusal`, the plan's id and the game, exit 1, nothing done.
fn assert_refused(
    scene: &Scene,
    line: &Value,
    exit: u8,
    refusal: &str,
    plan_id: &Value,
    before: &Files,
    case: &str,
) {
    assert_eq!(exit, 1, "{case}: {line}");
    assert_eq!(
        line["outcome"]["code"], "install_command_refused",
        "{case}: {line}"
    );
    let values = &line["outcome"]["values"];
    assert_eq!(values["operation"], "install_pack_place", "{case}: {line}");
    assert_eq!(values["install_refusal"], refusal, "{case}: {line}");
    assert_eq!(&values["plan_id"], plan_id, "{case}: {line}");
    assert_eq!(values["game"], GAME, "{case}: {line}");
    assert_nothing_done(scene, before, case);
}

// No reference, a blank one, and none beside an `--enables` that is not the plan's `enters`: each
// `place_authorization_missing` (the reference first, ruling 9), with the Runtime and without it,
// before anything is held or written.
#[test]
fn s32_no_reference_is_refused_before_the_hold_hot_and_cold() {
    for way in WAYS {
        let scene = staged(way, &entering());
        let plan_id = scene.plan()["plan_id"].clone();
        assert!(plan_id.is_string(), "{way:?}: {plan_id}");
        let before = scene.files();
        for (case, more) in [
            ("no reference", json!({"enables": [ADDED, ALSO]})),
            (
                "a blank reference",
                json!({"authorization": "   ", "enables": [ADDED, ALSO]}),
            ),
            (
                "no reference beside a wrong --enables",
                json!({"enables": [KEPT]}),
            ),
        ] {
            let (line, exit) = scene.place(more);
            let case = format!("{way:?}: {case}");
            assert_refused(
                &scene,
                &line,
                exit,
                "place_authorization_missing",
                &plan_id,
                &before,
                &case,
            );
        }
    }
}

// A reference longer than 1024 bytes or holding a control character is a malformed argument,
// named; one of 1024 bytes is taken (the next check, `--enables` absent, answers).
#[test]
fn s32_a_reference_out_of_bounds_is_refused_unusable() {
    let scene = staged(Way::Running, &entering());
    let plan_id = scene.plan()["plan_id"].clone();
    let before = scene.files();
    for (case, reference) in [
        ("1025 bytes", "r".repeat(1025)),
        ("a line feed", "board-463\ncomment".to_owned()),
        ("a tab", "board-463\tcomment".to_owned()),
    ] {
        let (line, exit) =
            scene.place(json!({"authorization": reference, "enables": [ADDED, ALSO]}));
        assert_eq!(exit, 1, "{case}: {line}");
        assert_eq!(
            line["outcome"]["code"], "actingctl_usage_invalid",
            "{case}: {line}"
        );
        let values = &line["outcome"]["values"];
        assert_eq!(values["argument"], "--authorization", "{case}: {line}");
        assert_eq!(values["argument_problem"], "unusable", "{case}: {line}");
        assert_nothing_done(&scene, &before, case);
    }
    let (line, exit) = scene.place(json!({"authorization": "r".repeat(1024)}));
    assert_refused(
        &scene,
        &line,
        exit,
        "place_enables_mismatch",
        &plan_id,
        &before,
        "1024 bytes",
    );
    assert_eq!(line["expected"], json!([ADDED, ALSO]), "{line}");
    assert_eq!(line["given"], json!([]), "{line}");
}

// `--enables` not exactly the plan's `enters` (the two packs the bundle adds), on both paths:
// none, only one of the two, the two and a third, one of them twice, another name in place of one;
// and where nothing enters, any name. Each `place_enables_mismatch` with the answer's `expected`
// and `given`, both sorted, each stated here name by name.
#[test]
fn s32_enables_not_the_plans_enters_is_refused_hot_and_cold() {
    for way in WAYS {
        let scene = staged(way, &entering());
        let plan = scene.plan();
        assert_eq!(
            plan["schedule_changes"]["enters"],
            json!([ADDED, ALSO]),
            "{way:?}: {plan}"
        );
        let plan_id = plan["plan_id"].clone();
        let before = scene.files();
        for (case, enables, given) in [
            ("no --enables", None, json!([])),
            ("only one of the two", Some(json!([ALSO])), json!([ALSO])),
            (
                "the two and a third",
                Some(json!([ALSO, KEPT, ADDED])),
                json!([ADDED, ALSO, KEPT]),
            ),
            (
                "one of the two twice",
                Some(json!([ALSO, ADDED, ALSO])),
                json!([ADDED, ALSO, ALSO]),
            ),
            (
                "another name in place of one",
                Some(json!([GHOST, ADDED])),
                json!([ADDED, GHOST]),
            ),
        ] {
            let mut more = json!({"authorization": REFERENCE});
            if let Some(enables) = enables {
                more["enables"] = enables;
            }
            let (line, exit) = scene.place(more);
            let case = format!("{way:?}: {case}");
            assert_refused(
                &scene,
                &line,
                exit,
                "place_enables_mismatch",
                &plan_id,
                &before,
                &case,
            );
            assert_eq!(line["expected"], json!([ADDED, ALSO]), "{case}: {line}");
            assert_eq!(line["given"], given, "{case}: {line}");
        }

        let replaced = staged(way, &content_only());
        let plan = replaced.plan();
        assert_eq!(
            plan["schedule_changes"]["enters"],
            json!([]),
            "{way:?}: {plan}"
        );
        let plan_id = plan["plan_id"].clone();
        let before = replaced.files();
        let (line, exit) = replaced.place(json!({"authorization": REFERENCE, "enables": [KEPT]}));
        let case = format!("{way:?}: a name where nothing enters");
        assert_refused(
            &replaced,
            &line,
            exit,
            "place_enables_mismatch",
            &plan_id,
            &before,
            &case,
        );
        assert_eq!(line["expected"], json!([]), "{case}: {line}");
        assert_eq!(line["given"], json!([KEPT]), "{case}: {line}");
    }
}

// A zip whose `schedule.json` the reader refuses: the plan answers `packages` and `unread` (the
// zip's side) and none of the three lists, and the placement is `place_enables_unknown`, the
// plan's `schedule_changes` beside it, with a reference, with or without `--enables`; without a
// reference that refusal comes first.
#[test]
fn s32_an_unreadable_bundle_schedule_is_refused_enables_unknown() {
    for way in WAYS {
        let scene = staged(way, &entering().schedule_bytes(UNREADABLE_SCHEDULE));
        let unread = bundle_unread(&scene.zip_schedule(), "schedule_invalid", "/neutral");
        let plan = scene.plan();
        assert_eq!(
            unread_answer(&plan["schedule_changes"]),
            (unread_names(), unread.clone()),
            "{way:?}: {plan}"
        );
        let plan_id = plan["plan_id"].clone();
        let before = scene.files();
        for (case, more) in [
            ("a reference", json!({"authorization": REFERENCE})),
            (
                "a reference and --enables",
                json!({"authorization": REFERENCE, "enables": [ADDED, ALSO]}),
            ),
        ] {
            let (line, exit) = scene.place(more);
            let case = format!("{way:?}: {case}");
            assert_refused(
                &scene,
                &line,
                exit,
                "place_enables_unknown",
                &plan_id,
                &before,
                &case,
            );
            assert_eq!(
                unread_answer(&line["schedule_changes"]),
                (unread_names(), unread.clone()),
                "{case}: {line}"
            );
        }
        let (line, exit) = scene.place(json!({"enables": [ADDED, ALSO]}));
        let case = format!("{way:?}: no reference");
        assert_refused(
            &scene,
            &line,
            exit,
            "place_authorization_missing",
            &plan_id,
            &before,
            &case,
        );
    }
}

// Round 2: a zip whose `schedule.json` the reader takes but whose third rule names a package the
// zip does not carry, which the composer's own reference check refuses (S3-1's fix round 2): the
// plan answers `packages` and `unread` (the zip's side, `rule_package_unregistered` at that rule's
// package) and none of the three lists, and the placement with a reference and `--enables` is
// `place_enables_unknown`, the plan's `schedule_changes` beside it: nothing written, no pause sent.
#[test]
fn s32_a_bundle_schedule_naming_a_package_it_lacks_is_refused_enables_unknown() {
    for way in WAYS {
        let bundle = installed()
            .pack(ADDED, CONTROL)
            .rule("rule.added", ADDED)
            .rule("rule.ghost", GHOST);
        let scene = staged(way, &bundle);
        let unread = bundle_unread(
            &scene.zip_schedule(),
            "rule_package_unregistered",
            "/rules/2/package_id",
        );
        let plan = scene.plan();
        assert_eq!(
            unread_answer(&plan["schedule_changes"]),
            (unread_names(), unread.clone()),
            "{way:?}: {plan}"
        );
        let plan_id = plan["plan_id"].clone();
        let before = scene.files();
        let (line, exit) = scene.place(json!({"authorization": REFERENCE, "enables": [ADDED]}));
        let case = format!("{way:?}: a rule naming a package the zip lacks");
        assert_refused(
            &scene,
            &line,
            exit,
            "place_enables_unknown",
            &plan_id,
            &before,
            &case,
        );
        assert_eq!(
            unread_answer(&line["schedule_changes"]),
            (unread_names(), unread),
            "{case}: {line}"
        );
    }
}

// With a Runtime, a bundle that replaces a pack's content, its rule unchanged, placed with a
// reference and no `--enables`: the journal records the reference and an empty list, `op status`
// shows both, and a journal without either (as an earlier build wrote it) still loads and is
// answered without them.
#[test]
fn s32_a_content_only_placement_records_the_reference_and_no_package() {
    let scene = staged(Way::Running, &content_only());
    let plan = scene.plan();
    assert_eq!(plan["schedule_changes"]["enters"], json!([]), "{plan}");
    assert_eq!(
        plan["schedule_changes"]["content_only"],
        json!([KEPT]),
        "{plan}"
    );
    let before = scene.journals();
    let (line, exit) = scene.place(json!({"authorization": REFERENCE}));
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    // A placement with a Runtime: its hold paused the bound instance and handed it back.
    let calls = scene.host.runtime.calls.borrow().clone();
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("pause neutral.a lab-place-")),
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|call| call == "resume neutral.a"),
        "{calls:?}"
    );
    assert_eq!(
        fs::read(scene.game_root.join(KEPT).join("control.json")).expect("the placed pack"),
        CONTROL_REPLACED
    );
    let (operation_id, journal) = scene.new_journal(&before);
    assert_eq!(journal["kind"], "pack_place", "{journal}");
    assert_eq!(journal["inputs"]["authorization"], REFERENCE, "{journal}");
    assert_eq!(journal["inputs"]["enabled"], json!([]), "{journal}");

    let (status, exit) = scene.line("op status", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{status}");
    let shown = &status["journal"]["inputs"];
    assert_eq!(shown["authorization"], REFERENCE, "{status}");
    assert_eq!(shown["enabled"], json!([]), "{status}");

    // The journal as an earlier build wrote it: without both members.
    let mut earlier = journal.clone();
    let earlier_inputs = earlier["inputs"].as_object_mut().expect("the inputs");
    assert!(earlier_inputs.remove("authorization").is_some());
    assert!(earlier_inputs.remove("enabled").is_some());
    let path = scene
        .root
        .join("install")
        .join("operations")
        .join(format!("{operation_id}.json"));
    write_json(&path, &earlier);
    let (older, exit) = scene.line("op status", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{older}");
    let read = &older["journal"]["inputs"];
    assert_eq!(read["bound"], json!([INSTANCE]), "{older}");
    assert!(read.get("authorization").is_none(), "{older}");
    assert!(read.get("enabled").is_none(), "{older}");
}

// With no Runtime, a bundle that adds two packs, each named by a rule, placed with a reference and
// `--enables` naming exactly the two, in the other order: the journal (cold) records both, the
// two sorted, and `op status` shows them.
#[test]
fn s32_a_cold_placement_records_the_packages_it_enables() {
    let scene = staged(Way::Stopped, &entering());
    let before = scene.journals();
    let (line, exit) = scene.place(json!({"authorization": REFERENCE, "enables": [ALSO, ADDED]}));
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        line["plan"]["schedule_changes"]["enters"],
        json!([ADDED, ALSO]),
        "{line}"
    );
    assert!(
        scene.host.runtime.calls.borrow().is_empty(),
        "{:?}",
        scene.host.runtime.calls.borrow()
    );
    for pack in [ADDED, ALSO] {
        assert_eq!(
            fs::read(scene.game_root.join(pack).join("control.json")).expect("a placed pack"),
            CONTROL,
            "{pack}"
        );
    }
    let (operation_id, journal) = scene.new_journal(&before);
    assert_eq!(journal["kind"], "pack_place", "{journal}");
    assert_eq!(journal["inputs"]["cold"], json!(true), "{journal}");
    assert_eq!(journal["inputs"]["authorization"], REFERENCE, "{journal}");
    assert_eq!(
        journal["inputs"]["enabled"],
        json!([ADDED, ALSO]),
        "{journal}"
    );

    let (status, exit) = scene.line("op status", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{status}");
    let shown = &status["journal"]["inputs"];
    assert_eq!(shown["authorization"], REFERENCE, "{status}");
    assert_eq!(shown["enabled"], json!([ADDED, ALSO]), "{status}");
}

// `place --plan` changes nothing and needs neither parameter; nor does a bundle whose plan changes
// nothing, placed without them.
#[test]
fn s32_the_plan_and_an_unchanged_bundle_need_neither() {
    let scene = staged(Way::Running, &entering());
    let before = scene.files();
    let plan = scene.plan();
    assert_eq!(
        plan["schedule_changes"]["enters"],
        json!([ADDED, ALSO]),
        "{plan}"
    );
    assert_nothing_done(&scene, &before, "--plan");

    let unchanged = staged(Way::Running, &installed());
    let before = unchanged.files();
    let (line, exit) = unchanged.place(json!({}));
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        line["outcome"]["values"]["installation_changed"],
        json!(false),
        "{line}"
    );
    assert_nothing_done(&unchanged, &before, "an unchanged bundle");
}
