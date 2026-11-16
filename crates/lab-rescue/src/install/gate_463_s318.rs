// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-18 (register §1.452; contracts/install-cli.md, "The Lab's rescue module", "The
// packages the undo of a placement makes enter or leave scheduling"; coordinator work order
// `rd-1116/brief-463-s318-undo-schedule-changes.md`): the undo of a `pack_place` operation
// computes, before its first step and by the placement plan's own judgement, which packages it
// makes enter and leave scheduling; it answers them (`schedule_changes`), warns when packages enter
// (`undo_enters_scheduling`) or when that cannot be known (`undo_scheduling_unknown`), records the
// two lists in its undo record, and its plan form (`undo <op> --plan`) answers the same and writes
// nothing. Each test that loops over [`WAYS`] runs with the fake Runtime running an instance bound
// to the game (the placement holds it; its undo asks the Runtime nothing) and with no Runtime (the
// placement and its undo apply the stopped rule): the work order's test 7.
// - `s318_the_undo_of_two_added_packs_lists_them_leaving_hot_and_cold` (test 1): a placement that
//   added two packs, each named by a rule, undone: on the plan form and on the undo done, `leaves`
//   the two, `enters` and `content_only` empty, each pack `scheduled` before and `absent` after
//   with `content` `retire`, no note; the undo done sets both packs aside;
// - `s318_the_undo_that_brings_a_pack_back_warns_and_names_it_hot_and_cold` (test 2): a placement
//   whose bundle removed the rule of [`BACK`], undone: on the plan form and on the undo done
//   `enters` names it (`unclassified` before, `scheduled` after), `leaves` empty, and the one note
//   is `undo_enters_scheduling` of `install_undo` with the game and `names` [`BACK`]; the undo done
//   puts the rule back; with a Runtime nothing is asked of it;
// - `s318_the_undo_of_a_content_only_placement_lists_the_pack_content_only_hot_and_cold` (test
//   3): a placement that only replaced a pack's content, undone: both lists empty, `content_only`
//   the pack (`content` `replace`), no note; the undo done puts the content back;
// - `s318_the_plan_form_writes_nothing_and_answers_what_the_undo_answers_hot_and_cold` (test 4):
//   on the scene of test 2 the plan form leaves the root and the game root byte for byte, asks the
//   Runtime nothing, answers `installation_changed` `false` and `reverses` (the steps started or
//   done, from the last, the `pack_item` of `schedule.json` among them); the undo then answers the
//   same `schedule_changes` and the same note; with no Runtime and an actingd under the root, the
//   plan form and the undo are both refused `runtime_running`, nothing written;
// - `s318_the_record_carries_the_lists_and_one_without_them_loads` (test 5): the journal's undo
//   record carries `schedule_changes` `{enters: [BACK], leaves: []}`, which `op status <op>` shows;
//   the same journal without the member loads and `op status` answers it without it;
// - `s318_an_undone_root_that_cannot_be_read_warns_unknown_and_is_undone_hot_and_cold` (test 6):
//   a placement that replaced a `schedule.json` the reader refuses, undone: the plan form and the
//   undo done answer `packages` and `unread` (`undone`, `standard_pack_root_invalid`
//   `schedule_invalid` at the game root's `schedule.json`) and no list, the one note
//   `undo_scheduling_unknown` caused by that failure; the undo is done (the refused file back byte
//   for byte) and its record's two lists are `null`;
// - `s318_the_undo_of_another_kind_answers_as_before` (test 8): the undo of an `adopt` operation
//   answers `kept`, `operation_id` and `outcome` alone, the outcome's values `operation`,
//   `install_operation_id` and `installation_changed` alone and no link, and its record has no
//   `schedule_changes`; `undo --plan` of it is refused `unusable` (`--plan`) and `actingctl install
//   undo --plan` `unknown`, each with nothing written.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep, no clock read; every root is a temporary folder with neutral files (made-up ids, no real
// game). Both roots of every scene hold a `schedule.json`.

use super::fixture::{
    BuildSpec, C1, FakeHost, REPOSITORY, code, live_root, run, tree, word, write_json,
};
use super::host::RunningImage;
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
/// The `package` range the installer of these tests declares, and the one every bundle declares.
const INSTALLER_PACKAGE: [u64; 2] = [3, 10];
const RELEASED: [u64; 2] = [3, 3];
/// The packs, sorted: [`ADDED`], [`ALSO`], [`BACK`], [`KEPT`].
const ADDED: &str = "neutral.added";
const ALSO: &str = "neutral.also";
/// The pack a placement makes leave scheduling, which its undo brings back.
const BACK: &str = "neutral.back";
const KEPT: &str = "neutral.kept";
/// Two contents of a pack's `control.json`, of one length (so `bundle.json` keeps its bytes).
const CONTROL: &[u8] = br#"{"neutral": 1}"#;
const CONTROL_REPLACED: &[u8] = br#"{"neutral": 2}"#;
/// A `schedule.json` the reader refuses: a member it does not know.
const UNREADABLE_SCHEDULE: &[u8] = br#"{"neutral": "schedule"}"#;
/// The reference of the authorization the placements name (a made-up board reference).
const REFERENCE: &str = "board-463-comment-neutral-18";
/// The catalogs the fake Runtime answers while it runs: the one read before a placement's hold,
/// and a new one at the read of its check, so that a placement that changes `schedule.json` sees
/// a new composition.
const CATALOG_BEFORE: &str = "neutral-catalog-before";
const CATALOG_AFTER: &str = "neutral-catalog-after";
/// The process id of an actingd the fake host lists under the root.
const STARTING_PID: u32 = 4_242;

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

    /// The bytes of its `schedule.json`.
    fn schedule(&self) -> Vec<u8> {
        match self.schedule {
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
        }
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
            ("schedule.json", self.schedule()),
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

/// The game root of [`KEPT`], named by a rule.
fn kept() -> Layout {
    Layout::default()
        .pack(KEPT, CONTROL)
        .rule("rule.kept", KEPT)
}

/// [`kept`] and the packs [`ADDED`] and [`ALSO`], each named by a rule.
fn adding() -> Layout {
    kept()
        .pack(ADDED, CONTROL)
        .pack(ALSO, CONTROL)
        .rule("rule.added", ADDED)
        .rule("rule.also", ALSO)
}

/// [`KEPT`] and [`BACK`], with `back_rule` the rule of [`BACK`] too.
fn with_back(back_rule: bool) -> Layout {
    let layout = kept().pack(BACK, CONTROL);
    if back_rule {
        layout.rule("rule.back", BACK)
    } else {
        layout
    }
}

/// [`KEPT`] with another content, its rule unchanged.
fn replaced() -> Layout {
    Layout::default()
        .pack(KEPT, CONTROL_REPLACED)
        .rule("rule.kept", KEPT)
}

/// How the placement finds the Runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// The fake Runtime runs [`INSTANCE`], bound to the game: the placement holds it.
    Running,
    /// No Runtime answers: the placement places cold.
    Stopped,
}

const WAYS: [Way; 2] = [Way::Running, Way::Stopped];

/// The root and the game root, each by file.
type Files = (BTreeMap<String, Vec<u8>>, BTreeMap<String, Vec<u8>>);

/// One root, adopted, whose configuration names the standard-pack root `<temp>\s318-packs`, with
/// the game root laid out, and one bundle zip.
struct Scene {
    host: FakeHost,
    root: PathBuf,
    game_root: PathBuf,
    zip: PathBuf,
    _temp: TempDir,
}

/// The scene of `way` whose game root is `before` and whose bundle is `after`.
fn staged(way: Way, before: &Layout, after: &Layout) -> Scene {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), "s318", &BuildSpec::new(C1));
    let packs = temp.path().join("s318-packs");
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
    before.lay_out(&game_root);
    if way == Way::Running {
        host.runtime.boot(&[INSTANCE]);
        for instance in host.runtime.instances.borrow_mut().iter_mut() {
            instance.game = Some(GAME.to_owned());
        }
        *host.runtime.catalog.borrow_mut() = Some(CATALOG_BEFORE.to_owned());
        host.runtime
            .catalogs
            .borrow_mut()
            .extend([CATALOG_BEFORE.to_owned(), CATALOG_AFTER.to_owned()]);
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

    /// The journal of `operation_id`.
    fn journal(&self, operation_id: &str) -> Value {
        serde_json::from_slice(&fs::read(self.journal_path(operation_id)).expect("the journal"))
            .expect("journal JSON")
    }

    fn journal_path(&self, operation_id: &str) -> PathBuf {
        self.root
            .join("install")
            .join("operations")
            .join(format!("{operation_id}.json"))
    }

    fn files(&self) -> Files {
        (tree(&self.root), tree(&self.game_root))
    }

    /// The bundle placed: its plan first (`place --plan`), then the placement with
    /// [`REFERENCE`] and the plan's `enters` as `enables`; done, and its one new journal a
    /// `pack_place`, whose operation id is answered.
    fn placed(&self) -> String {
        let bundle = word(&self.zip);
        let (planned, exit) = self.line("place", json!({"bundle": bundle, "plan": true}));
        assert_eq!(exit, 0, "{planned}");
        let enters = planned["plan"]["schedule_changes"]["enters"].clone();
        assert!(enters.is_array(), "{planned}");
        let mut params = json!({"bundle": bundle, "authorization": REFERENCE});
        if enters.as_array().is_some_and(|enters| !enters.is_empty()) {
            params["enables"] = enters;
        }
        let before = self.journals();
        let (placed, exit) = self.line("place", params);
        assert_eq!(exit, 0, "{placed}");
        assert_eq!(
            placed["outcome"]["code"], "install_command_done",
            "{placed}"
        );
        let mut written = self
            .journals()
            .into_iter()
            .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
            .collect::<Vec<_>>();
        assert_eq!(written.len(), 1, "{written:?}");
        let (operation_id, journal) = written.remove(0);
        assert_eq!(journal["kind"], "pack_place", "{journal}");
        operation_id
    }

    /// The rescue row `undo` of `operation_id`, its plan form with `plan`.
    fn undo(&self, operation_id: &str, plan: bool) -> (Value, u8) {
        let mut params = json!({"op": operation_id});
        if plan {
            params["plan"] = json!(true);
        }
        self.line("undo", params)
    }

    /// The number of calls the fake Runtime has had.
    fn asked(&self) -> usize {
        self.host.runtime.calls.borrow().len()
    }
}

/// Which form of the undo a line answers.
fn form(plan: bool) -> &'static str {
    if plan { "--plan" } else { "the undo done" }
}

/// `line`, exit `exit`, is `install_command_done` of `install_undo` of `operation_id`, its
/// `installation_changed` `!plan`.
fn assert_undo_done(line: &Value, exit: u8, operation_id: &str, plan: bool, case: &str) {
    assert_eq!(exit, 0, "{case}: {line}");
    let outcome = &line["outcome"];
    assert_eq!(outcome["code"], "install_command_done", "{case}: {line}");
    assert_eq!(
        outcome["values"]["operation"], "install_undo",
        "{case}: {line}"
    );
    assert_eq!(
        outcome["values"]["install_operation_id"], operation_id,
        "{case}: {line}"
    );
    assert_eq!(
        outcome["values"]["installation_changed"],
        json!(!plan),
        "{case}: {line}"
    );
    assert_eq!(line["operation_id"], operation_id, "{case}: {line}");
}

/// The notes of `line`'s outcome.
fn notes(line: &Value) -> Vec<Value> {
    line["outcome"]["causes"]
        .as_array()
        .map(|links| {
            links
                .iter()
                .filter(|link| link["relation"] == "note")
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The one note of `line`: `install_caveat_noted` of `install_undo` with `caveat` and the game.
fn the_note(line: &Value, caveat: &str, case: &str) -> Value {
    let notes = notes(line);
    let [note] = notes.as_slice() else {
        panic!("{case}: one note: {line}");
    };
    assert_eq!(note["code"], "install_caveat_noted", "{case}: {line}");
    assert_eq!(
        note["values"]["operation"], "install_undo",
        "{case}: {line}"
    );
    assert_eq!(note["values"]["install_caveat"], caveat, "{case}: {line}");
    assert_eq!(note["values"]["game"], GAME, "{case}: {line}");
    note.clone()
}

/// The three lists of `line`'s `schedule_changes`, and no `unread`.
fn assert_lists(line: &Value, enters: &Value, leaves: &Value, content_only: &Value, case: &str) {
    let changes = &line["schedule_changes"];
    assert_eq!(&changes["enters"], enters, "{case}: {line}");
    assert_eq!(&changes["leaves"], leaves, "{case}: {line}");
    assert_eq!(&changes["content_only"], content_only, "{case}: {line}");
    assert!(changes.get("unread").is_none(), "{case}: {line}");
}

// Test 1: a placement that added two packs, each named by a rule, undone: `leaves` the two,
// `enters` empty, no warning, on the plan form and on the undo done, with a Runtime and without.
#[test]
fn s318_the_undo_of_two_added_packs_lists_them_leaving_hot_and_cold() {
    for way in WAYS {
        let scene = staged(way, &kept(), &adding());
        let operation_id = scene.placed();
        for plan in [true, false] {
            let case = format!("{way:?}: {}", form(plan));
            let (line, exit) = scene.undo(&operation_id, plan);
            assert_undo_done(&line, exit, &operation_id, plan, &case);
            assert_lists(&line, &json!([]), &json!([ADDED, ALSO]), &json!([]), &case);
            assert_eq!(
                line["schedule_changes"]["packages"],
                json!([
                    {"package_id": ADDED, "before": {"state": "scheduled"},
                        "after": {"state": "absent"}, "content": "retire"},
                    {"package_id": ALSO, "before": {"state": "scheduled"},
                        "after": {"state": "absent"}, "content": "retire"},
                ]),
                "{case}: {line}"
            );
            assert!(notes(&line).is_empty(), "{case}: {line}");
        }
        assert!(!scene.game_root.join(ADDED).exists(), "{way:?}");
        assert!(!scene.game_root.join(ALSO).exists(), "{way:?}");
        assert_eq!(
            fs::read(scene.game_root.join("schedule.json")).expect("schedule.json"),
            kept().schedule(),
            "{way:?}"
        );
    }
}

// Test 2: a placement whose bundle removed the rule of [`BACK`] (it left scheduling), undone:
// `enters` names it and so does the warning, on the plan form and on the undo done, with a Runtime
// and without; the undo asks the Runtime nothing.
#[test]
fn s318_the_undo_that_brings_a_pack_back_warns_and_names_it_hot_and_cold() {
    for way in WAYS {
        let scene = staged(way, &with_back(true), &with_back(false));
        let operation_id = scene.placed();
        let asked = scene.asked();
        for plan in [true, false] {
            let case = format!("{way:?}: {}", form(plan));
            let (line, exit) = scene.undo(&operation_id, plan);
            assert_undo_done(&line, exit, &operation_id, plan, &case);
            assert_lists(&line, &json!([BACK]), &json!([]), &json!([]), &case);
            assert_eq!(
                line["schedule_changes"]["packages"],
                json!([{"package_id": BACK, "before": {"state": "unclassified"},
                    "after": {"state": "scheduled"}}]),
                "{case}: {line}"
            );
            let note = the_note(&line, "undo_enters_scheduling", &case);
            assert_eq!(note["values"]["names"], json!([BACK]), "{case}: {line}");
        }
        assert_eq!(
            scene.asked(),
            asked,
            "{way:?}: the undo asks the Runtime nothing"
        );
        assert_eq!(
            fs::read(scene.game_root.join("schedule.json")).expect("schedule.json"),
            with_back(true).schedule(),
            "{way:?}: the rule is back"
        );
    }
}

// Test 3: a placement that only replaced a pack's content, undone: both lists empty, the
// content-only list names the pack, no warning, with a Runtime and without.
#[test]
fn s318_the_undo_of_a_content_only_placement_lists_the_pack_content_only_hot_and_cold() {
    for way in WAYS {
        let scene = staged(way, &kept(), &replaced());
        let operation_id = scene.placed();
        for plan in [true, false] {
            let case = format!("{way:?}: {}", form(plan));
            let (line, exit) = scene.undo(&operation_id, plan);
            assert_undo_done(&line, exit, &operation_id, plan, &case);
            assert_lists(&line, &json!([]), &json!([]), &json!([KEPT]), &case);
            assert_eq!(
                line["schedule_changes"]["packages"],
                json!([{"package_id": KEPT, "before": {"state": "scheduled"},
                    "after": {"state": "scheduled"}, "content": "replace"}]),
                "{case}: {line}"
            );
            assert!(notes(&line).is_empty(), "{case}: {line}");
        }
        assert_eq!(
            fs::read(scene.game_root.join(KEPT).join("control.json")).expect("the pack"),
            CONTROL,
            "{way:?}: the content is back"
        );
    }
}

// Test 4: the plan form writes nothing (the root and the game root byte for byte, nothing asked of
// the Runtime) and answers the lists, and the warning, the undo then answers; with no Runtime it
// applies the stopped rule as the undo does.
#[test]
fn s318_the_plan_form_writes_nothing_and_answers_what_the_undo_answers_hot_and_cold() {
    for way in WAYS {
        let scene = staged(way, &with_back(true), &with_back(false));
        let operation_id = scene.placed();
        let journal = scene.journal(&operation_id);
        let schedule_step = journal["steps"]
            .as_array()
            .expect("the steps")
            .iter()
            .find(|step| {
                step["kind"] == "pack_item"
                    && step["target"]
                        .as_str()
                        .is_some_and(|target| target.ends_with("schedule.json"))
            })
            .cloned()
            .unwrap_or_else(|| panic!("{way:?}: the pack_item of schedule.json: {journal}"));

        if way == Way::Stopped {
            // An actingd under the root: the plan form and the undo are refused alike.
            scene.host.processes.replace(vec![RunningImage {
                pid: STARTING_PID,
                path: format!(
                    "{}\\runtime\\actingcommand-actingd.exe",
                    super::files::plain(&scene.root)
                ),
                mcp_serve: false,
                parent_pid: None,
            }]);
            let files = scene.files();
            for plan in [true, false] {
                let case = format!("{way:?}: an actingd under the root: {}", form(plan));
                let (line, exit) = scene.undo(&operation_id, plan);
                assert_eq!(exit, 1, "{case}: {line}");
                assert_eq!(
                    line["outcome"]["code"], "install_command_refused",
                    "{case}: {line}"
                );
                assert_eq!(
                    line["outcome"]["values"]["operation"], "install_undo",
                    "{case}: {line}"
                );
                assert_eq!(
                    line["outcome"]["values"]["install_refusal"], "runtime_running",
                    "{case}: {line}"
                );
                assert_eq!(scene.files(), files, "{case}: nothing written");
            }
            scene.host.processes.replace(Vec::new());
        }

        let files = scene.files();
        let asked = scene.asked();
        let (planned, exit) = scene.undo(&operation_id, true);
        let case = format!("{way:?}: --plan");
        assert_undo_done(&planned, exit, &operation_id, true, &case);
        assert!(
            scene.files() == files,
            "{case}: the root or the game root changed"
        );
        assert_eq!(scene.asked(), asked, "{case}: nothing asked of the Runtime");
        let reverses = planned["reverses"].as_array().expect("reverses");
        assert!(
            reverses.iter().any(|step| step["n"] == schedule_step["n"]
                && step["kind"] == "pack_item"
                && step["target"] == schedule_step["target"]),
            "{case}: {planned}"
        );
        let numbers = reverses
            .iter()
            .map(|step| step["n"].as_u64().expect("a step number"))
            .collect::<Vec<_>>();
        assert!(
            numbers.windows(2).all(|pair| pair[0] > pair[1]),
            "{case}: from the last: {planned}"
        );

        let (undone, exit) = scene.undo(&operation_id, false);
        let case = format!("{way:?}: the undo done");
        assert_undo_done(&undone, exit, &operation_id, false, &case);
        assert_eq!(
            undone["schedule_changes"], planned["schedule_changes"],
            "{case}: the plan form's lists"
        );
        assert_eq!(
            notes(&undone),
            notes(&planned),
            "{case}: the plan form's warning"
        );
        assert!(undone.get("reverses").is_none(), "{case}: {undone}");
        assert!(planned.get("kept").is_none(), "{case}: {planned}");
    }
}

// Test 5: the journal's undo record carries the two lists, `op status` shows them, and a record
// without them (as an earlier build wrote it) loads and is answered without them.
#[test]
fn s318_the_record_carries_the_lists_and_one_without_them_loads() {
    let scene = staged(Way::Stopped, &with_back(true), &with_back(false));
    let operation_id = scene.placed();
    let (undone, exit) = scene.undo(&operation_id, false);
    assert_undo_done(&undone, exit, &operation_id, false, "the undo");
    let recorded = json!({"enters": [BACK], "leaves": []});
    let journal = scene.journal(&operation_id);
    assert_eq!(journal["undo"]["schedule_changes"], recorded, "{journal}");
    assert_eq!(journal["undo"]["initiator"], WORD.as_str(), "{journal}");

    let (status, exit) = scene.line("op status", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{status}");
    assert_eq!(
        status["journal"]["undo"]["schedule_changes"], recorded,
        "{status}"
    );

    // The record as an earlier build wrote it: without the member.
    let mut earlier = journal.clone();
    let record = earlier["undo"].as_object_mut().expect("the undo record");
    assert!(record.remove("schedule_changes").is_some());
    write_json(&scene.journal_path(&operation_id), &earlier);
    let (older, exit) = scene.line("op status", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{older}");
    let read = &older["journal"]["undo"];
    assert!(read["finished_at_unix_ms"].is_u64(), "{older}");
    assert!(read.get("schedule_changes").is_none(), "{older}");
}

// Test 6: the game root as the undo leaves it cannot be read (it puts back a `schedule.json` the
// reader refuses): no list, the second warning caused by that failure, and the undo still done as
// today, with a Runtime and without; its record's lists are `null`.
#[test]
fn s318_an_undone_root_that_cannot_be_read_warns_unknown_and_is_undone_hot_and_cold() {
    for way in WAYS {
        let scene = staged(way, &kept().schedule_bytes(UNREADABLE_SCHEDULE), &kept());
        let operation_id = scene.placed();
        let refused_at = scene.game_root.join("schedule.json");
        for plan in [true, false] {
            let case = format!("{way:?}: {}", form(plan));
            let (line, exit) = scene.undo(&operation_id, plan);
            assert_undo_done(&line, exit, &operation_id, plan, &case);
            let changes = &line["schedule_changes"];
            let mut members = changes
                .as_object()
                .map(|members| members.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            members.sort();
            assert_eq!(members, ["packages", "unread"], "{case}: {line}");
            let unread = changes["unread"].as_array().expect("unread");
            let [side] = unread.as_slice() else {
                panic!("{case}: one side unread: {line}");
            };
            assert_eq!(side["root"], "undone", "{case}: {line}");
            let failure = &side["failure"]["outcome"];
            assert_eq!(
                failure["code"], "standard_pack_root_invalid",
                "{case}: {line}"
            );
            assert_eq!(
                failure["values"],
                json!({
                    "game": GAME,
                    "path": refused_at.to_string_lossy(),
                    "standard_pack_problem": "schedule_invalid",
                    "json_pointer": "/neutral",
                }),
                "{case}: {line}"
            );
            let note = the_note(&line, "undo_scheduling_unknown", &case);
            assert!(note["values"].get("names").is_none(), "{case}: {line}");
            let causes = note["causes"].as_array().expect("the note's cause");
            let [cause] = causes.as_slice() else {
                panic!("{case}: one cause: {line}");
            };
            assert_eq!(cause["relation"], "caused_by", "{case}: {line}");
            assert_eq!(
                cause["code"], "standard_pack_root_invalid",
                "{case}: {line}"
            );
            assert_eq!(cause["values"], failure["values"], "{case}: {line}");
        }
        assert_eq!(
            fs::read(&refused_at).expect("schedule.json"),
            UNREADABLE_SCHEDULE,
            "{way:?}: put back as it was"
        );
        let journal = scene.journal(&operation_id);
        assert_eq!(
            journal["undo"]["schedule_changes"],
            json!({"enters": null, "leaves": null}),
            "{way:?}: {journal}"
        );
    }
}

// Test 8: the undo of another kind of operation (the root's `adopt`) answers as it did: its
// members and its outcome's values, no link, no record of scheduling; `--plan` of it is refused,
// and `actingctl install undo` takes no `--plan`, each with nothing written.
#[test]
fn s318_the_undo_of_another_kind_answers_as_before() {
    let scene = staged(Way::Stopped, &kept(), &kept());
    let journals = scene.journals();
    let [(operation_id, journal)] = journals.as_slice() else {
        panic!("the adopt journal alone: {journals:?}");
    };
    assert_eq!(journal["kind"], "adopt", "{journal}");

    let files = scene.files();
    let (refused, exit) = scene.undo(operation_id, true);
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        refused["outcome"]["code"], "actingctl_usage_invalid",
        "{refused}"
    );
    assert_eq!(
        refused["outcome"]["values"]["argument"], "--plan",
        "{refused}"
    );
    assert_eq!(
        refused["outcome"]["values"]["argument_problem"], "unusable",
        "{refused}"
    );
    assert!(
        scene.files() == files,
        "--plan of an adopt changed the root"
    );
    let face = run(
        &scene.host,
        &[
            "undo",
            "--root",
            word(&scene.root),
            "--operation",
            operation_id.as_str(),
            "--plan",
        ],
    );
    assert_eq!(code(&face), "actingctl_usage_invalid", "{face:?}");
    assert_eq!(
        face.outcome.value("argument").and_then(Value::as_str),
        Some("--plan"),
        "{face:?}"
    );
    assert_eq!(
        face.outcome
            .value("argument_problem")
            .and_then(Value::as_str),
        Some("unknown"),
        "{face:?}"
    );
    assert!(
        scene.files() == files,
        "actingctl install undo --plan changed the root"
    );

    let (undone, exit) = scene.undo(operation_id, false);
    assert_eq!(exit, 0, "{undone}");
    let mut members = undone
        .as_object()
        .map(|members| members.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    members.sort();
    assert_eq!(members, ["kept", "operation_id", "outcome"], "{undone}");
    assert_eq!(
        undone["outcome"]["code"], "install_command_done",
        "{undone}"
    );
    assert_eq!(
        undone["outcome"]["values"],
        json!({
            "operation": "install_undo",
            "install_operation_id": operation_id,
            "installation_changed": true,
        }),
        "{undone}"
    );
    assert!(
        undone["outcome"]["causes"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "{undone}"
    );
    let record = scene.journal(operation_id)["undo"].clone();
    let mut keys = record
        .as_object()
        .map(|record| record.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    keys.sort();
    assert_eq!(
        keys,
        ["finished_at_unix_ms", "initiator", "started_at_unix_ms"],
        "{record}"
    );
}
