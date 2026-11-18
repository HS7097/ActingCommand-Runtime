// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S5b (register §1.474; contracts/install-cli.md, "The composition after a
// placement"; coordinator work order `rd-1118/brief-453-s5b-composition-check.md` and its rulings
// `rd-1118/brief-453-s5b-rulings.md`): after a placement with a Runtime whose game's
// `schedule.json` changed while an instance was bound to the game, the row `composition` compares
// rules digests: the one the live build's check-config previews for the installation as placed
// (its report's `standard_pack_schedule.rules_digest`) against the one of the generation the
// Runtime's latest evaluation ran under (the composition record of that generation below the
// state root). Every scene runs the fake Runtime with the instance [`INSTANCE`] bound to [`GAME`];
// the placement holds it.
// - `s5b_rules_naming_held_packages_pass_at_the_first_read` (the case observed on production
//   0.12.72): the bundle's `schedule.json` gains only two rules, each naming a held package, so
//   the placed root composes the rules of the generation read before the hold; the row passes at
//   its first read with `expected` `unchanged`, `schedule_changed_rules_unchanged` true and the
//   digest, without a wait; the row `check_config` beside it passes; the operation is done and
//   hands its hold back;
// - `s5b_a_rule_that_enters_waits_for_the_expected_digest_not_a_third`: the bundle adds a rule
//   naming a package no rule named; the generation the check reads first is a third one, whose
//   digest is neither the one before nor the expected one: the row waits 500 ms and reads again,
//   and passes with `expected` `new` on the generation of the expected digest;
// - `s5b_a_third_composition_alone_fails_with_the_three_digests`: the same placement, and the
//   Runtime stays on the third generation: the row fails when its wait ends, with `waited_ms` and
//   the three digests (before the hold, expected, read); the operation ends `post_check_failed`
//   with the hold kept.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device, no
// sleep, no clock read; every root is a temporary folder with neutral files (made-up ids, no real
// game). The fixture's check-config answers the digest a test sets; the composition records are
// written where the Runtime's catalog store keeps them.

use super::fixture::{
    BuildSpec, C1, FakeHost, REPOSITORY, code, live_root, preview_rules_digest, run, word,
    write_composed_generation, write_json,
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
/// The neutral game, its one server and the instance the fake Runtime binds to it.
const GAME: &str = "neutral-game";
const SERVER: &str = "neutral-server";
const INSTANCE: &str = "neutral.a";
/// The `package` range the installer of these tests declares, and the one every bundle declares.
const INSTALLER_PACKAGE: [u64; 2] = [3, 10];
const RELEASED: [u64; 2] = [3, 3];
/// The packs: one a rule schedules, two held, and one a later rule names.
const SCHEDULED: &str = "neutral.scheduled";
const HELD_A: &str = "neutral.held-a";
const HELD_B: &str = "neutral.held-b";
const ENTERING: &str = "neutral.entering";
/// Every pack's `control.json` (placeholder bytes).
const CONTROL: &[u8] = br#"{"neutral": 1}"#;
/// The reason of every held entry.
const HOLD_REASON: &str = "neutral hold";
/// The reference of the authorization the placements name (a made-up board reference).
const REFERENCE: &str = "board-453-comment-neutral-s5b";
/// Three generations of the Runtime, as it names them (`sha256:` and 64 hex digits): the one its
/// latest evaluation ran under before the hold, a third one, and the one of the expected digest.
const GENERATION_BEFORE: &str = concat!(
    "sha256:",
    "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b",
    "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b"
);
const GENERATION_THIRD: &str = concat!(
    "sha256:",
    "0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c",
    "0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c"
);
const GENERATION_EXPECTED: &str = concat!(
    "sha256:",
    "0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e",
    "0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e"
);
/// The rules digests of those three generations (made up).
const DIGEST_BEFORE: &str = "sha256:neutral-rules-before";
const DIGEST_THIRD: &str = "sha256:neutral-rules-third";
const DIGEST_EXPECTED: &str = "sha256:neutral-rules-expected";

/// One game root's content, as a bundle zip carries it and as an installed game root holds it.
#[derive(Default)]
struct Layout {
    /// Each pack's id, in `bundle.json`'s order.
    packs: Vec<&'static str>,
    /// `schedule.json`'s rules, each `(rule id, package id)`.
    rules: Vec<(&'static str, &'static str)>,
    /// `schedule.json`'s held packages.
    held: Vec<&'static str>,
}

impl Layout {
    fn pack(mut self, package_id: &'static str) -> Self {
        self.packs.push(package_id);
        self
    }

    fn rule(mut self, rule_id: &'static str, package_id: &'static str) -> Self {
        self.rules.push((rule_id, package_id));
        self
    }

    fn hold(mut self, package_id: &'static str) -> Self {
        self.held.push(package_id);
        self
    }

    /// The root files, each `(name, bytes)`: `bundle.json` and `schedule.json`.
    fn root_files(&self) -> Vec<(&'static str, Vec<u8>)> {
        let packs = self
            .packs
            .iter()
            .map(|package_id| {
                json!({
                    "package_id": package_id,
                    "server": SERVER,
                    "entry_task_id": format!("{package_id}.entry"),
                    "path": format!("packs/{package_id}"),
                    "file_count": 1,
                    "byte_count": CONTROL.len(),
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
            .map(|package_id| json!({"package_id": package_id, "reason": HOLD_REASON}))
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
            (
                "schedule.json",
                serde_json::to_vec(&json!({
                    "schema_version": "actingcommand.schedule.v1",
                    "rules": rules,
                    "held": held,
                }))
                .expect("JSON"),
            ),
        ]
    }

    /// Lays the root out as an installed game root: each pack's `<game root>\<id>\control.json`,
    /// each root file.
    fn lay_out(&self, game_root: &Path) {
        for package_id in &self.packs {
            fs::create_dir_all(game_root.join(package_id)).expect("a pack folder");
            fs::write(game_root.join(package_id).join("control.json"), CONTROL).expect("a pack");
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
        for package_id in &self.packs {
            entries.push((format!("packs/{package_id}/control.json"), CONTROL.to_vec()));
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

/// One root, adopted, whose configuration names the standard-pack root `<temp>\s5b-packs`, with
/// the game root laid out as `before`, the fake Runtime running [`INSTANCE`] bound to [`GAME`],
/// and the bundle zip of `after`.
struct Scene {
    host: FakeHost,
    root: PathBuf,
    state_root: PathBuf,
    zip: PathBuf,
    _temp: TempDir,
}

fn staged(before: &Layout, after: &Layout) -> Scene {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), "s5b", &BuildSpec::new(C1));
    let packs = temp.path().join("s5b-packs");
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
    host.runtime.boot(&[INSTANCE]);
    for instance in host.runtime.instances.borrow_mut().iter_mut() {
        instance.game = Some(GAME.to_owned());
    }
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config_path)
        .expect("the configured state root");
    let zip = temp.path().join("neutral-bundle.zip");
    after.write_zip(&zip);
    Scene {
        host,
        root,
        state_root,
        zip,
        _temp: temp,
    }
}

impl Scene {
    /// The rescue row `name` with `params` on the root, as [`WORD`]: its one line and exit code.
    fn line(&self, name: &str, params: Value) -> (Value, u8) {
        let config = self.root.join("actingd.config.json");
        let location = rescue::location_of(&self.root, &config, &self.state_root);
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

    /// The bundle placed with [`REFERENCE`] and, when its plan lists packages that enter
    /// scheduling, those as `enables`: the line, its exit code, and the one journal it wrote.
    fn place(&self) -> (Value, u8, Value) {
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
        let mut written = self
            .journals()
            .into_iter()
            .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
            .collect::<Vec<_>>();
        assert_eq!(written.len(), 1, "{written:?}");
        let (_, journal) = written.remove(0);
        assert_eq!(journal["kind"], "pack_place", "{journal}");
        (placed, exit, journal)
    }

    /// Every call of the fake Runtime that starts with `prefix`.
    fn calls_of(&self, prefix: &str) -> Vec<String> {
        self.host
            .runtime
            .calls
            .borrow()
            .iter()
            .filter(|call| call.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// The live build's check-config runs the fake host made after its first `since` runs and
    /// steps, each as its log line.
    fn check_config_runs(&self, since: usize) -> Vec<String> {
        self.host
            .log
            .borrow()
            .iter()
            .skip(since)
            .filter(|entry| entry.starts_with("live actingcommand-actingd.exe check-config "))
            .cloned()
            .collect()
    }
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

/// The journal's `post_check` step.
fn check_step(journal: &Value) -> Value {
    journal["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .find(|step| step["kind"] == "post_check")
        .cloned()
        .unwrap_or_else(|| panic!("the checks after the placement: {journal}"))
}

/// The rows of the journal's `post_check` step, in order.
fn check_rows(journal: &Value) -> Vec<Value> {
    check_step(journal)["evidence"]["checks"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the rows of the checks: {journal}"))
}

/// The row `id` of the journal's `post_check` step.
fn check_row(journal: &Value, id: &str) -> Value {
    check_rows(journal)
        .into_iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("no row {id}: {journal}"))
}

/// The ids of the journal's rows, in order.
fn row_ids(journal: &Value) -> Vec<String> {
    check_rows(journal)
        .iter()
        .map(|row| row["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The names of an object's members, sorted.
fn keys(value: &Value) -> Vec<String> {
    let mut keys = value
        .as_object()
        .map(|members| members.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// The installed root: [`SCHEDULED`] named by a rule, [`HELD_A`] and [`HELD_B`] held.
fn with_holds() -> Layout {
    Layout::default()
        .pack(SCHEDULED)
        .pack(HELD_A)
        .pack(HELD_B)
        .rule("rule.scheduled", SCHEDULED)
        .hold(HELD_A)
        .hold(HELD_B)
}

/// The installed root: [`SCHEDULED`] named by a rule, [`ENTERING`] named by none.
fn without_entering_rule() -> Layout {
    Layout::default()
        .pack(SCHEDULED)
        .pack(ENTERING)
        .rule("rule.scheduled", SCHEDULED)
}

// The case observed on production 0.12.72 (11-18): the bundle's `schedule.json` is the installed
// one with two more rules, each naming a held package, every other file the same. A rule that
// names a held package is not composed, so check-config previews the rules digest of the
// generation the Runtime had (the fake check-config answers [`DIGEST_BEFORE`]), and the Runtime's
// latest evaluation runs under that generation. The placement replaces `schedule.json` alone; the
// rows are `rerun`, `standard_pack_check`, `check_config` (pass: the live build's check-config, run
// once, with `--install-root`) and `composition`, which passes at its first read with `expected`
// `unchanged`, `schedule_changed_rules_unchanged` true and the digest: three reads of the catalog
// in all (before the hold, the check's one read, the schedule after the release), no wait. The
// operation is done and its hold handed back (the instance resumed, `kept` false).
#[test]
fn s5b_rules_naming_held_packages_pass_at_the_first_read() {
    let after = with_holds()
        .rule("rule.held-a", HELD_A)
        .rule("rule.held-b", HELD_B);
    let scene = staged(&with_holds(), &after);
    write_composed_generation(&scene.state_root, GENERATION_BEFORE, DIGEST_BEFORE);
    *scene.host.runtime.catalog.borrow_mut() = Some(GENERATION_BEFORE.to_owned());
    preview_rules_digest(&scene.host, DIGEST_BEFORE);

    let logged = scene.host.log.borrow().len();
    let (placed, exit, journal) = scene.place();
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(
        placed["outcome"]["code"], "install_command_done",
        "{placed}"
    );
    assert_eq!(
        step_kinds(&journal),
        ["pause", "pack_item", "post_check", "release_hold"],
        "{journal}"
    );
    assert_eq!(
        journal["steps"][1]["intent"]["item"], "schedule.json",
        "{journal}"
    );
    assert_eq!(journal["inputs"]["bound"], json!([INSTANCE]), "{journal}");
    assert_eq!(
        row_ids(&journal),
        [
            "rerun",
            "standard_pack_check",
            "check_config",
            "composition"
        ],
        "{journal}"
    );
    assert_eq!(
        check_row(&journal, "check_config")["result"],
        "pass",
        "{journal}"
    );
    assert_eq!(
        check_row(&journal, "composition"),
        json!({"id": "composition", "result": "pass", "values": {
            "catalog_hash": GENERATION_BEFORE,
            "catalog_before": GENERATION_BEFORE,
            "expected": "unchanged",
            "rules_digest": DIGEST_BEFORE,
            "schedule_changed_rules_unchanged": true,
        }}),
        "{journal}"
    );
    assert_eq!(check_step(&journal)["evidence"]["failed"], json!([]));
    let runs = scene.check_config_runs(logged);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0].contains(" --install-root "), "{runs:?}");
    assert_eq!(
        scene.calls_of("catalog").len(),
        3,
        "{:?}",
        scene.host.runtime.calls.borrow()
    );
    assert_eq!(scene.calls_of("resume "), [format!("resume {INSTANCE}")]);
    assert_eq!(
        placed["steps"][1]["result"]["hold"]["kept"],
        json!(false),
        "{placed}"
    );
}

// A rule that enters: the bundle adds a rule naming [`ENTERING`], which no rule named, so
// check-config previews another rules digest ([`DIGEST_EXPECTED`]). The check's first read finds
// the Runtime on a third generation, whose digest is neither the one before nor the expected one:
// the row does not pass there; it waits 500 ms, reads again, finds the generation of the expected
// digest and passes with `expected` `new` (four reads of the catalog in all). The operation is
// done, and the journal records the package it enabled.
#[test]
fn s5b_a_rule_that_enters_waits_for_the_expected_digest_not_a_third() {
    let after = without_entering_rule().rule("rule.entering", ENTERING);
    let scene = staged(&without_entering_rule(), &after);
    write_composed_generation(&scene.state_root, GENERATION_BEFORE, DIGEST_BEFORE);
    write_composed_generation(&scene.state_root, GENERATION_THIRD, DIGEST_THIRD);
    write_composed_generation(&scene.state_root, GENERATION_EXPECTED, DIGEST_EXPECTED);
    *scene.host.runtime.catalog.borrow_mut() = Some(GENERATION_BEFORE.to_owned());
    scene.host.runtime.catalogs.borrow_mut().extend([
        GENERATION_BEFORE.to_owned(),
        GENERATION_THIRD.to_owned(),
        GENERATION_EXPECTED.to_owned(),
    ]);
    preview_rules_digest(&scene.host, DIGEST_EXPECTED);

    let (placed, exit, journal) = scene.place();
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(
        placed["outcome"]["code"], "install_command_done",
        "{placed}"
    );
    assert_eq!(journal["inputs"]["enabled"], json!([ENTERING]), "{journal}");
    assert_eq!(
        check_row(&journal, "composition"),
        json!({"id": "composition", "result": "pass", "values": {
            "catalog_hash": GENERATION_EXPECTED,
            "catalog_before": GENERATION_BEFORE,
            "expected": "new",
            "rules_digest": DIGEST_EXPECTED,
        }}),
        "{journal}"
    );
    assert_eq!(
        scene.calls_of("catalog").len(),
        4,
        "{:?}",
        scene.host.runtime.calls.borrow()
    );
}

// The same placement, and the Runtime's latest evaluation stays on the third generation: the row
// reads it again every 500 ms until its wait ends and fails with `waited_ms` and the three digests,
// before the hold ([`DIGEST_BEFORE`]), expected ([`DIGEST_EXPECTED`]) and read ([`DIGEST_THIRD`]),
// beside the catalog it read and `expected` `new`. The operation ends `post_check_failed` with that
// row alone failed, and its hold is kept: the instance is not resumed.
#[test]
fn s5b_a_third_composition_alone_fails_with_the_three_digests() {
    let after = without_entering_rule().rule("rule.entering", ENTERING);
    let scene = staged(&without_entering_rule(), &after);
    write_composed_generation(&scene.state_root, GENERATION_BEFORE, DIGEST_BEFORE);
    write_composed_generation(&scene.state_root, GENERATION_THIRD, DIGEST_THIRD);
    write_composed_generation(&scene.state_root, GENERATION_EXPECTED, DIGEST_EXPECTED);
    *scene.host.runtime.catalog.borrow_mut() = Some(GENERATION_BEFORE.to_owned());
    scene
        .host
        .runtime
        .catalogs
        .borrow_mut()
        .extend([GENERATION_BEFORE.to_owned(), GENERATION_THIRD.to_owned()]);
    preview_rules_digest(&scene.host, DIGEST_EXPECTED);

    let (placed, exit, journal) = scene.place();
    assert_eq!(exit, 1, "{placed}");
    assert_eq!(
        placed["outcome"]["values"]["install_refusal"], "post_check_failed",
        "{placed}"
    );
    let composition = check_row(&journal, "composition");
    assert_eq!(composition["result"], "fail", "{journal}");
    let values = &composition["values"];
    assert_eq!(
        keys(values),
        [
            "catalog_hash",
            "expected",
            "rules_digest",
            "rules_digest_before",
            "rules_digest_expected",
            "waited_ms"
        ],
        "{journal}"
    );
    assert_eq!(values["catalog_hash"], GENERATION_THIRD, "{journal}");
    assert_eq!(values["expected"], "new", "{journal}");
    assert_eq!(values["rules_digest_before"], DIGEST_BEFORE, "{journal}");
    assert_eq!(
        values["rules_digest_expected"], DIGEST_EXPECTED,
        "{journal}"
    );
    assert_eq!(values["rules_digest"], DIGEST_THIRD, "{journal}");
    assert!(
        values["waited_ms"]
            .as_u64()
            .is_some_and(|waited| waited >= 500),
        "{journal}"
    );
    assert_eq!(
        check_step(&journal)["evidence"]["failed"],
        json!(["composition"]),
        "{journal}"
    );
    assert!(
        scene.calls_of("resume ").is_empty(),
        "{:?}",
        scene.host.runtime.calls.borrow()
    );
}
