// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #464 P3 (register §1.375; contracts/standard-pack-schedule.md, "Readiness"; the design
// rd-1114/464-pack-build-design.md F.1 and G, row P3, with the coordinator's rulings 3 and 7;
// coordinator work order rd-1114/brief-464-p3-ready.md): `judge_standard_pack_readiness`, the one
// function that judges the twelve readiness items of every pack of a game root.
// - `p3_a_complete_pack_answers_dispatchable`: on a made-up installed game root whose one business
//   pack has a rule with a complete value, the return-home package of its server and every part of
//   an installation read, the pack answers `dispatchable`: S1 to S6 and I1, I2, I4 to I6 hold, I3 is
//   an information line (`package_readiness_unjudged`, `no_revision_tells`), S2 by `scheduled`, S3
//   by `complete`, S6 and I6 name the pack's one model;
// - `p3_without_an_installation_only_the_root_items_are_answered`: the same root without an
//   installation answers `installation: not_read`, the six root items hold, the five installation
//   items are `not_judged` with `installation_not_read`, I3 is information, the pack `not_judged`;
// - one test per item that can fail, each changing one thing of the complete fixture, and holding
//   that exactly that item fails (`package_dispatch_waiting` with the item, its problem and its
//   values, the other check's outcome as its cause where one exists) and the pack `waiting`:
//   S1 (a task file that is no JSON; S5, S6 and I6 then `not_judged`, `not_admitted`), S2 (no rule:
//   `unclassified`; a held package: `held`), S3 (`value_pending`; `value_undeclared`), S4 (a rule of
//   a package the root does not hold: `composition_refused`; S2, S3 and I4 then `not_judged`),
//   S5 (a prerequisite package the root does not hold; and one it holds, which holds), I1
//   (`instance_unbound`; `server_unbound`; a binding without its server `not_judged`), I2
//   (`revision_outside`), I4 (`requires_unknown`), I5 (`pair_suspended`), I6 (`model_missing`).
//   S6 cannot fail alone: the loader refuses a target that names no model, which is S1's.
// Made-up packs only; no device, no Runtime, no clock read, no sleep.

use crate::codes::{ReadinessItem, ReadinessProblem, ReadinessUnjudged, ResourceToolingCode};
use crate::{
    InstallationRead, ItemState, PackReadiness, PackState, ReadinessBasis, ReadinessBinding,
    ReadinessInstallation, ReadinessModel, ReadinessSuspension, StandardPackReadiness,
    StandardPackReadyRequest, judge_standard_pack_readiness,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The business pack every test judges.
const CLAIM: &str = "neutral.test.claim";
/// The return-home package of the server.
const HOME: &str = "neutral.test.home";
/// A package the game root does not hold.
const ABSENT: &str = "neutral.test.absent";
/// The business pack's one rule.
const RULE: &str = "neutral.test.claim.rule";
/// The pool the rule produces into, with a valuation.
const VALUED: &str = "pool.valued";
/// The one `requires` word of the rules, which the complete installation names.
const SUBSYSTEM: &str = "fixture_subsystem";
/// The model the business pack's OCR target names.
const MODEL: (&str, &str) = (
    "neutral-ocr-zeta",
    "1111111111111111111111111111111111111111111111111111111111111111",
);
/// An instance of the installation.
const INSTANCE: &str = "instance_0000000000000000000000000000000a";

fn rect(x: u32, y: u32, width: u32, height: u32) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": rect(0, 0, 1, 1)},
        "expected": expected
    })
}

/// The control of the page-graph pack `package_id`, as `gate_453_s8` writes it.
fn control(package_id: &str) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 64, "height": 24},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    })
}

/// A 64x24 page graph whose page `home` is told by a blue first pixel and `page_b` by a red one,
/// one trusted click from `home` to `page_b`; with `model`, `page_b` also requires an OCR target
/// of that model (as `gate_453_s8` writes it).
fn task(model: Option<(&str, &str)>) -> Value {
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "P3 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    if let Some((model_ref, model_sha256)) = model {
        task["ocr_targets"] = json!([{
            "id": "title/claim", "region": {"mode": "rect", "rect": rect(34, 2, 12, 8)},
            "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["7"],
            "case_sensitive": false, "minimum_confidence": 0.9,
            "model_ref": model_ref, "model_sha256": model_sha256
        }]);
        if let Some(required) = task["page_rules"]["page_b"]["required"].as_array_mut() {
            required.push(json!("title/claim"));
        }
    }
    task
}

/// A rule of `package_id` with the id `id` on an hourly clock that produces `produces` (as
/// `gate_ss2b` writes them).
fn rule(id: &str, package_id: &str, produces: Vec<Value>) -> Value {
    json!({
        "id": id,
        "package_id": package_id,
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3600000,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": produces,
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86400000,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    })
}

fn produce(amount: Value) -> Value {
    json!({
        "pool_id": VALUED,
        "direction": "produce",
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// What one test writes: the complete fixture, of which a test changes one thing.
struct Fixture {
    /// The business pack's control.
    control: Value,
    /// Its task; `None` writes a task file that is no JSON.
    task: Option<Value>,
    rules: Vec<Value>,
    held: Vec<Value>,
}

impl Fixture {
    fn complete() -> Self {
        Self {
            control: control(CLAIM),
            task: Some(task(Some(MODEL))),
            rules: vec![rule(RULE, CLAIM, vec![produce(json!(1))])],
            held: Vec::new(),
        }
    }

    /// The installed game root `<temp>/packages/neutral`: the business pack and the server's
    /// return-home package, `bundle.json` naming the latter for `return_home`, and `schedule.json`.
    fn write(&self, temp: &TempDir) -> PathBuf {
        let root = temp.path().join("packages").join(GAME);
        let task = match &self.task {
            Some(task) => serde_json::to_vec_pretty(task).expect("task JSON"),
            None => b"{".to_vec(),
        };
        write_pack(&root.join(CLAIM), &self.control, &task);
        let home = serde_json::to_vec_pretty(&task_without_model()).expect("task JSON");
        write_pack(&root.join(HOME), &control(HOME), &home);
        let listed = [CLAIM, HOME]
            .iter()
            .map(|package_id| {
                json!({
                    "package_id": package_id, "server": SERVER, "entry_task_id": "task",
                    "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
                })
            })
            .collect::<Vec<_>>();
        write_json(
            &root.join("bundle.json"),
            &json!({
                "schema_version": "actingcommand.bundle.v4",
                "game": GAME,
                "packs": listed,
                "maintenance": [{"package_id": HOME, "server": SERVER, "uses": ["return_home"]}]
            }),
        );
        let valuation =
            json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
        write_json(
            &root.join("schedule.json"),
            &json!({
                "schema_version": "actingcommand.schedule.v1",
                "requires": [SUBSYSTEM],
                "rules": self.rules,
                "pools": [{
                    "id": VALUED,
                    "capacity": 1000,
                    "projection": {"kind": "none"},
                    "observation": {"kind": "fact", "fact_key": "resource.valued"},
                    "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
                    "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
                    "valuation": valuation
                }],
                "activity": [],
                "timeline": [],
                "held": self.held
            }),
        );
        root
    }
}

fn task_without_model() -> Value {
    task(None)
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("file");
}

/// The pack's files in `directory`: its control, its resources index and its task.
fn write_pack(directory: &Path, control: &Value, task: &[u8]) {
    write_json(&directory.join("control.json"), control);
    write_json(
        &directory.join("resources/operations/resources.json"),
        &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
    );
    let task_path = directory.join("resources/operations/task/task.json");
    fs::create_dir_all(task_path.parent().expect("a parent")).expect("directory");
    fs::write(task_path, task).expect("task file");
}

/// Every part of an installation read, each as the complete fixture needs it.
fn installation() -> ReadinessInstallation {
    ReadinessInstallation {
        bound: Some(vec![ReadinessBinding {
            instance_id: INSTANCE.to_owned(),
            game: GAME.to_owned(),
            server: Some(SERVER.to_owned()),
        }]),
        package: Some([3, 12]),
        subsystems: Some(BTreeSet::from([SUBSYSTEM.to_owned()])),
        suspended: Some(Vec::new()),
        models: Some(vec![model()]),
    }
}

fn model() -> ReadinessModel {
    ReadinessModel {
        model_ref: MODEL.0.to_owned(),
        model_sha256: MODEL.1.to_owned(),
    }
}

fn judge(root: &Path, installation: Option<ReadinessInstallation>) -> StandardPackReadiness {
    judge_standard_pack_readiness(&StandardPackReadyRequest {
        root: root.to_path_buf(),
        declared_package: Some([12, 12]),
        installation,
    })
    .unwrap_or_else(|error| panic!("the readiness: {}: {}", error.code, error.message))
}

/// The business pack's answer.
fn claim(readiness: &StandardPackReadiness) -> &PackReadiness {
    readiness
        .packs
        .iter()
        .find(|pack| pack.package_id == CLAIM)
        .unwrap_or_else(|| panic!("the business pack is answered: {readiness:?}"))
}

/// The items of `pack` in `state`.
fn in_state(pack: &PackReadiness, state: ItemState) -> Vec<ReadinessItem> {
    pack.items
        .iter()
        .filter(|answer| answer.state == state)
        .map(|answer| answer.item)
        .collect()
}

/// The outcome of `item` of `pack`.
fn outcome_of(pack: &PackReadiness, item: ReadinessItem) -> &Outcome {
    pack.item(item)
        .and_then(|answer| answer.outcome.as_ref())
        .unwrap_or_else(|| panic!("{} carries an outcome: {pack:?}", item.as_str()))
}

/// Holds that exactly `item` of the business pack fails, with `problem`, and that the pack waits;
/// the failing item's outcome.
fn fails_alone(
    readiness: &StandardPackReadiness,
    item: ReadinessItem,
    problem: ReadinessProblem,
) -> Outcome {
    let pack = claim(readiness);
    assert_eq!(in_state(pack, ItemState::Fails), [item], "{pack:?}");
    assert_eq!(pack.readiness, PackState::Waiting, "{pack:?}");
    let outcome = outcome_of(pack, item).clone();
    assert!(
        outcome.is(ResourceToolingCode::PackageDispatchWaiting),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("package_id"),
        Some(&json!(CLAIM)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("readiness_item"),
        Some(&json!(item.as_str())),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("readiness_problem"),
        Some(&json!(problem.as_str())),
        "{outcome:?}"
    );
    outcome
}

/// Holds that `item` of `pack` is not judged for `why`.
fn not_judged(pack: &PackReadiness, item: ReadinessItem, why: ReadinessUnjudged) {
    let answer = pack
        .item(item)
        .unwrap_or_else(|| panic!("{} is answered", item.as_str()));
    assert_eq!(answer.state, ItemState::NotJudged, "{answer:?}");
    let outcome = outcome_of(pack, item);
    assert!(
        outcome.is(ResourceToolingCode::PackageReadinessUnjudged),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("readiness_unjudged"),
        Some(&json!(why.as_str())),
        "{outcome:?}"
    );
}

/// The code of the first cause of `outcome`.
fn cause_is(outcome: &Outcome, code: impl Into<actingcommand_contract::outcome::Code>) -> bool {
    outcome
        .causes()
        .first()
        .is_some_and(|link| link.outcome().is(code))
}

#[test]
fn p3_a_complete_pack_answers_dispatchable() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::complete().write(&temp);

    let readiness = judge(&root, Some(installation()));

    assert_eq!(readiness.game, GAME);
    assert_eq!(readiness.installation, InstallationRead::Read);
    assert_eq!(
        readiness
            .packs
            .iter()
            .map(|pack| pack.package_id.as_str())
            .collect::<Vec<_>>(),
        [CLAIM, HOME],
        "every pack of the registry, in its order"
    );
    let pack = claim(&readiness);
    assert_eq!(pack.server, SERVER);
    assert_eq!(pack.readiness, PackState::Dispatchable, "{pack:?}");
    assert_eq!(pack.items.len(), 12, "{pack:?}");
    assert_eq!(
        in_state(pack, ItemState::Holds),
        [
            ReadinessItem::Admitted,
            ReadinessItem::Classified,
            ReadinessItem::Valued,
            ReadinessItem::Composed,
            ReadinessItem::EntryReachable,
            ReadinessItem::ModelsNamed,
            ReadinessItem::InstanceBound,
            ReadinessItem::PackageRevision,
            ReadinessItem::RequiresKnown,
            ReadinessItem::PairUnsuspended,
            ReadinessItem::ModelsInstalled,
        ],
        "{pack:?}"
    );
    assert_eq!(
        in_state(pack, ItemState::Information),
        [ReadinessItem::UnrecordedScreenCode],
        "I3 is an information line"
    );
    let information = outcome_of(pack, ReadinessItem::UnrecordedScreenCode);
    assert!(information.is(ResourceToolingCode::PackageReadinessUnjudged));
    assert_eq!(
        information.value("readiness_unjudged"),
        Some(&json!(ReadinessUnjudged::NoRevisionTells.as_str()))
    );
    let basis = |item| pack.item(item).and_then(|answer| answer.basis);
    assert_eq!(
        basis(ReadinessItem::Classified),
        Some(ReadinessBasis::Scheduled)
    );
    assert_eq!(basis(ReadinessItem::Valued), Some(ReadinessBasis::Complete));
    for item in [ReadinessItem::ModelsNamed, ReadinessItem::ModelsInstalled] {
        let answer = pack.item(item).expect("answered");
        assert_eq!(answer.models, [model()], "{}", item.as_str());
    }
    assert_eq!(
        pack.items
            .iter()
            .map(|answer| answer.id)
            .collect::<Vec<_>>(),
        [
            "S1", "S2", "S3", "S4", "S5", "S6", "I1", "I2", "I3", "I4", "I5", "I6"
        ]
    );
}

#[test]
fn p3_without_an_installation_only_the_root_items_are_answered() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::complete().write(&temp);

    let readiness = judge(&root, None);

    assert_eq!(readiness.installation, InstallationRead::NotRead);
    let pack = claim(&readiness);
    assert_eq!(
        in_state(pack, ItemState::Holds),
        [
            ReadinessItem::Admitted,
            ReadinessItem::Classified,
            ReadinessItem::Valued,
            ReadinessItem::Composed,
            ReadinessItem::EntryReachable,
            ReadinessItem::ModelsNamed,
        ],
        "{pack:?}"
    );
    let installation_items = [
        ReadinessItem::InstanceBound,
        ReadinessItem::PackageRevision,
        ReadinessItem::RequiresKnown,
        ReadinessItem::PairUnsuspended,
        ReadinessItem::ModelsInstalled,
    ];
    assert_eq!(in_state(pack, ItemState::NotJudged), installation_items);
    for item in installation_items {
        not_judged(pack, item, ReadinessUnjudged::InstallationNotRead);
    }
    assert_eq!(
        in_state(pack, ItemState::Information),
        [ReadinessItem::UnrecordedScreenCode]
    );
    assert!(in_state(pack, ItemState::Fails).is_empty(), "{pack:?}");
    assert_eq!(pack.readiness, PackState::NotJudged);
    assert_eq!(readiness.dispatchable, 0);
}

#[test]
fn p3_s1_a_pack_the_loader_refuses_fails_admitted_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let mut fixture = Fixture::complete();
    fixture.task = None;
    let root = fixture.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::Admitted,
        ReadinessProblem::AdmissionRefused,
    );
    assert!(
        cause_is(
            &outcome,
            actingcommand_contract::RuntimeErrorCode::PackageInvalid
        ),
        "{outcome:?}"
    );
    let pack = claim(&readiness);
    for item in [
        ReadinessItem::EntryReachable,
        ReadinessItem::ModelsNamed,
        ReadinessItem::ModelsInstalled,
    ] {
        not_judged(pack, item, ReadinessUnjudged::NotAdmitted);
    }
}

#[test]
fn p3_s2_an_unclassified_or_held_pack_fails_classified_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let mut unruled = Fixture::complete();
    unruled.rules = Vec::new();
    let root = unruled.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::Classified,
        ReadinessProblem::Unclassified,
    );
    assert!(
        cause_is(&outcome, SharedCode::StandardPackPackageUnclassified),
        "{outcome:?}"
    );
    not_judged(
        claim(&readiness),
        ReadinessItem::Valued,
        ReadinessUnjudged::NoRule,
    );

    let temp = TempDir::new().expect("temporary directory");
    let mut held = Fixture::complete();
    held.held = vec![json!({"package_id": CLAIM, "reason": "the fixture holds it"})];
    let root = held.write(&temp);

    let readiness = judge(&root, Some(installation()));

    fails_alone(
        &readiness,
        ReadinessItem::Classified,
        ReadinessProblem::Held,
    );
    not_judged(
        claim(&readiness),
        ReadinessItem::Valued,
        ReadinessUnjudged::NoRule,
    );
}

#[test]
fn p3_s3_a_pending_or_undeclared_value_fails_valued_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let mut pending = Fixture::complete();
    pending.rules = vec![rule(RULE, CLAIM, vec![produce(json!("pending"))])];
    let root = pending.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::Valued,
        ReadinessProblem::ValuePending,
    );
    assert_eq!(outcome.value("task_id"), Some(&json!(RULE)), "{outcome:?}");

    let temp = TempDir::new().expect("temporary directory");
    let mut undeclared = Fixture::complete();
    undeclared.rules = vec![rule(RULE, CLAIM, Vec::new())];
    let root = undeclared.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::Valued,
        ReadinessProblem::ValueUndeclared,
    );
    assert_eq!(outcome.value("task_id"), Some(&json!(RULE)), "{outcome:?}");
    assert!(
        cause_is(&outcome, SharedCode::StandardPackBenefitUndeclared),
        "{outcome:?}"
    );
}

#[test]
fn p3_s4_a_refused_schedule_fails_composed_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let mut refused = Fixture::complete();
    refused
        .rules
        .push(rule("neutral.test.absent.rule", ABSENT, Vec::new()));
    let root = refused.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::Composed,
        ReadinessProblem::CompositionRefused,
    );
    assert!(
        cause_is(&outcome, SharedCode::StandardPackRootInvalid),
        "{outcome:?}"
    );
    let pack = claim(&readiness);
    for item in [
        ReadinessItem::Classified,
        ReadinessItem::Valued,
        ReadinessItem::RequiresKnown,
    ] {
        not_judged(pack, item, ReadinessUnjudged::CompositionRefused);
    }
}

#[test]
fn p3_s5_an_unregistered_prerequisite_fails_entry_reachable_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let mut unregistered = Fixture::complete();
    unregistered.control["prerequisite_package_id"] = json!(ABSENT);
    let root = unregistered.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::EntryReachable,
        ReadinessProblem::PrerequisiteUnregistered,
    );
    assert_eq!(outcome.value("server"), Some(&json!(SERVER)), "{outcome:?}");

    // A prerequisite package the root holds reaches the entry.
    let temp = TempDir::new().expect("temporary directory");
    let mut registered = Fixture::complete();
    registered.control["prerequisite_package_id"] = json!(HOME);
    let root = registered.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let pack = claim(&readiness);
    assert_eq!(pack.readiness, PackState::Dispatchable, "{pack:?}");
    assert_eq!(
        pack.item(ReadinessItem::EntryReachable)
            .and_then(|answer| answer.basis),
        Some(ReadinessBasis::Prerequisite)
    );
}

#[test]
fn p3_i1_an_instance_of_another_game_or_server_fails_instance_bound_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::complete().write(&temp);
    let bound = |game: &str, server: Option<&str>| {
        let mut reading = installation();
        reading.bound = Some(vec![ReadinessBinding {
            instance_id: INSTANCE.to_owned(),
            game: game.to_owned(),
            server: server.map(str::to_owned),
        }]);
        reading
    };

    let readiness = judge(&root, Some(bound("other-game", Some(SERVER))));
    let outcome = fails_alone(
        &readiness,
        ReadinessItem::InstanceBound,
        ReadinessProblem::InstanceUnbound,
    );
    assert_eq!(outcome.value("game"), Some(&json!(GAME)), "{outcome:?}");

    let readiness = judge(&root, Some(bound(GAME, Some("other-server"))));
    let outcome = fails_alone(
        &readiness,
        ReadinessItem::InstanceBound,
        ReadinessProblem::ServerUnbound,
    );
    assert_eq!(outcome.value("server"), Some(&json!(SERVER)), "{outcome:?}");

    // A read that tells the game but not the server judges nothing.
    let readiness = judge(&root, Some(bound(GAME, None)));
    let pack = claim(&readiness);
    assert!(in_state(pack, ItemState::Fails).is_empty(), "{pack:?}");
    not_judged(
        pack,
        ReadinessItem::InstanceBound,
        ReadinessUnjudged::ServerNotTold,
    );
    assert_eq!(pack.readiness, PackState::NotJudged);
}

#[test]
fn p3_i2_a_revision_outside_the_installation_fails_package_revision_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::complete().write(&temp);
    let mut reading = installation();
    reading.package = Some([3, 11]);

    let readiness = judge(&root, Some(reading));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::PackageRevision,
        ReadinessProblem::RevisionOutside,
    );
    assert_eq!(
        (
            outcome.value("required_revision"),
            outcome.value("range_min"),
            outcome.value("range_max"),
        ),
        (Some(&json!(12)), Some(&json!(3)), Some(&json!(11))),
        "{outcome:?}"
    );
}

#[test]
fn p3_i4_a_requires_word_the_build_does_not_name_fails_requires_known_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::complete().write(&temp);
    let mut reading = installation();
    reading.subsystems = Some(BTreeSet::new());

    let readiness = judge(&root, Some(reading));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::RequiresKnown,
        ReadinessProblem::RequiresUnknown,
    );
    assert!(
        cause_is(&outcome, SharedCode::StandardPackRootInvalid),
        "{outcome:?}"
    );
}

#[test]
fn p3_i5_a_suspended_pair_fails_pair_unsuspended_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::complete().write(&temp);
    let mut reading = installation();
    reading.suspended = Some(vec![ReadinessSuspension {
        package_id: CLAIM.to_owned(),
        task_id: Some(RULE.to_owned()),
        instance_id: Some(INSTANCE.to_owned()),
    }]);

    let readiness = judge(&root, Some(reading));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::PairUnsuspended,
        ReadinessProblem::PairSuspended,
    );
    assert_eq!(
        (outcome.value("task_id"), outcome.value("instance_id")),
        (Some(&json!(RULE)), Some(&json!(INSTANCE))),
        "{outcome:?}"
    );
}

#[test]
fn p3_i6_a_model_not_installed_fails_models_installed_alone() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::complete().write(&temp);
    let mut reading = installation();
    reading.models = Some(Vec::new());

    let readiness = judge(&root, Some(reading));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::ModelsInstalled,
        ReadinessProblem::ModelMissing,
    );
    assert_eq!(
        outcome.value("model_name"),
        Some(&json!(MODEL.0)),
        "{outcome:?}"
    );
}
