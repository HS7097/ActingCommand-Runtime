// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V2, host half (0.12.61; register §1.382; contracts/package-variables.md, "The
// run"; coordinator work order rd-1115/brief-463-v2-variables-run.md): a policy dispatch of a task
// whose package declares a variable hands its run the value set on the application, else the
// package default, and says so on its `policy_dispatch_parameters`; the run's parameters read the
// value from that reason; a start reads the stored value again; a value set on the application
// that the declaration no longer takes is warned once (`goal_value_not_applied`), the run reading
// the default.
//
// A neutral fixture, built as `gate_442_rem1` builds its own. The host runs in composer mode over
// one game root on disk, a manual Runtime clock and one fake instance. The root's `schedule.json`
// holds one rule, `fixture.vars`, due at every evaluation; its package goes from `home` to
// `result` in one tap and declares the variable `pick` (a `choice` of `item_a` and `item_b`,
// default `item_b`). Values are set through `ApplyResourceTargets` on a connection bound `lab`, as
// `targets set` with `entries` writes them. Every expected value is the work order's.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{DeclaredInitiator, Initiator, RuntimeChannel, RuntimePayload};
use actingcommand_policy::PolicyEvaluation;
use serde_json::{Value, json};
use std::fs;

const TASK: &str = "fixture.vars";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const PARAMETERS: &str = "policy_dispatch_parameters";
const NOT_APPLIED: &str = "goal_value_not_applied";

// ---- The game root.

// The variable the package declares.
fn variables() -> Value {
    json!([{"name": "pick", "type": "choice", "title": {"en": "Item"},
            "options": [{"value": "item_a", "title": {"en": "Item A"}},
                        {"value": "item_b", "title": {"en": "Item B"}}],
            "default": "item_b"}])
}

// Places the game root below `packs`: its bundle, its `schedule.json` with the one rule, due at
// every evaluation, and the package, whose control declaration declares [`variables`].
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
    state_root: TempDir,
    packs: TempDir,
    instance: InstanceId,
    device: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

fn start_host(
    state_root: &TempDir,
    packs: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    device: &Arc<FakeState>,
    instance: InstanceId,
) -> RuntimeHost {
    RuntimeHost::start(
        config(state_root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_standard_packs_root(Some(packs.path().to_owned()))
            .with_standard_pack_schedule(true),
        Arc::new(FakeProvider::one(INSTANCE, instance, Arc::clone(device)).fixture_simulation()),
    )
    .expect("start the composer-mode host")
}

impl Scene {
    fn new() -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path());
        let device = Arc::new(FakeState::default());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let instance = instance_id();
        let host = start_host(&state_root, &packs, &clock, &device, instance);
        Self {
            state_root,
            packs,
            instance,
            device,
            clock,
            host,
        }
    }

    // The host closed and started again on the same files, state root and instance, its clock at
    // `at`.
    fn restart(self, at: u64) -> Self {
        let Self {
            state_root,
            packs,
            instance,
            device,
            host,
            ..
        } = self;
        host.close().expect("close the composer-mode host");
        let clock = Arc::new(ManualRuntimeClock::new(at, 0));
        let host = start_host(&state_root, &packs, &clock, &device, instance);
        Self {
            state_root,
            packs,
            instance,
            device,
            clock,
            host,
        }
    }

    fn sample_capacity(&self) {
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample");
    }

    // One full evaluation at `at`, the Runtime clock moved there first.
    fn evaluate(&self, at: u64, seed: u64) -> PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        self.sample_capacity();
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                seed,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy at {at}: {error}"))
            .evaluation
            .expect("a full evaluation")
    }

    fn request(&self) -> ContainedTaskRequest {
        ContainedTaskRequest::new(
            self.packs
                .path()
                .join(GAME_A)
                .join(TASK)
                .to_string_lossy()
                .into_owned(),
        )
        .expect("contained task request")
    }

    // Evaluates at `at`, admits the dispatch of the rule it must select, runs it (`home`, one
    // tap, `result`) and settles it: the reason chain the dispatch carried and its run's context.
    fn evaluate_and_run(&self, at: u64, seed: u64) -> (DecisionReasonChain, Box<PolicyRunContext>) {
        let evaluation = self.evaluate(at, seed);
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK && intent.instance_id == INSTANCE)
            .unwrap_or_else(|| panic!("a dispatch of {TASK}: {evaluation:#?}"))
            .clone();
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the intent's reason chain")
            .clone();
        self.sample_capacity();
        let admission = PolicyAdmissionContext {
            fact_ledger_position: intent.input_ledger_position,
            fact_snapshot_id: intent.fact_snapshot_id.clone(),
            approval_fact_ids: BTreeSet::new(),
            fencing_owner_epoch: self.host.runtime_info().owner_epoch(),
            now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
        };
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(&intent, &chain, &admission, &self.request())
            .unwrap_or_else(|error| panic!("admit {TASK}: {error}"))
        else {
            panic!("expected one policy run context")
        };
        // The device: `home` until the run's tap, then `result`.
        let taps = self.device.input_count.load(Ordering::Acquire);
        *self.device.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                vec![(0, if inputs <= taps { HOME } else { RESULT })]
            }));
        let receipt = self
            .host
            .run_scheduled_contained_task(&context, &self.request())
            .expect("the scheduled run");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        self.host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        (chain, context)
    }

    // `ApplyResourceTargets` of a v3 document of the instance with no target and the one entry
    // `pick` = `value`, on a connection bound `lab` with `Lab-CLI` and the Lab's envelope, the
    // Runtime clock at `at` first. It is applied.
    fn set_pick(&self, value: Value, at: u64) {
        advance_manual_clock_to(&self.clock, at);
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator: Some(DeclaredInitiator::from(Initiator::LabCli)),
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
        let document = json!({
            "schema_version": "actingcommand.resource-targets.v3",
            "instance": INSTANCE,
            "targets": [],
            "entries": [{"name": "pick", "value": value}]
        });
        let request = RuntimeRequest::new(
            client.ids.mint_request_id().expect("request id"),
            client.ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::Lab,
            EventSource::Lab,
            1,
            RuntimeOperation::ApplyResourceTargets {
                document_json: document.to_string(),
                expected_version: None,
            },
        )
        .expect("lab runtime request");
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "the value is set: {receipt:?}"
        );
    }

    // The outcomes of the `runtime.failed` records of `code`, in ledger order.
    fn failed(&self, code: &str) -> Vec<Outcome> {
        let mut events = self
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::RuntimeFailed),
                ..EventQuery::default()
            })
            .expect("the runtime.failed records");
        events.sort_by_key(PersistedEvent::sequence);
        events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::Failed(record))
                    if record.outcome().code().as_str() == code =>
                {
                    Some(record.outcome().clone())
                }
                _ => None,
            })
            .collect()
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

// The `variable_values` of the chain's one `policy_dispatch_parameters`.
fn variable_values(chain: &DecisionReasonChain) -> Value {
    let parameters = chain
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == PARAMETERS)
        .map(|reason| &reason.outcome)
        .collect::<Vec<_>>();
    assert_eq!(parameters.len(), 1, "{chain:?}");
    parameters[0]
        .value("variable_values")
        .cloned()
        .unwrap_or(Value::Null)
}

// The variable values the run of `context` reads from its dispatch's reason.
fn run_values(context: &PolicyRunContext) -> Vec<(String, Value)> {
    crate::fact_refresh::dispatch_parameters(context)
        .expect("the run's dispatch parameters")
        .variables
}

// When the instance may run its next dispatch after the run of `context`.
fn next_eligible(context: &PolicyRunContext) -> u64 {
    context.admission().activity.next_eligible_unix_ms
}

// Requirements 1, 4, 5 and the work order's tests "set beats default", "restart reloads" and "the
// warning once": with no value set, the dispatch's reason records the default `item_b` (source
// `default`) and its run reads it; a value set through the Lab (`item_a`) is the next dispatch's,
// recorded with source `set`, when it was set and the channel `lab`, and its run reads it; a start
// reads the stored value again and the dispatch after it hands it to its run unchanged. A value the
// declaration no longer takes (`item_z`) reads the default with `not_an_option` and who set it, on
// each of the next two dispatches, while one `goal_value_not_applied` names the instance, the
// task, the package, the variable, its declaration and the word.
#[test]
fn v2_host_a_dispatch_hands_its_run_the_set_value_and_a_start_reads_it_again() {
    let scene = Scene::new();
    let (chain, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "default",
                "value_json": "\"item_b\""}])
    );
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_b"))]);

    let set_at = next_eligible(&context);
    scene.set_pick(json!("item_a"), set_at);
    let (chain, context) = scene.evaluate_and_run(set_at, 8);
    let set = json!([{"variable_name": "pick", "variable_source": "set",
                      "value_json": "\"item_a\"", "set_unix_ms": set_at, "set_channel": "lab"}]);
    assert_eq!(variable_values(&chain), set);
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_a"))]);
    assert!(scene.failed(NOT_APPLIED).is_empty());

    let restart_at = next_eligible(&context);
    let scene = scene.restart(restart_at);
    let (chain, context) = scene.evaluate_and_run(restart_at, 9);
    assert_eq!(
        variable_values(&chain),
        set,
        "the start reads the stored value"
    );
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_a"))]);

    let gone_at = next_eligible(&context);
    scene.set_pick(json!("item_z"), gone_at);
    let mut at = gone_at;
    for seed in [10, 11] {
        let (chain, context) = scene.evaluate_and_run(at, seed);
        assert_eq!(
            variable_values(&chain),
            json!([{"variable_name": "pick", "variable_source": "default",
                    "value_json": "\"item_b\"", "set_unix_ms": gone_at, "set_channel": "lab",
                    "variable_problem": "not_an_option"}]),
            "seed {seed}"
        );
        assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_b"))]);
        at = next_eligible(&context);
    }
    let warnings = scene.failed(NOT_APPLIED);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    let warning = &warnings[0];
    for (key, expected) in [
        ("instance_id", INSTANCE),
        ("task_id", TASK),
        ("package_id", TASK),
        ("variable_name", "pick"),
        ("variable_problem", "not_an_option"),
    ] {
        assert_eq!(
            warning.value(key).and_then(Value::as_str),
            Some(expected),
            "{key}: {warning}"
        );
    }
    assert!(
        warning
            .value("declaration_sha256")
            .and_then(Value::as_str)
            .is_some_and(|sha| sha.starts_with("sha256:") && sha.len() == 71),
        "{warning}"
    );
    scene.close();
}
