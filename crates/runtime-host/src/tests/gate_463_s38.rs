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
//   run parameters read, no record of the run's admission.
// - `s38_host_an_undeclared_name_is_refused`: a value under `other`: Denied
//   `run_variable_undeclared` with the package and the name; nothing ran.
// - `s38_host_without_values_the_run_reads_the_defaults`: no value: the run completes and reads
//   `item_b`.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use actingcommand_contract::{FactContent, FactValue, InstanceFactContext, InstanceFactSnapshot};
use serde_json::{Value, json};
use std::fs;

const TASK: &str = "fixture.s38";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const PICK_KEY: &str = "session.dispatch.parameter.pick";

// ---- The game root.

// The variable the package declares.
fn variables() -> Value {
    json!([{"name": "pick", "type": "choice", "title": {"en": "Item"},
            "options": [{"value": "item_a", "title": {"en": "Item A"}},
                        {"value": "item_b", "title": {"en": "Item B"}}],
            "default": "item_b"}])
}

// Places the game root below `packs`: its bundle, its `schedule.json` with the one rule, and the
// package, whose control declaration declares [`variables`].
fn place_game_root(packs: &Path) {
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
    control["variables"] = variables();
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
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path());
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
        assert_eq!(roots.table.root_word(GAME_A, "pick"), None, "pick applies");
        scene
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
        let taps = self.device.input_count.load(Ordering::Acquire);
        *self.device.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                vec![(0, if inputs <= taps { HOME } else { RESULT })]
            }));
        self.sample_capacity();
        let request = ContainedTaskRequest::new(
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
        .expect("the values' names are variables' names");
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
        .find(|record| record.key == PICK_KEY)
        .map(|record| match &record.content {
            FactContent::Inline { value } => value.clone(),
            other => panic!("an overlay record is inline: {other:?}"),
        })
}

// A refused run: Denied with `code` on top and `values` among its values; no tap and no run read
// its parameters since `taps` and `runs` were counted.
#[track_caller]
fn refused(
    scene: &Scene,
    receipt: &RuntimeReceipt,
    code: &str,
    values: &[(&str, Value)],
    (taps, runs): (usize, usize),
) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let outcome = receipt.outcome();
    assert_eq!(outcome.code().as_str(), code, "{receipt:?}");
    for (key, value) in values {
        assert_eq!(outcome.value(key), Some(value), "{key}: {receipt:?}");
    }
    assert_eq!(scene.taps(), taps, "nothing was pressed");
    assert_eq!(scene.runs_read(), runs, "no run reached its execution");
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
    let counted = (scene.taps(), scene.runs_read());
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
    let counted = (scene.taps(), scene.runs_read());
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
