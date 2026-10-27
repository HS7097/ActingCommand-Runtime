// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1c (#411; coordinator work order `rd-1027/brief-rl1c.md`, tests C-2, C-3, C-4 and C-6;
// board #411 6432236193 and 6435964241): actingd's readers of a composer-mode configuration (a
// `policy` without `catalog`, `catalog_approval_ids` and `procedure_manifest`, with
// `standard_packs.root`). Each test writes one neutral game root below the configuration's
// `standard_packs.root` (`bundle.json` and `schedule.json`, the rules built from the scheduling
// examples' catalog-a task), one fixture-backend instance bound to that game, and runs the
// subcommand in process; no device is opened and nothing waits.
//
// - C-3: check-config in composer mode notes `fact_reader_missing` exactly where catalog mode
//   notes it for the same rules: the configured catalog of the catalog-mode configuration is the
//   composition of the same files, written as the four documents.
// - C-4: in composer mode check-config admits the composed packages (a package refused with a
//   suspend code is a `pack_admission_suspended` note) and checks the remedy choice rows against
//   the composition (`policy_remedy_declaration_unusable`).
// - C-2: `actingd suspended` in composer mode no longer answers `suspended_policy_unconfigured`:
//   it composes the configuration's rules and goes on to its ledger (the report itself is
//   runtime-host's `gate_rl1c`).
// - C-6: a catalog-mode configuration answers as before: check-config reads its configured
//   catalog, not the game root's `schedule.json`, and `suspended` reads that catalog.

use super::*;
use actingcommand_contract::{IdentifierIssuer, StandardPackRootBytes, read_standard_pack_root};
use actingcommand_policy::{
    DEFAULT_HEAVY_THRESHOLD_MILLI, EvaluationFacts, EvaluationResources, HostResourceSnapshot,
    InstanceSnapshot, STANDARD_PACK_OPERATION_ID, StandardPackBinding, StandardPackComposeInput,
    StandardPackComposition, StandardPackGameFiles, compose_standard_pack_rules,
};
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Cursor, Write};
use zip::ZipWriter;
use zip::write::FileOptions;

#[path = "../../../tests/support/standard_pack_schedule.rs"]
mod schedule_fixture;
use schedule_fixture::{bundle, rule, schedule, write_json};

const GAME: &str = "fixture-game-a";
const SERVER: &str = "fixture-server-a";
const HOST: &str = "fixture-host-a";
const ALIAS: &str = "fixture-instance-a";
/// The rule whose trigger reads the realistic fact `resource.primary` in its instance's scope,
/// and the rule whose trigger reads only the clock.
const READS: &str = "fixture.reads";
const CLOCK_ONLY: &str = "fixture.clock";
const READS_PACKAGE: &str = "fixture.reads";
const CLOCK_PACKAGE: &str = "fixture.clock";
const READ_FACT: &str = "resource.primary";
/// The rule of the refused package (C-4), a package file, so the registry's directory names a
/// ZIP container; the `package_id` its control declares; the code that refuses it.
const REFUSED: &str = "fixture.refused";
const REFUSED_PACKAGE: &str = "fixture.refused.zip";
const REFUSED_ID: &str = "neutral.suspended.task";
const SUSPEND_CODE: &str = "recognition_sample_budget_invalid";
const APPROVAL: &str = "approval:fixture-a";

fn arguments(values: &[&OsStr]) -> Vec<OsString> {
    values.iter().map(|value| value.to_os_string()).collect()
}

/// The SR-3 package the kernel refuses at admission with `recognition_sample_budget_invalid`
/// (page `home` declares two frames 100 ms apart while the step timeout is 50 ms), as an
/// assembled ZIP container.
fn refused_package() -> Vec<u8> {
    let entries = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": REFUSED_ID,
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 2, "height": 1},
                "entry_task_id": "task",
                "capture_interval_ms": 1,
                "step_timeout_ms": 50,
                "timeout_ms": 5000
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": 2, "height": 1},
                "target_page": "terminal",
                "operations": [{
                    "id": "open_terminal",
                    "from": "home",
                    "to": "terminal",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true,
                    "retryable": false
                }]
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    {"type": "color", "id": "page/home",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                     "expected": [255, 0, 0]},
                    {"type": "color", "id": "page/terminal",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                     "expected": [0, 0, 255]},
                    {"type": "color", "id": "guard/ready",
                     "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                     "expected": [0, 255, 0]}
                ],
                "target_consensus": {
                    "page/home": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                                  "sample_interval_ms": 100}
                }
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": [
                {"id": "neutral/home", "required": ["page/home"], "optional": [],
                 "forbidden": []},
                {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
                 "forbidden": []}
            ]}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, &value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// The configuration's policy inputs: the instance bound to the game, able to run the Runtime's
/// package operation, on the one host.
fn policy_inputs() -> (Value, Value) {
    let facts = EvaluationFacts {
        ledger_position: 0,
        fact_snapshot_id: "snapshot:rl1c".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![InstanceSnapshot {
            instance_id: ALIAS.to_owned(),
            server_id: SERVER.to_owned(),
            game_id: GAME.to_owned(),
            host_id: HOST.to_owned(),
            available: true,
            capability_operation_ids: vec![STANDARD_PACK_OPERATION_ID.to_owned()],
            preferred_task_ids: Vec::new(),
        }],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: HOST.to_owned(),
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
            heavy_threshold_milli: DEFAULT_HEAVY_THRESHOLD_MILLI,
        }],
    };
    (
        serde_json::to_value(facts).expect("facts JSON"),
        serde_json::to_value(resources).expect("resources JSON"),
    )
}

/// One configuration folder: `packages/<game>/` with `bundle.json` registering `packages` and
/// `schedule.json` holding `rules`.
struct Fixture {
    folder: PathBuf,
    rules: Value,
}

impl Fixture {
    fn new(folder: PathBuf, packages: &[&str], rules: &[Value]) -> Self {
        let game = folder.join("packages").join(GAME);
        write_json(&game.join("bundle.json"), &bundle(GAME, SERVER, packages));
        let rules = schedule(rules);
        write_json(&game.join("schedule.json"), &rules);
        Self { folder, rules }
    }

    fn game(&self) -> PathBuf {
        self.folder.join("packages").join(GAME)
    }

    /// The configuration around `policy`, with one fixture-backend instance, the standard-pack
    /// root and a state root that does not exist; `extra` members are added at the top level.
    fn config(&self, name: &str, policy: Value, extra: &[(&str, Value)]) -> PathBuf {
        let instance_id = IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id");
        let mut config = json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": self.folder.join("state"),
            "bind_host": "127.0.0.1",
            "bind_port": 0,
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [{
                "alias": ALIAS,
                "instance_id": instance_id.transport(),
                "fixture_backend": {
                    "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                    "max_inputs": 1
                }
            }],
            "standard_packs": {"root": "packages"},
            "policy": policy
        });
        for (member, value) in extra {
            config[*member] = value.clone();
        }
        let path = self.folder.join(name);
        write_json(&path, &config);
        path
    }

    /// A composer-mode configuration: the policy inputs only.
    fn composer(&self, extra: &[(&str, Value)]) -> PathBuf {
        let (facts, resources) = policy_inputs();
        self.config(
            "composer.json",
            json!({"facts": facts, "resources": resources}),
            extra,
        )
    }

    /// A catalog-mode configuration whose catalog is the composition of `rules` (the same
    /// composer, the game's registry) written as the four documents under `policy/`, with an
    /// approval, and whose procedure bindings are the composition's procedures.
    fn catalog(&self, rules: &[Value]) -> PathBuf {
        let schedule = serde_json::to_vec(&schedule(rules)).expect("schedule bytes");
        let bundle = fs::read(self.game().join("bundle.json")).expect("read the bundle");
        let root = read_standard_pack_root(
            StandardPackRootBytes {
                bundle: Some(bundle.as_slice()),
                pages: None,
                interrupts: None,
                navigation: None,
                tables: false,
                display_layout: None,
            },
            GAME,
        )
        .expect("a sound game root")
        .expect("a game root");
        let instances = BTreeMap::from([(
            ALIAS.to_owned(),
            StandardPackBinding {
                game_id: GAME.to_owned(),
                server_id: SERVER.to_owned(),
            },
        )]);
        let games = BTreeMap::from([(
            GAME.to_owned(),
            StandardPackGameFiles {
                schedule: schedule.as_slice(),
                root: &root,
                maintenance: &[],
            },
        )]);
        let composition: StandardPackComposition =
            compose_standard_pack_rules(&StandardPackComposeInput {
                instances: &instances,
                games: &games,
                subsystems: None,
                catalog_version: 1,
            })
            .unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
        let sources = composition.sources();
        for (name, source) in [
            ("tasks.json", &sources.tasks),
            ("pools.json", &sources.pools),
            ("activity.json", &sources.activity),
            ("timeline.json", &sources.timeline),
        ] {
            let mut document: Value =
                serde_json::from_slice(&source.bytes).expect("a composed document");
            document["catalog"] = json!({
                "catalog_id": "fixture.catalog-rl1c",
                "catalog_version": 1,
                "approval_refs": [APPROVAL]
            });
            write_json(&self.folder.join("policy").join(name), &document);
        }
        let bindings = composition
            .manifest()
            .iter()
            .map(|procedure| {
                json!({
                    "procedure_ref": procedure.procedure_ref,
                    "operation_id": STANDARD_PACK_OPERATION_ID,
                    "yield_points": []
                })
            })
            .collect::<Vec<_>>();
        let (facts, resources) = policy_inputs();
        self.config(
            "catalog.json",
            json!({
                "facts": facts,
                "resources": resources,
                "catalog": {
                    "tasks": "policy/tasks.json",
                    "pools": "policy/pools.json",
                    "activity": "policy/activity.json",
                    "timeline": "policy/timeline.json"
                },
                "catalog_approval_ids": [APPROVAL],
                "procedure_manifest": bindings
            }),
            &[],
        )
    }
}

/// check-config of `config`, which must pass: its notes, in order.
fn check_notes(config: &Path) -> Vec<Outcome> {
    let reported = check_config::run(arguments(&[
        OsStr::new("check-config"),
        OsStr::new("--config"),
        config.as_os_str(),
    ]))
    .unwrap_or_else(|error| panic!("check-config passes: {}", error.outcome()));
    reported
        .outcome
        .causes()
        .iter()
        .filter(|link| link.relation().as_str() == "note")
        .map(|link| link.outcome().clone())
        .collect()
}

/// The values of the notes of `code`.
fn values_of(notes: &[Outcome], code: &str) -> Vec<Value> {
    notes
        .iter()
        .filter(|note| note.code().as_str() == code)
        .map(|note| Value::Object(note.values().clone()))
        .collect()
}

/// The code `suspended` refuses `config` with.
fn suspended_refusal(config: &Path) -> String {
    let refused = suspended::run(arguments(&[
        OsStr::new("suspended"),
        OsStr::new("--config"),
        config.as_os_str(),
    ]))
    .err()
    .unwrap_or_else(|| panic!("a state root that does not exist has no report"));
    refused.outcome().code().as_str().to_owned()
}

/// C-3 (work order item 3): for the same rules, check-config in composer mode notes
/// `fact_reader_missing` exactly where catalog mode notes it: the instance has no reader, the
/// rule that reads `resource.primary` in its scope gets one note naming the instance, the key and
/// the rule, and the clock-only rule gets none.
#[test]
fn rl1c_c3_composer_mode_notes_fact_reader_missing_where_catalog_mode_does() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let rules = [
        rule(READS, READS_PACKAGE, false),
        rule(CLOCK_ONLY, CLOCK_PACKAGE, true),
    ];
    let fixture = Fixture::new(
        temp.path().to_path_buf(),
        &[READS_PACKAGE, CLOCK_PACKAGE],
        &rules,
    );

    let composed = values_of(&check_notes(&fixture.composer(&[])), "fact_reader_missing");
    let configured = values_of(
        &check_notes(&fixture.catalog(&rules)),
        "fact_reader_missing",
    );

    assert_eq!(
        composed,
        vec![json!({"instance_id": ALIAS, "fact_key": READ_FACT, "task_id": READS})],
        "the composition's notes ({:#})",
        fixture.rules
    );
    assert_eq!(
        composed, configured,
        "the catalog's notes for the same rules"
    );
}

/// C-4 (work order item 3): in composer mode check-config admits every package the composed
/// procedures locate (the refused package is a `pack_admission_suspended` note naming the
/// package its control declares and the refusal) and builds the remedy table over the
/// composition (a configured `allow` row whose need no composed package declares is a
/// `policy_remedy_declaration_unusable` note, `choice_invalid`).
#[test]
fn rl1c_c4_composer_mode_check_config_runs_the_admission_and_remedy_checks() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let fixture = Fixture::new(
        temp.path().to_path_buf(),
        &[REFUSED_PACKAGE],
        &[rule(REFUSED, REFUSED_PACKAGE, true)],
    );
    fs::write(fixture.game().join(REFUSED_PACKAGE), refused_package()).expect("the package");
    let config = fixture.composer(&[(
        "scheduling",
        json!({"remedy": {"choices": [
            {"row": "allow", "need": "fixture_need", "capability": "fixture_capability"}
        ]}}),
    )]);

    let notes = check_notes(&config);

    let suspended = values_of(&notes, "pack_admission_suspended");
    assert_eq!(suspended.len(), 1, "{notes:#?}");
    assert_eq!(suspended[0]["package_id"], json!(REFUSED_ID), "{notes:#?}");
    assert_eq!(
        suspended[0]["failure_code"],
        json!(SUSPEND_CODE),
        "{notes:#?}"
    );
    let unusable = values_of(&notes, "policy_remedy_declaration_unusable");
    assert_eq!(unusable.len(), 1, "{notes:#?}");
    assert_eq!(
        unusable[0]["remedy_problem"],
        json!("choice_invalid"),
        "{notes:#?}"
    );
    assert_eq!(unusable[0]["need"], json!("fixture_need"), "{notes:#?}");
}

/// C-2 (work order item 2): `actingd suspended` in composer mode composes the configuration's
/// rules and goes on to read its ledger (here a state root that does not exist,
/// `suspended_ledger_unavailable`) instead of answering `suspended_policy_unconfigured`.
#[test]
fn rl1c_c2_suspended_reads_the_composition_in_composer_mode() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let fixture = Fixture::new(
        temp.path().to_path_buf(),
        &[READS_PACKAGE],
        &[rule(READS, READS_PACKAGE, false)],
    );

    assert_eq!(
        suspended_refusal(&fixture.composer(&[])),
        "suspended_ledger_unavailable"
    );
}

/// C-6 (work order, tests): a catalog-mode configuration answers as before. Its configured
/// catalog holds only the clock-only rule while the game root's `schedule.json` also holds the
/// rule that reads `resource.primary`: check-config reads the configured catalog and notes no
/// `fact_reader_missing`, and `suspended` reads that catalog and goes on to its ledger.
#[test]
fn rl1c_c6_a_catalog_mode_configuration_reads_its_configured_catalog() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let fixture = Fixture::new(
        temp.path().to_path_buf(),
        &[READS_PACKAGE, CLOCK_PACKAGE],
        &[
            rule(READS, READS_PACKAGE, false),
            rule(CLOCK_ONLY, CLOCK_PACKAGE, true),
        ],
    );
    let config = fixture.catalog(&[rule(CLOCK_ONLY, CLOCK_PACKAGE, true)]);

    let notes = check_notes(&config);
    assert!(
        values_of(&notes, "fact_reader_missing").is_empty(),
        "{notes:#?}"
    );
    assert_eq!(suspended_refusal(&config), "suspended_ledger_unavailable");
}
