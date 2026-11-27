// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-8, the Runtime's half (0.12.68; register §1.444; contracts/runtime-interface.md,
// "The variable values of a contained run"; contracts/package-variables.md, "Runs without a
// dispatch"; coordinator work order rd-1116/brief-463-s38-run-variables.md): a direct run's request
// may give values for variables its package declares (`ContainedTaskRequest.variables`); the host
// judges each before the run's admission with the policy crate's `resolve_variable`, the function
// a dispatch resolves a value by, and the variable table a dispatch reads, refuses one it does not
// take, and overlays the others as a dispatch's values are overlaid
// (`session.dispatch.parameter.<name>`); a variable not given reads its default.
//
// A neutral fixture, built as `gate_463_v2` builds its own: the host runs in composer mode over one
// game root on disk, a manual Runtime clock and one fake instance on a physical provider whose
// display is observed (a direct run needs it, Workflow #392 DA-A). The root's `schedule.json` holds
// one rule whose package goes from `home` to `result` in one tap and declares the variable `pick` (a
// `choice` of `item_a` and `item_b`, default `item_b`). One evaluation first, so that the
// workbench checks the root and judges its variables; no dispatch is admitted. Each direct run is
// sent on a connection of its own with the command line's envelope. Every expected value is the
// work order's.
// - `s38_host_a_given_value_is_what_the_package_reads`: `pick` given `item_a`: the run completes
//   after its one tap and its snapshots are overlaid with `item_a`.
// - `s38_host_a_value_the_variable_does_not_take_is_refused_before_admission`: `item_z`:
//   Denied `run_variable_value_refused` with the package, the name and `not_an_option`; no tap, no
//   run parameters read, no record of the run's admission (fix round 1: the ledger holds no
//   `task.package_admitted` more than before the run).
// - `s38_host_an_undeclared_name_is_refused`: a value under `other`: Denied
//   `run_variable_undeclared` with the package and the name; nothing ran.
// - `s38_host_without_values_the_run_reads_the_defaults`: no value: the run completes and reads
//   `item_b`.
// Fix round 1 (rd-1116/brief-463-s38-fix1.md; the review of #1789, its section (h)); each refusal
// as above: Denied, nothing pressed, no run read its parameters, no admission in the ledger:
// - `s38_fix1_a_variable_that_does_not_apply_is_refused_before_admission`: the root's
//   `goals.json` uses the name `pick` for a heading, so `pick` does not apply in game A's root
//   (`goal_name_conflict`, as the workbench judged it): `item_a` is refused
//   `run_variable_not_applying` with the package, the name and that word.
// - `s38_fix1_a_value_taken_only_in_part_is_refused`: the package also declares `order` (`ordered`,
//   the options `item_a` and `item_b`), which applies; `["item_a", "item_z"]`, of which the
//   variable would take `item_a` alone, is refused `run_variable_value_refused` with
//   `keys_left_out`, as the goal's edit refuses it.
// - `s38_fix1_seventeen_values_in_a_raw_request_are_refused_at_the_door`: a request frame whose
//   `variables` holds seventeen values, one more than a package can declare variables, written as
//   raw JSON past the contract's own constructor: Denied `invalid_contained_task_variable_count`
//   by the contract's check of the request at the door.
// Fix round of the 0.12.68 train (rd-1116/brief-train-01268-fix1.md, point 4; the review of the
// train, which found only refusals of an `ordered` value here):
// - `train_01268_fix1_an_allowed_order_of_a_generated_choice_is_what_the_package_reads`: the
//   package declares one `ordered` variable as stage two's `record stop` writes a choice
//   (`list_item`, the options `item_a` and `item_b`, one or two of them, default `["item_b"]`),
//   which applies; `["item_b", "item_a"]`, allowed and not in the options' order, is accepted: the
//   run completes after its one tap and its snapshots hold the two keys in that order.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use actingcommand_contract::{
    FactContent, FactScalar, FactValue, InstanceFactContext, InstanceFactSnapshot,
    MAX_PACKAGE_VARIABLES, ORDERED_VARIABLE_KEY_COLUMN, ORDERED_VARIABLE_RANK_COLUMN,
    VariableProblem,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;

const TASK: &str = "fixture.s38";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const PICK_KEY: &str = "session.dispatch.parameter.pick";
// The fix round of the 0.12.68 train: the record a run reads `list_item` from.
const LIST_ITEM_KEY: &str = "session.dispatch.parameter.list_item";

// ---- The game root.

// The variable the package declares.
fn variables() -> Value {
    json!([{"name": "pick", "type": "choice", "title": {"en": "Item"},
            "options": [{"value": "item_a", "title": {"en": "Item A"}},
                        {"value": "item_b", "title": {"en": "Item B"}}],
            "default": "item_b"}])
}

// Fix round 1: [`variables`] and `order`, an `ordered` variable of the options `item_a` and
// `item_b`.
fn variables_with_order() -> Value {
    let order = json!({"name": "order", "type": "ordered", "title": {"en": "Order"},
                       "options": [{"value": "item_a", "title": {"en": "Item A"}},
                                   {"value": "item_b", "title": {"en": "Item B"}}],
                       "min_count": 0, "max_count": 2, "default": ["item_b"]});
    let mut declared = variables();
    declared
        .as_array_mut()
        .expect("the declared variables are a list")
        .push(order);
    declared
}

// Places the game root below `packs`: its bundle, its `schedule.json` with the one rule, and the
// package, whose control declaration declares [`variables`].
fn place_game_root(packs: &Path) {
    place_game_root_declaring(packs, variables());
}

// Fix round 1: [`place_game_root`], the root's `goals.json` using the name `pick` for a heading.
fn place_with_goals_taking_pick(packs: &Path) {
    place_game_root(packs);
    write_json(
        &packs.join(GAME_A).join("goals.json"),
        &json!({"schema_version": "actingcommand.goals.v3", "game": GAME_A,
                "entries": [{"name": "pick", "type": "heading", "title": {"en": "Pick"}}]}),
    );
}

// Fix round 1: [`place_game_root`], the package declaring [`variables_with_order`].
fn place_with_order(packs: &Path) {
    place_game_root_declaring(packs, variables_with_order());
}

// The fix round of the 0.12.68 train: the one variable of a package stage two generates from a
// recording with a choice (`choice_variable_json`, as the train's test of the trial's plan writes
// it): `ordered`, its texts under the package's locale, one to two of its two options, the recorded
// option its default.
fn generated_choice() -> Value {
    let title = |text: &str| json!({"en-US": text});
    json!([{
        "name": "list_item",
        "type": "ordered",
        "title": title("list_item"),
        "options": [{"value": "item_a", "title": title("ALPHA")},
                    {"value": "item_b", "title": title("BRAVO")}],
        "min_count": 1,
        "max_count": 2,
        "default": ["item_b"]
    }])
}

// The fix round of the 0.12.68 train: [`place_game_root`], the package declaring
// [`generated_choice`] alone.
fn place_with_generated_choice(packs: &Path) {
    place_game_root_declaring(packs, generated_choice());
}

// The game root of [`place_game_root`], its package's control declaration declaring `declared`.
fn place_game_root_declaring(packs: &Path, declared: Value) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[TASK]),
    );
    let mut vars = rule(TASK, TASK, 100, Some(POOL_A));
    vars["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    vars["loop_budget"]["window_iteration_limit"] = json!(8);
    vars["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(8);
    }
    write_json(
        &game.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": [],
            "rules": [vars],
            "pools": [pool(POOL_A)],
            "activity": activity,
            "timeline": [],
            "held": []
        }),
    );
    let package = game.join(TASK);
    write_package(&package, TASK, None, None);
    let control_path = package.join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    control["variables"] = declared;
    write_json(&control_path, &control);
}

// The fixture's facts: the one instance bound to game A on server A, able to run the Runtime's
// package operation, and no recorded outcome.
fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances = vec![InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: SERVER_A.to_owned(),
        game_id: GAME_A.to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec![actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned()],
        preferred_task_ids: Vec::new(),
    }];
    facts
}

// ---- The fixture.

struct Scene {
    _state_root: TempDir,
    packs: TempDir,
    device: Arc<FakeState>,
    host: RuntimeHost,
}

impl Scene {
    // The host over the game root, after one evaluation: the workbench judged the root and `pick`
    // applies in game A.
    fn new() -> Self {
        let scene = Self::with(place_game_root);
        assert_eq!(scene.root_word("pick"), None, "pick applies");
        scene
    }

    // Fix round 1: the host over the game root `place` places below its standard-pack root, after
    // one evaluation: the workbench judged the root.
    fn with(place: impl FnOnce(&Path)) -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place(packs.path());
        let device = Arc::new(FakeState::default());
        // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
        device.physical_task_geometry.store(true, Ordering::Release);
        *device
            .physical_task_extent
            .lock()
            .expect("fake physical task extent") = Some((2, 1));
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = RuntimeHost::start(
            config(&state_root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock)
                .with_standard_packs_root(Some(packs.path().to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(FakeProvider::one(
                INSTANCE,
                instance_id(),
                Arc::clone(&device),
            )),
        )
        .expect("start the composer-mode host");
        let scene = Self {
            _state_root: state_root,
            packs,
            device,
            host,
        };
        scene.sample_capacity();
        // The workbench checks the game roots at the start of a policy cycle; what the cycle
        // dispatches is not this test's.
        let _cycle = scene
            .host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                7,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy: {error}"));
        let roots = scene
            .host
            .standard_pack_goal_roots()
            .expect("the workbench's roots")
            .expect("a composed standard pack");
        assert!(roots.roots.contains_key(GAME_A), "the root is judged");
        scene
    }

    // Fix round 1: the word that keeps the variable `name` from applying in game A's root, as the
    // workbench's table says it (`None`: it applies).
    fn root_word(&self, name: &str) -> Option<VariableProblem> {
        self.host
            .standard_pack_goal_roots()
            .expect("the workbench's roots")
            .expect("a composed standard pack")
            .table
            .root_word(GAME_A, name)
    }

    // The request of one direct run of the package with the values `given`.
    fn request(&self, given: &[(&str, Value)]) -> ContainedTaskRequest {
        ContainedTaskRequest::new(
            self.packs
                .path()
                .join(GAME_A)
                .join(TASK)
                .to_string_lossy()
                .into_owned(),
        )
        .expect("contained task request")
        .with_variables(
            given
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        )
        .expect("the values' names are variables' names")
    }

    // The device shows `home` until the next run's tap, then `result`.
    fn script_device(&self) {
        let taps = self.device.input_count.load(Ordering::Acquire);
        *self.device.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                vec![(0, if inputs <= taps { HOME } else { RESULT })]
            }));
    }

    fn sample_capacity(&self) {
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample");
    }

    // One direct run of the package with the values `given`, on a connection of its own; the
    // device shows `home` until the run's tap, then `result`. Its receipt.
    fn run(&self, given: &[(&str, Value)]) -> RuntimeReceipt {
        self.script_device();
        self.sample_capacity();
        let request = self.request(given);
        let mut client = TestClient::connect(&self.host);
        client.set_receipt_read_timeout();
        let run = client.request(RuntimeOperation::run_contained_task(
            INSTANCE,
            client.ids.mint_holder_id().expect("holder"),
            request,
        ));
        let receipt = client.send(&run);
        drop(client);
        receipt
    }

    // Fix round 1: how many runs the ledger records the admission of (`task.package_admitted`).
    fn admitted(&self) -> usize {
        self.host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("the ledger's records")
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Task(TaskPayload::Semantic(payload))
                        if matches!(payload.fact(), TaskSemanticFact::PackageAdmitted { .. })
                )
            })
            .count()
    }

    // What a refused run must leave as it was: the taps, the runs that read their parameters and
    // (fix round 1) the admissions the ledger records.
    fn counted(&self) -> (usize, usize, usize) {
        (self.taps(), self.runs_read(), self.admitted())
    }

    // How many runs reached their execution and overlaid their snapshots.
    fn runs_read(&self) -> usize {
        self.host
            .run_parameters_for_test()
            .expect("the parameters of the host's runs")
            .len()
    }

    fn taps(&self) -> usize {
        self.device.input_count.load(Ordering::Acquire)
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

// The value of `session.dispatch.parameter.pick` in a snapshot overlaid with what the host's last
// run read (`None` when it holds none): what the run's snapshots hold.
fn overlaid_pick(scene: &Scene) -> Option<FactValue> {
    overlaid(scene, PICK_KEY)
}

// The value of the record `key` in a snapshot overlaid with what the host's last run read (`None`
// when it holds none); the fix round of the 0.12.68 train reads `list_item` by it too.
fn overlaid(scene: &Scene, key: &str) -> Option<FactValue> {
    let runs = scene
        .host
        .run_parameters_for_test()
        .expect("the parameters of the host's runs");
    let parameters = runs.last().expect("a run read its parameters");
    let mut snapshot = InstanceFactSnapshot {
        snapshot_id: "snapshot-s38-host".to_owned(),
        ledger_position: 1,
        context: InstanceFactContext {
            instance_id: INSTANCE.to_owned(),
            server_id: SERVER_A.to_owned(),
            game_id: GAME_A.to_owned(),
        },
        records: Vec::new(),
    };
    parameters.overlay(&mut snapshot, POLICY_NOW_UNIX_MS);
    snapshot
        .records
        .iter()
        .find(|record| record.key == key)
        .map(|record| match &record.content {
            FactContent::Inline { value } => value.clone(),
            other => panic!("an overlay record is inline: {other:?}"),
        })
}

// A refused run: Denied with `code` on top and `values` among its values; no tap, no run read its
// parameters and (fix round 1) the ledger records no admission since `taps`, `runs` and
// `admitted` were counted ([`Scene::counted`]).
#[track_caller]
fn refused(
    scene: &Scene,
    receipt: &RuntimeReceipt,
    code: &str,
    values: &[(&str, Value)],
    (taps, runs, admitted): (usize, usize, usize),
) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let outcome = receipt.outcome();
    assert_eq!(outcome.code().as_str(), code, "{receipt:?}");
    for (key, value) in values {
        assert_eq!(outcome.value(key), Some(value), "{key}: {receipt:?}");
    }
    assert_eq!(scene.taps(), taps, "nothing was pressed");
    assert_eq!(scene.runs_read(), runs, "no run reached its execution");
    assert_eq!(
        scene.admitted(),
        admitted,
        "the ledger records no admission of the run"
    );
}

#[test]
fn s38_host_a_given_value_is_what_the_package_reads() {
    let scene = Scene::new();
    let taps = scene.taps();
    let receipt = scene.run(&[("pick", json!("item_a"))]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(scene.taps(), taps + 1, "the run's one tap");
    assert_eq!(
        overlaid_pick(&scene),
        Some(FactValue::String("item_a".to_owned())),
        "the given value is overlaid"
    );
    scene.close();
}

#[test]
fn s38_host_a_value_the_variable_does_not_take_is_refused_before_admission() {
    let scene = Scene::new();
    let counted = scene.counted();
    let receipt = scene.run(&[("pick", json!("item_z"))]);
    refused(
        &scene,
        &receipt,
        "run_variable_value_refused",
        &[
            ("package_id", json!(TASK)),
            ("variable_name", json!("pick")),
            ("variable_problem", json!("not_an_option")),
        ],
        counted,
    );
    scene.close();
}

#[test]
fn s38_host_an_undeclared_name_is_refused() {
    let scene = Scene::new();
    let counted = scene.counted();
    let receipt = scene.run(&[("other", json!(1))]);
    refused(
        &scene,
        &receipt,
        "run_variable_undeclared",
        &[
            ("package_id", json!(TASK)),
            ("variable_name", json!("other")),
        ],
        counted,
    );
    scene.close();
}

#[test]
fn s38_host_without_values_the_run_reads_the_defaults() {
    let scene = Scene::new();
    let receipt = scene.run(&[]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        overlaid_pick(&scene),
        Some(FactValue::String("item_b".to_owned())),
        "the default is overlaid"
    );
    scene.close();
}

#[test]
fn s38_fix1_a_variable_that_does_not_apply_is_refused_before_admission() {
    let scene = Scene::with(place_with_goals_taking_pick);
    assert_eq!(
        scene.root_word("pick"),
        Some(VariableProblem::GoalNameConflict),
        "goals.json takes the name"
    );
    let counted = scene.counted();
    let receipt = scene.run(&[("pick", json!("item_a"))]);
    refused(
        &scene,
        &receipt,
        "run_variable_not_applying",
        &[
            ("package_id", json!(TASK)),
            ("variable_name", json!("pick")),
            (
                "variable_problem",
                json!(VariableProblem::GoalNameConflict.as_str()),
            ),
        ],
        counted,
    );
    scene.close();
}

#[test]
fn s38_fix1_a_value_taken_only_in_part_is_refused() {
    let scene = Scene::with(place_with_order);
    assert_eq!(scene.root_word("pick"), None, "pick applies");
    assert_eq!(scene.root_word("order"), None, "order applies");
    let counted = scene.counted();
    let receipt = scene.run(&[("order", json!(["item_a", "item_z"]))]);
    refused(
        &scene,
        &receipt,
        "run_variable_value_refused",
        &[
            ("package_id", json!(TASK)),
            ("variable_name", json!("order")),
            ("variable_problem", json!("keys_left_out")),
        ],
        counted,
    );
    scene.close();
}

#[test]
fn s38_fix1_seventeen_values_in_a_raw_request_are_refused_at_the_door() {
    let scene = Scene::new();
    scene.script_device();
    scene.sample_capacity();
    let counted = scene.counted();
    let mut client = TestClient::connect(&scene.host);
    client.set_receipt_read_timeout();
    let run = client.request(RuntimeOperation::run_contained_task(
        INSTANCE,
        client.ids.mint_holder_id().expect("holder"),
        scene.request(&[]),
    ));
    // The contract's constructor refuses seventeen values, so they are written into the request's
    // JSON: each under a name of a variable's form.
    let mut wire = serde_json::to_value(&run).expect("the run's request as JSON");
    wire["operation"]["request"]["variables"] = Value::Object(
        (0..=MAX_PACKAGE_VARIABLES)
            .map(|index| (format!("v{index:02}"), json!(index)))
            .collect(),
    );
    write_frame(&mut client.stream, &wire, DEFAULT_RUNTIME_MAX_FRAME_BYTES)
        .expect("write the request frame");
    let frame = match read_frame(&mut client.stream, DEFAULT_RUNTIME_MAX_FRAME_BYTES)
        .expect("read the receipt frame")
    {
        FrameRead::Data(frame) => frame,
        FrameRead::Idle | FrameRead::Closed => panic!("the Runtime wrote no receipt"),
    };
    drop(client);
    let receipt: RuntimeReceipt = serde_json::from_slice(&frame).expect("a receipt");
    refused(
        &scene,
        &receipt,
        "invalid_contained_task_variable_count",
        &[],
        counted,
    );
    scene.close();
}

// The fix round of the 0.12.68 train: a value of a generated choice's `ordered` variable that its
// declaration allows, not in the options' own order, is accepted, and the run reads it in its order:
// one row per key, `rank` from 1.
#[test]
fn train_01268_fix1_an_allowed_order_of_a_generated_choice_is_what_the_package_reads() {
    let scene = Scene::with(place_with_generated_choice);
    assert_eq!(scene.root_word("list_item"), None, "list_item applies");
    let taps = scene.taps();
    let receipt = scene.run(&[("list_item", json!(["item_b", "item_a"]))]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(scene.taps(), taps + 1, "the run's one tap");
    let row = |key: &str, rank: i64| {
        BTreeMap::from([
            (
                ORDERED_VARIABLE_KEY_COLUMN.to_owned(),
                FactScalar::String(key.to_owned()),
            ),
            (
                ORDERED_VARIABLE_RANK_COLUMN.to_owned(),
                FactScalar::Integer(rank),
            ),
        ])
    };
    assert_eq!(
        overlaid(&scene, LIST_ITEM_KEY),
        Some(FactValue::RecordList(vec![
            row("item_b", 1),
            row("item_a", 2)
        ])),
        "the given value is overlaid in its own order"
    );
    scene.close();
}
