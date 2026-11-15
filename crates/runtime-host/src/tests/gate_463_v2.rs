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
use actingcommand_contract::outcome::{ENVELOPE_BODY_BUDGET, Outcome};
use actingcommand_contract::{
    DeclaredInitiator, FactContent, FactValue, Initiator, InstanceFactContext,
    InstanceFactSnapshot, RuntimeChannel, RuntimeLifecyclePhase, RuntimePayload, VariableProblem,
};
use actingcommand_policy::PolicyEvaluation;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;

const TASK: &str = "fixture.vars";
/// Fix round 1: a package of the root that no rule runs.
const HELD: &str = "fixture.held";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const PARAMETERS: &str = "policy_dispatch_parameters";
const NOT_APPLIED: &str = "goal_value_not_applied";
/// Fix round 2 (item 6): a dispatched package the workbench could not read.
const PACKAGE_UNREAD: &str = "goal_value_package_unreadable";
/// Completed in #1731 (#1728's review, item 11): a game root's `goals.json` not applied.
const GOALS_NOT_APPLIED: &str = "standard_pack_goals_not_applied";
const PICK_KEY: &str = "session.dispatch.parameter.pick";
/// The default `standard_packs.check_interval_ms`: a check reads the roots again after it.
const CHECK_INTERVAL_MS: u64 = 60_000;

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
    patch_control(&package, |control| control["variables"] = variables());
}

// Fix round 1: changes the control declaration of the package in `package` in place.
fn patch_control(package: &Path, change: impl FnOnce(&mut Value)) {
    let control_path = package.join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    change(&mut control);
    write_json(&control_path, &control);
}

// Fix round 1: the game root of [`place_game_root`] whose registry also lists [`HELD`], placed
// below `packs` with `provides` (none when `None`): a package no rule runs.
fn place_with_held(packs: &Path, provides: Option<Value>) {
    place_game_root(packs);
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[TASK, HELD]),
    );
    write_package(&game.join(HELD), HELD, None, provides);
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
        Self::with(place_game_root)
    }

    // Fix round 1: the host over the game root `place` places below its standard-pack root.
    fn with(place: impl FnOnce(&Path)) -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place(packs.path());
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
        self.set_entries(json!([{"name": "pick", "value": value}]), at);
    }

    // `ApplyResourceTargets` of a v3 document of the instance with no target and the entries
    // `entries`, as `set_pick` sends it (completed in #1731: the record bound's test sets sixteen).
    fn set_entries(&self, entries: Value, at: u64) {
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
            "entries": entries
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

    // Completed in #1731 (#1728's review, item 11): the `standard_pack_goals_not_applied`
    // outcomes of the workbench's `standard_pack_read` lifecycle records, in ledger order.
    fn goals_not_applied(&self) -> Vec<Outcome> {
        let mut events = self
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::RuntimeLifecycleObserved),
                ..EventQuery::default()
            })
            .expect("the lifecycle records");
        events.sort_by_key(PersistedEvent::sequence);
        events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => {
                    match value.phase() {
                        RuntimeLifecyclePhase::StandardPackRead { outcome }
                            if outcome.code().as_str() == GOALS_NOT_APPLIED =>
                        {
                            Some(outcome.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    // Completed in #1731 (#1728's review, item 10): the reasons of each policy dispatch record of
    // `event_type` (the intent or the admission) of the fixture's task the ledger holds, in ledger
    // order, as recorded.
    fn dispatch_records(&self, event_type: EventType) -> Vec<Vec<Outcome>> {
        let mut events = self
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(event_type),
                ..EventQuery::default()
            })
            .expect("the policy dispatch records");
        events.sort_by_key(PersistedEvent::sequence);
        events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(
                    PolicyPayload::DispatchIntent(payload)
                    | PolicyPayload::DispatchAdmitted(payload),
                ) if payload.task_id() == TASK => Some(
                    payload
                        .reasons()
                        .iter()
                        .map(|reason| reason.0.clone())
                        .collect(),
                ),
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

// Fix round 1 (item 8): the value of `session.dispatch.parameter.pick` in a snapshot overlaid
// with what the host's last run read (`None` when it holds none): what the run's snapshots hold.
fn overlaid_pick(scene: &Scene) -> Option<FactValue> {
    let runs = scene
        .host
        .run_parameters_for_test()
        .expect("the parameters of the host's runs");
    let parameters = runs.last().expect("a run read its parameters");
    let mut snapshot = InstanceFactSnapshot {
        snapshot_id: "snapshot-v2-host".to_owned(),
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

// The string fact `text`.
fn text(text: &str) -> Option<FactValue> {
    Some(FactValue::String(text.to_owned()))
}

// Requirements 1, 4, 5 and the work order's tests "set beats default", "restart reloads" and "the
// warning once": with no value set, the dispatch's reason records the default `item_b` (source
// `default`) and its run reads it; a value set through the Lab (`item_a`) is the next dispatch's,
// recorded with source `set`, when it was set and the channel `lab`, and its run reads it; a start
// reads the stored value again and the dispatch after it hands it to its run unchanged. A value the
// declaration no longer takes (`item_z`) reads the default with `not_an_option` and who set it, on
// each of the next two dispatches, while one `goal_value_not_applied` names the instance, the
// task, the package, the variable, its declaration and the word. Fix round 1: the run's
// snapshots hold each value the record says (item 8), and replacing the value not applied by
// another one not applied (`item_y`) is warned once more (item 5).
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
    assert_eq!(overlaid_pick(&scene), text("item_b"));

    let set_at = next_eligible(&context);
    scene.set_pick(json!("item_a"), set_at);
    let (chain, context) = scene.evaluate_and_run(set_at, 8);
    let set = json!([{"variable_name": "pick", "variable_source": "set",
                      "value_json": "\"item_a\"", "set_unix_ms": set_at, "set_channel": "lab"}]);
    assert_eq!(variable_values(&chain), set);
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_a"))]);
    assert_eq!(
        overlaid_pick(&scene),
        text("item_a"),
        "the set value is overlaid"
    );
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
    assert_eq!(overlaid_pick(&scene), text("item_a"));

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
        assert_eq!(overlaid_pick(&scene), text("item_b"), "seed {seed}");
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

    // Fix round 1 (item 5): another value the declaration does not take is warned once more.
    scene.set_pick(json!("item_y"), at);
    let (chain, context) = scene.evaluate_and_run(at, 12);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "default",
                "value_json": "\"item_b\"", "set_unix_ms": at, "set_channel": "lab",
                "variable_problem": "not_an_option"}])
    );
    assert_eq!(overlaid_pick(&scene), text("item_b"));
    let warnings = scene.failed(NOT_APPLIED);
    assert_eq!(warnings.len(), 2, "{warnings:#?}");
    assert_eq!(
        warnings[1].value("declaration_sha256"),
        warning.value("declaration_sha256"),
        "the same declaration, another value"
    );
    assert_eq!(
        warnings[1]
            .value("variable_problem")
            .and_then(Value::as_str),
        Some("not_an_option")
    );
    let again_at = next_eligible(&context);
    scene.evaluate_and_run(again_at, 13);
    assert_eq!(scene.failed(NOT_APPLIED).len(), 2, "once per value");
    scene.close();
}

// Fix round 1, item 2 (the coordinator's ruling: the judgement of "does not apply" is the root
// check's, over the whole root): with a composed standard pack the host judges every package of
// the game root's registry, a package no rule runs included, and the root's `goals.json`, read
// with the contract crate's reader. In each case the dispatch after a value is set through the
// Lab (`item_a`) hands its run the default `item_b`, its record names who set the value and the
// word, the run's snapshots hold `item_b`, and one `goal_value_not_applied` says the word: a
// package no rule runs marks irreversible a capability the dispatched package provides unmarked
// (`irreversible_by_root`); it declares `pick` differently (`declaration_conflict`); an entry of
// the accepted `goals.json` uses the name `pick` for a heading (`goal_name_conflict`); its control
// declaration cannot be read (item 3: `package_unreadable`, not `irreversible_by_root`).
#[test]
fn v2_fix1_the_whole_root_keeps_a_variable_from_applying() {
    fn held_marks_irreversible(packs: &Path) {
        // Fix round 2 (item 9): an irreversible capability declares its parameters, or the
        // package reader refuses the whole `provides` (`validate_provides`); without them the
        // held package could not be read and the case read `package_unreadable`.
        place_with_held(
            packs,
            Some(
                json!([{"capability": "cap_x", "irreversible": true, "parameters": [
                    {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}
                ]}]),
            ),
        );
        patch_control(&packs.join(GAME_A).join(TASK), |control| {
            control["provides"] = json!([{"capability": "cap_x"}]);
        });
    }
    fn held_declares_otherwise(packs: &Path) {
        place_with_held(packs, None);
        patch_control(&packs.join(GAME_A).join(HELD), |control| {
            let mut differing = variables();
            differing[0]["default"] = json!("item_a");
            control["variables"] = differing;
        });
    }
    fn goals_take_the_name(packs: &Path) {
        place_game_root(packs);
        write_json(
            &packs.join(GAME_A).join("goals.json"),
            &json!({"schema_version": "actingcommand.goals.v2", "game": GAME_A,
                    "entries": [{"name": "pick", "type": "heading",
                                 "title": {"en": "Pick"}}]}),
        );
    }
    fn held_unreadable(packs: &Path) {
        place_with_held(packs, None);
        write_json(
            &packs.join(GAME_A).join(HELD).join("control.json"),
            &json!({"schema_version": "Lab-1y.control.v2"}),
        );
    }
    kept_from_applying(
        "a package no rule runs marks the capability irreversible",
        held_marks_irreversible,
        VariableProblem::IrreversibleByRoot,
    );
    kept_from_applying(
        "a package no rule runs declares the name otherwise",
        held_declares_otherwise,
        VariableProblem::DeclarationConflict,
    );
    kept_from_applying(
        "goals.json takes the name",
        goals_take_the_name,
        VariableProblem::GoalNameConflict,
    );
    kept_from_applying(
        "a package of the root cannot be read",
        held_unreadable,
        VariableProblem::PackageUnreadable,
    );
}

// The case `case` of [`v2_fix1_the_whole_root_keeps_a_variable_from_applying`]: the root `place`
// places keeps `pick` from applying with `problem`.
fn kept_from_applying(case: &str, place: impl FnOnce(&Path), problem: VariableProblem) {
    let scene = Scene::with(place);
    let (chain, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "default",
                "value_json": "\"item_b\""}]),
        "{case}: nothing set"
    );
    let set_at = next_eligible(&context);
    scene.set_pick(json!("item_a"), set_at);
    let (chain, _) = scene.evaluate_and_run(set_at, 8);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "default",
                "value_json": "\"item_b\"", "set_unix_ms": set_at, "set_channel": "lab",
                "variable_problem": problem.as_str()}]),
        "{case}"
    );
    assert_eq!(overlaid_pick(&scene), text("item_b"), "{case}");
    let warnings = scene.failed(NOT_APPLIED);
    assert_eq!(warnings.len(), 1, "{case}: {warnings:#?}");
    assert_eq!(
        warnings[0]
            .value("variable_problem")
            .and_then(Value::as_str),
        Some(problem.as_str()),
        "{case}"
    );
    scene.close();
}

// Fix round 1, item 2: a package replaced in place is judged with its new declaration after the
// workbench's next read of the root (its control declaration's marks changed). With `item_a` set
// and applied, the package's declaration is rewritten in place without the option `item_a`; the
// first dispatch after the next check hands its run the default `item_b` with `not_an_option`.
#[test]
fn v2_fix1_a_package_replaced_in_place_is_judged_with_its_new_declaration() {
    let scene = Scene::new();
    let (_, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7);
    let set_at = next_eligible(&context);
    scene.set_pick(json!("item_a"), set_at);
    let (chain, context) = scene.evaluate_and_run(set_at, 8);
    assert_eq!(
        variable_values(&chain)[0]["variable_source"],
        "set",
        "{chain:?}"
    );
    assert_eq!(overlaid_pick(&scene), text("item_a"));

    patch_control(&scene.packs.path().join(GAME_A).join(TASK), |control| {
        control["variables"] = json!([{
            "name": "pick", "type": "choice", "title": {"en": "Item"},
            "options": [{"value": "item_b", "title": {"en": "Item B"}},
                        {"value": "item_c", "title": {"en": "Item C"}},
                        {"value": "item_d", "title": {"en": "Item D"}}],
            "default": "item_b"
        }]);
    });
    let read_at = next_eligible(&context).max(set_at + CHECK_INTERVAL_MS);
    let (chain, _) = scene.evaluate_and_run(read_at, 9);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "default",
                "value_json": "\"item_b\"", "set_unix_ms": set_at, "set_channel": "lab",
                "variable_problem": VariableProblem::NotAnOption.as_str()}])
    );
    assert_eq!(overlaid_pick(&scene), text("item_b"));
    assert_eq!(scene.failed(NOT_APPLIED).len(), 1);
    scene.close();
}

// The gate tests' seam that makes the metadata of a file fail as a file the process may not stat
// (`DECLARATION_METADATA_FAILS_FOR_TEST`); each test names files below its own root.
fn metadata_seam() -> std::sync::MutexGuard<'static, BTreeSet<std::path::PathBuf>> {
    crate::host::DECLARATION_METADATA_FAILS_FOR_TEST
        .lock()
        .expect("the metadata seam")
}

// Fix round 2, item 1 (the coordinator's ruling: the texts are right): while a game's root fails
// its check, nothing says whether a variable applies, so every variable of that game's instances
// reads its default with `root_unchecked`. With `item_a` set and applied, the metadata of the
// root's `schedule.json` cannot be read at the next check (the seam): the dispatch after it hands
// its run the default `item_b`, its record says `default` with `root_unchecked` and who set the
// value, the run's snapshots hold `item_b`, and one `goal_value_not_applied` says
// `root_unchecked`. Once the root reads again, the dispatch after the next check hands its run
// `item_a` again.
#[test]
fn v2_fix2_a_root_that_fails_its_check_reads_each_variable_at_its_default() {
    let scene = Scene::new();
    let (_, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7);
    let set_at = next_eligible(&context);
    scene.set_pick(json!("item_a"), set_at);
    let (chain, context) = scene.evaluate_and_run(set_at, 8);
    assert_eq!(
        variable_values(&chain)[0]["variable_source"],
        "set",
        "{chain:?}"
    );

    let failing = scene.packs.path().join(GAME_A).join("schedule.json");
    metadata_seam().insert(failing.clone());
    let failed_at = next_eligible(&context).max(set_at + CHECK_INTERVAL_MS);
    let (chain, context) = scene.evaluate_and_run(failed_at, 9);
    metadata_seam().remove(&failing);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "default",
                "value_json": "\"item_b\"", "set_unix_ms": set_at, "set_channel": "lab",
                "variable_problem": VariableProblem::RootUnchecked.as_str()}])
    );
    assert_eq!(overlaid_pick(&scene), text("item_b"));
    let warnings = scene.failed(NOT_APPLIED);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert_eq!(
        warnings[0]
            .value("variable_problem")
            .and_then(Value::as_str),
        Some(VariableProblem::RootUnchecked.as_str())
    );

    let read_at = next_eligible(&context).max(failed_at + CHECK_INTERVAL_MS);
    let (chain, _) = scene.evaluate_and_run(read_at, 10);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "set",
                "value_json": "\"item_a\"", "set_unix_ms": set_at, "set_channel": "lab"}]),
        "the root judged again"
    );
    assert_eq!(overlaid_pick(&scene), text("item_a"));
    scene.close();
}

// Fix round 2, item 6: a dispatched package the workbench cannot read has no variable in its
// table, so its dispatch carries no `variable_values` and its run reads the package's defaults;
// nothing is skipped silently. The metadata of the package's control declaration cannot be read
// (the seam), while the run itself reads the package: the dispatch carries no
// `policy_dispatch_parameters`, the run's snapshots hold the default `item_b`, and one
// `goal_value_package_unreadable` names the instance, the task and the package. The next dispatch,
// after a check that finds the package's files as they were, writes no second one.
#[test]
fn v2_fix2_a_dispatched_package_the_workbench_cannot_read_is_warned_once() {
    let scene = Scene::new();
    let control = scene
        .packs
        .path()
        .join(GAME_A)
        .join(TASK)
        .join("control.json");
    metadata_seam().insert(control.clone());
    let (chain, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7);
    assert!(
        chain
            .reasons
            .iter()
            .all(|reason| reason.outcome.code().as_str() != PARAMETERS),
        "{chain:?}"
    );
    assert_eq!(overlaid_pick(&scene), text("item_b"));
    let warnings = scene.failed(PACKAGE_UNREAD);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    for (key, expected) in [
        ("instance_id", INSTANCE),
        ("task_id", TASK),
        ("package_id", TASK),
    ] {
        assert_eq!(
            warnings[0].value(key).and_then(Value::as_str),
            Some(expected),
            "{key}: {}",
            warnings[0]
        );
    }

    let again_at = next_eligible(&context).max(POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS);
    scene.evaluate_and_run(again_at, 8);
    metadata_seam().remove(&control);
    assert_eq!(scene.failed(PACKAGE_UNREAD).len(), 1, "once per state");
    assert!(scene.failed(NOT_APPLIED).is_empty());
    scene.close();
}

// ---- Completed in #1731 (#1728's review, items 10 to 12; coordinator brief
// rd-1115/brief-1731-fix1.md, the round's later items): host tests of #1728's fix round 2 that its
// review found missing. They belong to V2 and go in with V3, which merges with it.

// Item 10: a dispatch whose `policy_dispatch_parameters` passes an outcome record's bound. The
// package declares sixteen `ordered` variables of 64-byte names, each set through the Lab to a value
// of 1024 bytes (32 keys, the most quotes to escape), and provides 64 parameters of 64-byte names
// and options (the most `provides` declares), whose defaults the remedy plan resolves into the
// reason's `run_parameters`. The reason the run is admitted with is whole: the run's values hold
// every value as set. The ledger's policy dispatch records of that dispatch (the intent and the
// admission) hold the reason in its recorded form: for the first value (the longest first; all are
// equal) its text's sha256 and length and `value_json_left_out` in place of its text, every other
// value's text, the run parameters, and the record within the bound.
#[test]
fn v2_fix3_a_reason_past_the_record_bound_is_whole_for_the_run_and_shortened_in_the_ledger() {
    let keys = (0..32)
        .map(|index| {
            let filler = if index < 31 { 25 } else { 24 };
            format!("k{index:02}_{}", "x".repeat(filler))
        })
        .collect::<Vec<_>>();
    let set = json!(keys);
    let text = set.to_string();
    assert_eq!(text.len(), 1024, "setup: a value of 1024 bytes");
    let names = (0..16)
        .map(|index| format!("v{index:02}_{}", "n".repeat(60)))
        .collect::<Vec<_>>();
    let declared = names
        .iter()
        .map(|name| {
            json!({"name": name, "type": "ordered", "title": {"en": "Many"},
                   "options": keys
                       .iter()
                       .map(|key| json!({"value": key, "title": {"en": "K"}}))
                       .collect::<Vec<_>>(),
                   "min_count": 1, "max_count": 32, "default": [keys[0]]})
        })
        .collect::<Vec<_>>();
    let provides = (0..8)
        .map(|capability| {
            let parameters = (0..8)
                .map(|parameter| {
                    let index = capability * 8 + parameter;
                    let option = format!("o{index:02}_{}", "y".repeat(60));
                    json!({"name": format!("p{index:02}_{}", "x".repeat(60)),
                           "options": [option], "default": option})
                })
                .collect::<Vec<_>>();
            json!({"capability": format!("cap_{capability}"), "parameters": parameters})
        })
        .collect::<Vec<_>>();
    let scene = Scene::with(|packs| {
        place_game_root(packs);
        patch_control(&packs.join(GAME_A).join(TASK), |control| {
            control["variables"] = json!(declared);
            control["provides"] = json!(provides);
        });
    });
    let (_, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7);
    let set_at = next_eligible(&context);
    scene.set_entries(
        Value::Array(
            names
                .iter()
                .map(|name| json!({"name": name, "value": set}))
                .collect(),
        ),
        set_at,
    );
    let (chain, context) = scene.evaluate_and_run(set_at, 8);
    let size = |outcome: &Outcome| serde_json::to_vec(outcome).expect("the record form").len();
    let whole = chain
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == PARAMETERS)
        .map(|reason| reason.outcome.clone())
        .unwrap_or_else(|| panic!("the dispatch's parameters: {chain:?}"));
    assert!(
        size(&whole) > ENVELOPE_BODY_BUDGET,
        "setup: the reason passes the bound: {} bytes",
        size(&whole)
    );
    assert_eq!(
        run_values(&context),
        names
            .iter()
            .map(|name| (name.clone(), set.clone()))
            .collect::<Vec<_>>(),
        "the run reads every value whole"
    );

    let sha256 = format!("sha256:{:x}", Sha256::digest(text.as_bytes()));
    for event_type in [
        EventType::PolicyDispatchIntent,
        EventType::PolicyDispatchAdmitted,
    ] {
        let records = scene.dispatch_records(event_type);
        let reasons = records
            .last()
            .unwrap_or_else(|| panic!("a {event_type:?} record of {TASK}"));
        let recorded = reasons
            .iter()
            .find(|reason| reason.code().as_str() == PARAMETERS)
            .unwrap_or_else(|| panic!("{event_type:?}: the recorded parameters"));
        assert!(
            size(recorded) <= ENVELOPE_BODY_BUDGET,
            "{event_type:?}: the record fits: {} bytes",
            size(recorded)
        );
        assert_eq!(
            recorded.value("run_parameters"),
            whole.value("run_parameters"),
            "{event_type:?}: the run parameters kept"
        );
        let values = recorded
            .value("variable_values")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{event_type:?}: the recorded values"));
        assert_eq!(values.len(), 16, "{event_type:?}");
        assert_eq!(
            (
                values[0].get("value_sha256"),
                values[0].get("value_bytes"),
                values[0].get("value_json_left_out"),
                values[0].get("value_json"),
            ),
            (
                Some(&json!(sha256)),
                Some(&json!(1024)),
                Some(&json!(true)),
                None
            ),
            "{event_type:?}: {}",
            values[0]
        );
        for value in &values[1..] {
            assert_eq!(value["value_json"], json!(text), "{event_type:?}: {value}");
        }
    }
    scene.close();
}

// Fix round 2 of #1731 (item 5): rewrites a package's control declaration one byte longer (its
// `timeout_ms` 8000 becomes 80000, which nothing these tests read depends on), so the length the
// check's marks read changes for certain and the next check reads the root again; a rewrite with
// the same bytes could leave the marks as they were (a coarse file clock) and pass by doing nothing.
fn one_byte_longer(control: &mut Value) {
    assert_eq!(
        control["timeout_ms"],
        json!(8_000),
        "setup: the fixture's timeout"
    );
    control["timeout_ms"] = json!(80_000);
}

// Item 11: `standard_pack_goals_not_applied`, once for each `goals_failure`, with its cause, and
// once per content. The reader refuses the root's `goals.json` (version 1): one record, `refused`,
// naming the game, caused by the reader's refusal (`standard_pack_root_invalid` with its first
// problem); the root read again with the same file (another declaration file rewritten one byte
// longer, fix round 2) adds none; another refused file adds one. A `goals.json` that cannot be read
// (a directory in its place): one record, `unreadable`, caused by the operating system's failure,
// and none more at the next read (the declaration rewritten one byte longer too).
#[test]
fn v2_fix3_a_goals_json_not_applied_is_recorded_once_per_content_with_its_cause() {
    let goals_path = |packs: &Path| packs.join(GAME_A).join("goals.json");
    let retired = |entries: Value| json!({"schema_version": "actingcommand.goals.v1", "game": GAME_A, "entries": entries});
    let scene = Scene::with(|packs| {
        place_game_root(packs);
        write_json(&goals_path(packs), &retired(json!([])));
    });
    scene.evaluate(POLICY_NOW_UNIX_MS, 7);
    let refused = scene.goals_not_applied();
    assert_eq!(refused.len(), 1, "{refused:#?}");
    assert_eq!(
        (
            refused[0].value("goals_failure").and_then(Value::as_str),
            refused[0].value("game").and_then(Value::as_str),
        ),
        (Some("refused"), Some(GAME_A))
    );
    let cause = refused[0]
        .causes()
        .first()
        .unwrap_or_else(|| panic!("caused by the reader's refusal: {}", refused[0]))
        .outcome();
    assert_eq!(
        cause.code().as_str(),
        "standard_pack_root_invalid",
        "{cause}"
    );
    assert_eq!(
        cause.value("standard_pack_problem").and_then(Value::as_str),
        Some("goal_version_retired"),
        "{cause}"
    );

    // The root read again (a package's declaration rewritten one byte longer, so the check finds
    // its marks changed whatever the file times say), the same file: no second record.
    patch_control(&scene.packs.path().join(GAME_A).join(TASK), one_byte_longer);
    let again_at = POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS;
    scene.evaluate(again_at, 8);
    assert_eq!(scene.goals_not_applied().len(), 1, "once per content");
    // Another refused file: once more.
    write_json(
        &goals_path(scene.packs.path()),
        &retired(json!([{"name": "group_a", "type": "heading"}])),
    );
    scene.evaluate(again_at + CHECK_INTERVAL_MS, 9);
    assert_eq!(scene.goals_not_applied().len(), 2, "another content");
    scene.close();

    let scene = Scene::with(|packs| {
        place_game_root(packs);
        fs::create_dir_all(goals_path(packs)).expect("a directory in the file's place");
    });
    scene.evaluate(POLICY_NOW_UNIX_MS, 7);
    patch_control(&scene.packs.path().join(GAME_A).join(TASK), one_byte_longer);
    scene.evaluate(POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS, 8);
    let unreadable = scene.goals_not_applied();
    assert_eq!(
        unreadable.len(),
        1,
        "once until it reads again: {unreadable:#?}"
    );
    assert_eq!(
        unreadable[0].value("goals_failure").and_then(Value::as_str),
        Some("unreadable")
    );
    assert!(
        !unreadable[0].causes().is_empty(),
        "caused by the operating system's failure: {}",
        unreadable[0]
    );
    scene.close();
}

// Item 12, the two cases `v2_fix2_a_dispatched_package_the_workbench_cannot_read_is_warned_once`
// leaves out. (a) The state of the package's files changes while it stays unreadable (the metadata
// of its resources manifest fails too): the next dispatch after the next read writes
// `goal_value_package_unreadable` once more.
#[test]
fn v2_fix3_a_package_whose_files_change_state_is_warned_once_more() {
    let scene = Scene::new();
    let package = scene.packs.path().join(GAME_A).join(TASK);
    let control = package.join("control.json");
    let manifest = package
        .join("resources")
        .join("operations")
        .join("resources.json");
    metadata_seam().insert(control.clone());
    let (_, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7);
    let first = scene.failed(PACKAGE_UNREAD).len();
    metadata_seam().insert(manifest.clone());
    let again_at = next_eligible(&context).max(POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS);
    scene.evaluate_and_run(again_at, 8);
    metadata_seam().remove(&control);
    metadata_seam().remove(&manifest);
    assert_eq!(first, 1, "the first state");
    let warnings = scene.failed(PACKAGE_UNREAD);
    assert_eq!(warnings.len(), 2, "another state, once more: {warnings:#?}");
    assert_eq!(
        warnings[1].value("package_id").and_then(Value::as_str),
        Some(TASK)
    );
    scene.close();
}

// Item 12 (b): a game root the workbench has not read since the start. After a restart the root's
// `schedule.json` cannot be read at any check (the seam), so the generation in force, restored from
// the ledger, still dispatches the task while no check of this start read its root: its dispatch
// carries no `policy_dispatch_parameters` and one `goal_value_package_unreadable` names the instance,
// the task and the package.
#[test]
fn v2_fix3_a_dispatched_package_of_a_root_not_read_since_the_start_is_warned() {
    let scene = Scene::new();
    let (_, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7);
    assert!(scene.failed(PACKAGE_UNREAD).is_empty());
    let schedule = scene.packs.path().join(GAME_A).join("schedule.json");
    let restart_at = next_eligible(&context).max(POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS);
    metadata_seam().insert(schedule.clone());
    let scene = scene.restart(restart_at);
    let evaluation = scene.evaluate(restart_at, 8);
    metadata_seam().remove(&schedule);
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == INSTANCE)
        .unwrap_or_else(|| panic!("a dispatch of {TASK}: {evaluation:#?}"));
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain");
    assert!(
        chain
            .reasons
            .iter()
            .all(|reason| reason.outcome.code().as_str() != PARAMETERS),
        "{chain:?}"
    );
    let warnings = scene.failed(PACKAGE_UNREAD);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    for (key, expected) in [
        ("instance_id", INSTANCE),
        ("task_id", TASK),
        ("package_id", TASK),
    ] {
        assert_eq!(
            warnings[0].value(key).and_then(Value::as_str),
            Some(expected),
            "{key}: {}",
            warnings[0]
        );
    }
    scene.close();
}
