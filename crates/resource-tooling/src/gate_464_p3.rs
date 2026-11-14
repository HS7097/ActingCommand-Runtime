// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #464 P3 (register §1.375; contracts/standard-pack-schedule.md, "Readiness"; the design
// rd-1114/464-pack-build-design.md F.1 and G, row P3, with the coordinator's rulings 3 and 7;
// coordinator work orders rd-1114/brief-464-p3-ready.md and rd-1114/brief-1718-fix1.md):
// `judge_standard_pack_readiness`, the one function that judges the thirteen readiness items of
// every pack of a game root.
// - `p3_a_complete_pack_answers_dispatchable`: on a made-up installed game root whose one business
//   pack has a rule with a complete value, the return-home package of its server and every part of
//   an installation read, the pack answers `dispatchable`: S1 to S7 and I1, I2, I4 to I6 hold, I3 is
//   an information line (`package_readiness_unjudged`, `no_revision_tells`), S2 by `scheduled`, S3
//   by `complete`, S6 and I6 name the pack's one model;
// - `p3_without_an_installation_only_the_root_items_are_answered`: the same root without an
//   installation answers `installation: not_read`, the seven root items hold, the five installation
//   items are `not_judged` with `installation_not_read`, I3 is information, the pack
//   `ready_to_place` (fix round 1: was `not_judged`);
// - one test per item that can fail, each changing one thing of the complete fixture, and holding
//   that exactly that item fails (`package_dispatch_waiting` with the item, its problem and its
//   values, the other check's outcome as its cause where one exists) and the pack `waiting`:
//   S1 (a task file that is no JSON; S5, S6 and I6 then `not_judged`, `not_admitted`, and S7
//   `check_stopped`), S2 (no rule: `unclassified`; a held package: `held`), S3 (`value_pending`;
//   `value_undeclared`), S4 (a rule of a package the root does not hold: `composition_refused`; S2,
//   S3 and I4 then `not_judged`), S5 (a prerequisite package the root does not hold, caused by the
//   kernel's `contained_task_prerequisite_unbound`; and one it holds, which holds), I1
//   (`instance_unbound`; `server_unbound`; a binding without its server `not_judged`, the pack
//   `ready_to_place`), I2 (`revision_outside`), I4 (`requires_unknown`), I5 (`pair_suspended`), I6
//   (`model_missing`). S6 cannot fail alone: the loader refuses a target that names no model, which
//   is S1's.
// - Fix round 1 (the independent review and the coordinator's rulings): a `linear_steps` business
//   pack whose recorded entry page the page list does not name reaches it through its server's
//   return-home package (S5 `return_home`, the pack dispatchable, the check's
//   `package_bundle_entry_page_unlisted` among the answer's warnings) and without one fails
//   `entry_unreachable` alone; navigation data that lists its entry is S5 `navigation`, and an
//   application entry S5 `application_entry`; the kernel's prerequisite chain refuses a
//   prerequisite of another server (`contained_task_prerequisite_incompatible`), a cycle
//   (`contained_task_prerequisite_cycle`) and four declared layers
//   (`contained_task_prerequisite_depth_exceeded`), each S5 `prerequisite_refused` alone with the
//   kernel's code as its cause and the refused layer as `blocking_package_id`; a root
//   `standard-pack check` refuses (a loading copy without `loading.json`) has no pack complete: the
//   pack it names fails S7 `check_refused`, the other `root_refused` naming it; the four answers
//   (`dispatchable`, `ready_to_place`, `waiting`, `not_scheduled` for the return-home package, with
//   its role); a follower pack is dispatchable behind a dispatchable head and waits
//   (`head_waiting`, naming the head's pack) behind a waiting one.
// - Fix round 2 (the second review, the coordinator's work order): a page-graph business pack
//   whose entry the Runtime checks (an entry page set) keeps its home entry, which takes the
//   server's return-home package through no chain; a return-home package of another resolution,
//   and one that declares a prerequisite package, each fail S5 `return_home_refused` alone, caused
//   by the kernel's `contained_task_prerequisite_incompatible` and naming the return-home package.
// Made-up packs only; no device, no Runtime, no clock read, no sleep.

use crate::codes::{
    ReadinessItem, ReadinessProblem, ReadinessRole, ReadinessState, ReadinessUnjudged,
    ResourceToolingCode,
};
use crate::{
    InstallationRead, ItemState, PackReadiness, ReadinessBasis, ReadinessBinding,
    ReadinessInstallation, ReadinessModel, ReadinessSuspension, StandardPackReadiness,
    StandardPackReadyRequest, judge_standard_pack_readiness,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_execution_kernel::ExecutionKernelCode;
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
/// Fix round 1: another server of the game, and a pack of it.
const OTHER_SERVER: &str = "other";
const OTHER: &str = "neutral.other.prereq";
/// Fix round 1: a pack whose prerequisite is the business pack.
const LOOP: &str = "neutral.test.loop";
/// Fix round 1: four packs, each the prerequisite of the one before it.
const DEPTH: [&str; 4] = [
    "neutral.test.depth1",
    "neutral.test.depth2",
    "neutral.test.depth3",
    "neutral.test.depth4",
];
/// Fix round 1: the feature package of `pages.json`.
const FEATURES: &str = "neutral.test.features";
/// Fix round 1: a pack whose one rule rides with the business pack's rule.
const FOLLOWER: &str = "neutral.test.follower";
const FOLLOW_RULE: &str = "neutral.test.follower.rule";
/// Fix round 2: a pack the return-home package declares its prerequisite package.
const BEFORE_HOME: &str = "neutral.test.before_home";

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

/// The colour of the first pixel that tells `page`.
fn colour(page: &str) -> [u8; 3] {
    match page {
        "home" => [0, 0, 255],
        "page_b" => [255, 0, 0],
        "shop" => [0, 255, 0],
        "stage" => [255, 255, 0],
        "done" => [255, 255, 255],
        other => panic!("no page {other}"),
    }
}

/// The control of the page-graph pack `package_id`, as `gate_453_s8` writes it.
fn control(package_id: &str) -> Value {
    control_on(package_id, SERVER)
}

/// [`control`] of a pack of `server`.
fn control_on(package_id: &str, server: &str) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": server,
        "resolution": {"width": 64, "height": 24},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    })
}

/// [`control`] of a pack that declares `prerequisite` its prerequisite package.
fn control_after(package_id: &str, prerequisite: &str) -> Value {
    let mut control = control(package_id);
    control["prerequisite_package_id"] = json!(prerequisite);
    control
}

/// A 64x24 page graph whose page `home` is told by a blue first pixel and `page_b` by a red one,
/// one trusted click from `home` to `page_b`; with `model`, `page_b` also requires an OCR target
/// of that model (as `gate_453_s8` writes it).
fn task(model: Option<(&str, &str)>) -> Value {
    task_on(SERVER, model)
}

/// [`task`] of a pack of `server`.
fn task_on(server: &str, model: Option<(&str, &str)>) -> Value {
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [server],
        "locale": "en-US",
        "goal": "P3 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", colour("home")), probe("page_b", colour("page_b"))],
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

/// The pages `pages` of a 64x24 task: their colour probes and page rules.
fn pages_of(pages: &[&str]) -> (Value, Value) {
    let probes = pages
        .iter()
        .map(|page| probe(page, colour(page)))
        .collect::<Vec<_>>();
    let rules = pages
        .iter()
        .map(|page| {
            (
                (*page).to_owned(),
                json!({"required": [format!("{page}_colour")]}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    (json!(probes), Value::Object(rules))
}

/// Fix round 1: the control of the one-step `linear_steps` pack `package_id`.
fn linear_control(package_id: &str) -> Value {
    let mut control = control(package_id);
    control["execution_mode"] = json!("linear_steps");
    control["max_steps"] = json!(1);
    control
}

/// Fix round 1: a one-step `linear_steps` task from `from` to `to`: a trusted click, or, from
/// `any` (an application entry), a restart of the application to `home`.
fn linear_task(from: &str, to: &str) -> Value {
    let pages = [from, to]
        .into_iter()
        .filter(|page| *page != "any")
        .collect::<Vec<_>>();
    let (probes, rules) = pages_of(&pages);
    let operation = if from == "any" {
        json!({"id": "start", "from": from, "to": to, "application": {"action": "restart"}})
    } else {
        json!({
            "id": "go", "from": from, "to": to,
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        })
    };
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "P3 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": probes,
        "page_rules": rules,
        "entry_page": from,
        "target_page": to,
        "max_steps": 1,
        "operations": [operation]
    })
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

/// Fix round 1: a pack beside the business pack and the return-home package.
struct Extra {
    package_id: &'static str,
    server: &'static str,
    control: Value,
    task: Value,
}

/// A page-graph pack of the server without a model.
fn extra(package_id: &'static str, control: Value) -> Extra {
    Extra {
        package_id,
        server: SERVER,
        control,
        task: task_without_model(),
    }
}

/// Fix round 1: the feature package of `pages.json`, a `recognize_only` pack whose pages are the
/// page list's, `home` and `shop` (as `gate_373_navt` writes it).
fn features_pack() -> Extra {
    let mut control = control(FEATURES);
    control["execution_mode"] = json!("recognize_only");
    let (probes, rules) = pages_of(&["home", "shop"]);
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "P3 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": probes,
        "page_rules": rules,
        "entry_page": "home",
        "target_page": "home",
        "operations": [{
            "id": "see_shop", "from": "shop", "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    Extra {
        package_id: FEATURES,
        server: SERVER,
        control,
        task,
    }
}

/// Fix round 1: `pages.json` listing `home` and `shop`, recognised by [`features_pack`].
fn pages_file() -> (&'static str, Value) {
    (
        "pages.json",
        json!({
            "schema_version": "actingcommand.pages.v1",
            "game": GAME,
            "features": [FEATURES],
            "pages": [{"id": "home", "kind": "page"}, {"id": "shop", "kind": "page"}]
        }),
    )
}

/// Fix round 1: `navigation.json` with no edge and the return-home package as its recovery chain.
fn navigation_file() -> (&'static str, Value) {
    (
        "navigation.json",
        json!({
            "schema_version": "actingcommand.navigation.v1",
            "game": GAME,
            "edges": [],
            "recovery_chain": [HOME]
        }),
    )
}

/// What one test writes: the complete fixture, of which a test changes one thing.
struct Fixture {
    /// The business pack's control.
    control: Value,
    /// Its task; `None` writes a task file that is no JSON.
    task: Option<Value>,
    rules: Vec<Value>,
    held: Vec<Value>,
    /// Fix round 1: the packs beside the business pack and the return-home package.
    extra: Vec<Extra>,
    /// Fix round 1: whether `bundle.json` names the return-home package for `return_home`.
    return_home: bool,
    /// Fix round 1: root files beside `bundle.json` and `schedule.json`, by name.
    root_files: Vec<(&'static str, Value)>,
    /// Fix round 1: a loading copy (`resources/loading`) in the business pack, which
    /// `standard-pack check` refuses in a root without `loading.json`.
    loading_copy: bool,
    /// Fix round 2: the return-home package's control and task.
    home_control: Value,
    home_task: Value,
}

impl Fixture {
    fn complete() -> Self {
        Self {
            control: control(CLAIM),
            task: Some(task(Some(MODEL))),
            rules: vec![rule(RULE, CLAIM, vec![produce(json!(1))])],
            held: Vec::new(),
            extra: Vec::new(),
            return_home: true,
            root_files: Vec::new(),
            loading_copy: false,
            home_control: control(HOME),
            home_task: task_without_model(),
        }
    }

    /// Fix round 2: the complete fixture whose page-graph business pack declares its entry as
    /// the entry page set `["home"]`, an entry the Runtime checks (an entry page set is always
    /// checked), so that its home entry takes the server's return-home package.
    fn home_entered() -> Self {
        let mut entered = task(Some(MODEL));
        entered["entry_page"] = json!(["home"]);
        Self {
            task: Some(entered),
            ..Self::complete()
        }
    }

    /// Fix round 1: the complete fixture with a one-step `linear_steps` business pack from `from`
    /// to `to`.
    fn linear(from: &str, to: &str) -> Self {
        Self {
            control: linear_control(CLAIM),
            task: Some(linear_task(from, to)),
            ..Self::complete()
        }
    }

    /// The installed game root `<temp>/packages/neutral`: the business pack, the server's
    /// return-home package (named for `return_home` unless the fixture says otherwise), the extra
    /// packs, `bundle.json`, `schedule.json` and the other root files.
    fn write(&self, temp: &TempDir) -> PathBuf {
        let root = temp.path().join("packages").join(GAME);
        let task = match &self.task {
            Some(task) => serde_json::to_vec_pretty(task).expect("task JSON"),
            None => b"{".to_vec(),
        };
        write_pack(&root.join(CLAIM), &self.control, &task);
        if self.loading_copy {
            fs::create_dir_all(root.join(CLAIM).join("resources").join("loading"))
                .expect("loading copy");
        }
        let home = serde_json::to_vec_pretty(&self.home_task).expect("task JSON");
        write_pack(&root.join(HOME), &self.home_control, &home);
        for extra in &self.extra {
            let task = serde_json::to_vec_pretty(&extra.task).expect("task JSON");
            write_pack(&root.join(extra.package_id), &extra.control, &task);
        }
        let listed = [(CLAIM, SERVER), (HOME, SERVER)]
            .into_iter()
            .chain(
                self.extra
                    .iter()
                    .map(|extra| (extra.package_id, extra.server)),
            )
            .map(|(package_id, server)| {
                json!({
                    "package_id": package_id, "server": server, "entry_task_id": "task",
                    "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
                })
            })
            .collect::<Vec<_>>();
        let mut bundle = json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": listed
        });
        if self.return_home {
            bundle["maintenance"] =
                json!([{"package_id": HOME, "server": SERVER, "uses": ["return_home"]}]);
        }
        write_json(&root.join("bundle.json"), &bundle);
        for (name, value) in &self.root_files {
            write_json(&root.join(name), value);
        }
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

/// [`installation`] with the business pack's pair suspended.
fn claim_suspended() -> ReadinessInstallation {
    let mut reading = installation();
    reading.suspended = Some(vec![ReadinessSuspension {
        package_id: CLAIM.to_owned(),
        task_id: Some(RULE.to_owned()),
        instance_id: Some(INSTANCE.to_owned()),
    }]);
    reading
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

/// The answer of the pack `package_id`.
fn pack<'a>(readiness: &'a StandardPackReadiness, package_id: &str) -> &'a PackReadiness {
    readiness
        .packs
        .iter()
        .find(|pack| pack.package_id == package_id)
        .unwrap_or_else(|| panic!("{package_id} is answered: {readiness:?}"))
}

/// The business pack's answer.
fn claim(readiness: &StandardPackReadiness) -> &PackReadiness {
    pack(readiness, CLAIM)
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

/// The basis `item` of `pack` holds by.
fn basis(pack: &PackReadiness, item: ReadinessItem) -> Option<ReadinessBasis> {
    pack.item(item).and_then(|answer| answer.basis)
}

/// Holds that exactly `item` of `pack` fails, with `problem`, and that the pack waits; the
/// failing item's outcome.
fn alone(pack: &PackReadiness, item: ReadinessItem, problem: ReadinessProblem) -> Outcome {
    assert_eq!(in_state(pack, ItemState::Fails), [item], "{pack:?}");
    assert_eq!(pack.readiness, ReadinessState::Waiting, "{pack:?}");
    let outcome = outcome_of(pack, item).clone();
    assert!(
        outcome.is(ResourceToolingCode::PackageDispatchWaiting),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("package_id"),
        Some(&json!(pack.package_id)),
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

/// [`alone`] of the business pack.
fn fails_alone(
    readiness: &StandardPackReadiness,
    item: ReadinessItem,
    problem: ReadinessProblem,
) -> Outcome {
    alone(claim(readiness), item, problem)
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

/// The first cause of `outcome`.
fn first_cause(outcome: &Outcome) -> &Outcome {
    outcome
        .causes()
        .first()
        .map(|link| link.outcome())
        .unwrap_or_else(|| panic!("a cause: {outcome:?}"))
}

/// The four counts of `readiness`: dispatchable, ready to place, waiting, not scheduled.
fn counts(readiness: &StandardPackReadiness) -> [usize; 4] {
    [
        readiness.dispatchable,
        readiness.ready_to_place,
        readiness.waiting,
        readiness.not_scheduled,
    ]
}

/// The seven items of the game root, in order.
const ROOT_ITEMS: [ReadinessItem; 7] = [
    ReadinessItem::Admitted,
    ReadinessItem::Classified,
    ReadinessItem::Valued,
    ReadinessItem::Composed,
    ReadinessItem::EntryReachable,
    ReadinessItem::ModelsNamed,
    ReadinessItem::RootChecked,
];

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
    assert_eq!(pack.readiness, ReadinessState::Dispatchable, "{pack:?}");
    assert_eq!(pack.items.len(), 13, "{pack:?}");
    assert_eq!(
        in_state(pack, ItemState::Holds),
        [
            ReadinessItem::Admitted,
            ReadinessItem::Classified,
            ReadinessItem::Valued,
            ReadinessItem::Composed,
            ReadinessItem::EntryReachable,
            ReadinessItem::ModelsNamed,
            ReadinessItem::RootChecked,
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
    assert_eq!(
        basis(pack, ReadinessItem::Classified),
        Some(ReadinessBasis::Scheduled)
    );
    assert_eq!(
        basis(pack, ReadinessItem::Valued),
        Some(ReadinessBasis::Complete)
    );
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
            "S1", "S2", "S3", "S4", "S5", "S6", "S7", "I1", "I2", "I3", "I4", "I5", "I6"
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
    assert_eq!(in_state(pack, ItemState::Holds), ROOT_ITEMS, "{pack:?}");
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
    // Fix round 1: complete as far as the root tells, never dispatchable.
    assert_eq!(pack.readiness, ReadinessState::ReadyToPlace);
    assert_eq!(readiness.dispatchable, 0);
    assert_eq!(readiness.ready_to_place, 1);
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
    // Fix round 1: `standard-pack check` stopped at this admission, answered under S1.
    not_judged(
        pack,
        ReadinessItem::RootChecked,
        ReadinessUnjudged::CheckStopped,
    );
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
    // Fix round 1: the kernel's chain refused the layer.
    assert!(
        cause_is(
            &outcome,
            ExecutionKernelCode::ContainedTaskPrerequisiteUnbound
        ),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("blocking_package_id"),
        Some(&json!(ABSENT)),
        "{outcome:?}"
    );

    // A prerequisite package the root holds reaches the entry.
    let temp = TempDir::new().expect("temporary directory");
    let mut registered = Fixture::complete();
    registered.control["prerequisite_package_id"] = json!(HOME);
    let root = registered.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let pack = claim(&readiness);
    assert_eq!(pack.readiness, ReadinessState::Dispatchable, "{pack:?}");
    assert_eq!(
        basis(pack, ReadinessItem::EntryReachable),
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
    // Fix round 1: an installation item not judged never counts as holding.
    assert_eq!(pack.readiness, ReadinessState::ReadyToPlace);
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

    let readiness = judge(&root, Some(claim_suspended()));

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

// ---- Fix round 1.

// Ruling 7: a recorded `linear_steps` pack whose entry page `pages.json` does not list is accepted
// with its warning, and reaches its entry through its server's return-home package; without one
// S5 fails `entry_unreachable` alone.
#[test]
fn p3_fr1_s5_a_linear_pack_reaches_an_unlisted_entry_only_by_return_home() {
    let fixture = || {
        let mut fixture = Fixture::linear("stage", "done");
        fixture.extra.push(features_pack());
        fixture.root_files.push(pages_file());
        fixture
    };
    let temp = TempDir::new().expect("temporary directory");
    let root = fixture().write(&temp);

    let readiness = judge(&root, Some(installation()));

    let pack = claim(&readiness);
    assert_eq!(pack.readiness, ReadinessState::Dispatchable, "{pack:?}");
    assert_eq!(
        basis(pack, ReadinessItem::EntryReachable),
        Some(ReadinessBasis::ReturnHome)
    );
    let unlisted = readiness
        .warnings
        .iter()
        .filter(|warning| warning.is(ResourceToolingCode::PackageBundleEntryPageUnlisted))
        .collect::<Vec<_>>();
    assert_eq!(unlisted.len(), 1, "{:?}", readiness.warnings);
    assert_eq!(
        unlisted[0].value("package_id"),
        Some(&json!(CLAIM)),
        "{unlisted:?}"
    );

    let temp = TempDir::new().expect("temporary directory");
    let mut without = fixture();
    without.return_home = false;
    let root = without.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let outcome = fails_alone(
        &readiness,
        ReadinessItem::EntryReachable,
        ReadinessProblem::EntryUnreachable,
    );
    assert_eq!(outcome.value("server"), Some(&json!(SERVER)), "{outcome:?}");
}

// A `linear_steps` pack whose entry the page list names, in a root with navigation data, is
// reached by the navigation (S5 `navigation`).
#[test]
fn p3_fr1_s5_a_linear_pack_whose_entry_is_listed_is_reached_by_navigation() {
    let temp = TempDir::new().expect("temporary directory");
    let mut fixture = Fixture::linear("home", "shop");
    fixture.extra.push(features_pack());
    fixture.root_files.push(pages_file());
    fixture.root_files.push(navigation_file());
    let root = fixture.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let pack = claim(&readiness);
    assert_eq!(pack.readiness, ReadinessState::Dispatchable, "{pack:?}");
    assert_eq!(
        basis(pack, ReadinessItem::EntryReachable),
        Some(ReadinessBasis::Navigation)
    );
}

// A `linear_steps` pack whose first step starts the application from any screen needs no page
// entry (S5 `application_entry`).
#[test]
fn p3_fr1_s5_a_linear_application_entry_holds() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::linear("any", "home").write(&temp);

    let readiness = judge(&root, Some(installation()));

    let pack = claim(&readiness);
    assert_eq!(pack.readiness, ReadinessState::Dispatchable, "{pack:?}");
    assert_eq!(
        basis(pack, ReadinessItem::EntryReachable),
        Some(ReadinessBasis::ApplicationEntry)
    );
}

// The kernel's prerequisite chain, as the Runtime runs it before a run: a prerequisite of another
// server, a cycle and four declared layers each fail S5 alone, `prerequisite_refused`, with the
// kernel's code as the cause and the refused layer named.
#[test]
fn p3_fr1_s5_the_kernel_chain_refuses_another_server_a_cycle_and_depth_four() {
    let mut other = Fixture::complete();
    other.control = control_after(CLAIM, OTHER);
    other.extra.push(Extra {
        package_id: OTHER,
        server: OTHER_SERVER,
        control: control_on(OTHER, OTHER_SERVER),
        task: task_on(OTHER_SERVER, None),
    });
    let mut cycle = Fixture::complete();
    cycle.control = control_after(CLAIM, LOOP);
    cycle.extra.push(extra(LOOP, control_after(LOOP, CLAIM)));
    let mut deep = Fixture::complete();
    deep.control = control_after(CLAIM, DEPTH[0]);
    for (index, package_id) in DEPTH.into_iter().enumerate() {
        let layer = match DEPTH.get(index + 1) {
            Some(next) => control_after(package_id, next),
            None => control(package_id),
        };
        deep.extra.push(extra(package_id, layer));
    }
    let cases: [(&str, Fixture, actingcommand_contract::outcome::Code, &str); 3] = [
        (
            "another server",
            other,
            SharedCode::ContainedTaskPrerequisiteIncompatible.into(),
            OTHER,
        ),
        (
            "a cycle",
            cycle,
            ExecutionKernelCode::ContainedTaskPrerequisiteCycle.into(),
            CLAIM,
        ),
        (
            "four declared layers",
            deep,
            ExecutionKernelCode::ContainedTaskPrerequisiteDepthExceeded.into(),
            DEPTH[3],
        ),
    ];
    for (case, fixture, code, layer) in cases {
        let temp = TempDir::new().expect("temporary directory");
        let root = fixture.write(&temp);

        let readiness = judge(&root, Some(installation()));

        let outcome = fails_alone(
            &readiness,
            ReadinessItem::EntryReachable,
            ReadinessProblem::PrerequisiteRefused,
        );
        assert!(cause_is(&outcome, code), "{case}: {outcome:?}");
        assert_eq!(
            outcome.value("blocking_package_id"),
            Some(&json!(layer)),
            "{case}: {outcome:?}"
        );
    }
}

// The coordinator's ruling: the root check runs first. A root `standard-pack check` refuses (here
// a loading copy in the business pack, without `loading.json`) has no pack complete: the pack the
// refusal names fails S7 `check_refused` with it as the cause, and every other pack S7
// `root_refused` naming that pack.
#[test]
fn p3_fr1_s7_a_root_the_check_refuses_has_no_pack_complete() {
    let temp = TempDir::new().expect("temporary directory");
    let mut fixture = Fixture::complete();
    fixture.loading_copy = true;
    let root = fixture.write(&temp);

    for installation in [None, Some(installation())] {
        let readiness = judge(&root, installation);

        assert!(readiness.warnings.is_empty(), "{:?}", readiness.warnings);
        assert_eq!(counts(&readiness), [0, 0, 2, 0], "{readiness:?}");
        let claimed = claim(&readiness);
        assert_eq!(
            claimed
                .item(ReadinessItem::RootChecked)
                .map(|answer| answer.state),
            Some(ItemState::Fails),
            "{claimed:?}"
        );
        let refused = outcome_of(claimed, ReadinessItem::RootChecked);
        assert_eq!(
            refused.value("readiness_problem"),
            Some(&json!(ReadinessProblem::CheckRefused.as_str())),
            "{refused:?}"
        );
        let cause = first_cause(refused);
        assert!(cause.is(SharedCode::StandardPackRootInvalid), "{cause:?}");
        assert_eq!(cause.value("package_id"), Some(&json!(CLAIM)), "{cause:?}");
        let home = pack(&readiness, HOME);
        let elsewhere = outcome_of(home, ReadinessItem::RootChecked);
        assert_eq!(
            home.item(ReadinessItem::RootChecked)
                .map(|answer| answer.state),
            Some(ItemState::Fails),
            "{home:?}"
        );
        assert_eq!(
            elsewhere.value("readiness_problem"),
            Some(&json!(ReadinessProblem::RootRefused.as_str())),
            "{elsewhere:?}"
        );
        assert_eq!(
            elsewhere.value("blocking_package_id"),
            Some(&json!(CLAIM)),
            "{elsewhere:?}"
        );
    }
}

// The four answers: with every part of an installation the business pack is `dispatchable`, the
// return-home package `not_scheduled` with its role; without an installation the business pack is
// `ready_to_place`; with its pair suspended it is `waiting`.
#[test]
fn p3_fr1_each_of_the_four_answers() {
    let temp = TempDir::new().expect("temporary directory");
    let root = Fixture::complete().write(&temp);

    let readiness = judge(&root, Some(installation()));
    assert_eq!(claim(&readiness).readiness, ReadinessState::Dispatchable);
    let home = pack(&readiness, HOME);
    assert_eq!(home.readiness, ReadinessState::NotScheduled, "{home:?}");
    assert_eq!(home.roles, [ReadinessRole::ReturnHome], "{home:?}");
    assert!(claim(&readiness).roles.is_empty());
    assert_eq!(counts(&readiness), [1, 0, 0, 1]);

    let readiness = judge(&root, None);
    assert_eq!(claim(&readiness).readiness, ReadinessState::ReadyToPlace);
    assert_eq!(
        pack(&readiness, HOME).readiness,
        ReadinessState::NotScheduled
    );
    assert_eq!(counts(&readiness), [0, 1, 0, 1]);

    let readiness = judge(&root, Some(claim_suspended()));
    assert_eq!(claim(&readiness).readiness, ReadinessState::Waiting);
    assert_eq!(counts(&readiness), [0, 0, 1, 1]);
}

// A follower is not ahead of its head: a pack whose one rule rides with the business pack's rule
// is dispatchable while the business pack is, and waits, S3 `head_waiting` naming the business
// pack, while the business pack waits.
#[test]
fn p3_fr1_s3_a_follower_waits_behind_a_waiting_head() {
    let temp = TempDir::new().expect("temporary directory");
    let mut fixture = Fixture::complete();
    fixture.extra.push(extra(FOLLOWER, control(FOLLOWER)));
    let mut follower = rule(FOLLOW_RULE, FOLLOWER, Vec::new());
    follower["trigger"] = json!({
        "kind": "dependency_completed",
        "task_id": RULE,
        "terminal_states": ["succeeded"],
        "since_own_run": true
    });
    fixture.rules.push(follower);
    let root = fixture.write(&temp);

    let readiness = judge(&root, Some(installation()));

    let behind = pack(&readiness, FOLLOWER);
    assert_eq!(behind.readiness, ReadinessState::Dispatchable, "{behind:?}");
    assert_eq!(
        basis(behind, ReadinessItem::Valued),
        Some(ReadinessBasis::Follower)
    );

    let readiness = judge(&root, Some(claim_suspended()));

    assert_eq!(claim(&readiness).readiness, ReadinessState::Waiting);
    let outcome = alone(
        pack(&readiness, FOLLOWER),
        ReadinessItem::Valued,
        ReadinessProblem::HeadWaiting,
    );
    assert_eq!(
        (
            outcome.value("task_id"),
            outcome.value("blocking_package_id")
        ),
        (Some(&json!(FOLLOW_RULE)), Some(&json!(CLAIM))),
        "{outcome:?}"
    );
}

// ---- Fix round 2.

/// Fix round 2: holds that the business pack fails S5 alone, `return_home_refused`, caused by the
/// kernel's `contained_task_prerequisite_incompatible` and naming the return-home package.
fn return_home_refused(readiness: &StandardPackReadiness) {
    let outcome = fails_alone(
        readiness,
        ReadinessItem::EntryReachable,
        ReadinessProblem::ReturnHomeRefused,
    );
    assert!(
        cause_is(&outcome, SharedCode::ContainedTaskPrerequisiteIncompatible),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("blocking_package_id"),
        Some(&json!(HOME)),
        "{outcome:?}"
    );
}

// The Runtime's home entry of a page-graph pack refuses a return-home package of another
// resolution (the kernel's `prerequisite_incompatibility_with` for a configured return-home
// package): S5 fails `return_home_refused` alone.
#[test]
fn p3_fr2_s5_a_return_home_package_of_another_resolution_is_refused() {
    let temp = TempDir::new().expect("temporary directory");
    let mut fixture = Fixture::home_entered();
    fixture.home_control["resolution"] = json!({"width": 128, "height": 48});
    fixture.home_task["coordinate_space"] = json!({"width": 128, "height": 48});
    let root = fixture.write(&temp);

    let readiness = judge(&root, Some(installation()));

    return_home_refused(&readiness);
}

// The Runtime's home entry of a page-graph pack refuses a return-home package that declares a
// prerequisite package of its own, which a chain layer may: S5 fails `return_home_refused` alone.
#[test]
fn p3_fr2_s5_a_return_home_package_that_declares_a_prerequisite_is_refused() {
    let temp = TempDir::new().expect("temporary directory");
    let mut fixture = Fixture::home_entered();
    fixture.home_control = control_after(HOME, BEFORE_HOME);
    fixture.extra.push(extra(BEFORE_HOME, control(BEFORE_HOME)));
    let root = fixture.write(&temp);

    let readiness = judge(&root, Some(installation()));

    return_home_refused(&readiness);
}
