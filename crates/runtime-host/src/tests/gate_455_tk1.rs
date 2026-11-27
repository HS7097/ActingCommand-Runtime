// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 TK-1, host half (0.12.89; register §1.532; contracts/package-variables.md, "A
// variable a rule gives"; contracts/scheduling-blackout.md, "Order of the deferrals", position 5a;
// coordinator work order rd-1123/brief-455-tk1.md, its rulings 2, 4, 5, 6 and 7 and its tests 3,
// 4 and 5), written as `gate_500_sv2` is.
//
// A neutral fixture: the host in composer mode over one game root on disk, a manual Runtime clock
// and one fake instance. The root's one package goes from `home` to `result` in one tap; its
// `schedule.json` holds the rules each test names, `fixture.tk1.b` first in the file and producing
// two units, so that it is the one the evaluation picks, and `fixture.tk1.a` second. The package
// declares `pick` (a `choice` of `item_a`, `item_b` and `item_c`), a person's variable with the
// default `item_c`, or one only a rule gives (`given_by: "rule"`, no default), as each step says;
// the package is converted in place, and the workbench reads it again at its next check. Values are
// set and cleared by name (`EditGoal`) on a connection bound `lab`.
// - `tk1_h1_...`: both rules carry `pick`; a value stored while `pick` was a person's variable
//   holds neither back once it only a rule gives: the first runs with its own `item_b`, source
//   `rule`, no setter; the page lists no `pick` and shows the stored value as an orphan; `goals set`
//   is refused `rule_given`, `goals clear` removes the orphan, a second clear answers `rule_given`.
// - `tk1_h2_...`: the first rule carries nothing, the second `item_a`; `pick` converted while the
//   generation in force stays: the first waits `goal_value_rule_missing` (a warning: the instance,
//   the task, the package, `pick`), deferred with no wake of its own and not dispatched, while the
//   second runs with `item_a`; the package mended, the first is dispatched again.
// - `tk1_h3_...`: the same hold lifted by the rule: `schedule.json` rewritten so that the first
//   rule carries `item_b`, it is dispatched and its run reads `item_b`, source `rule`.
// - `tk1_h4_...`: a direct run of the converted package given no value is Denied
//   `run_variable_missing` (the package, `pick`) with nothing pressed; given `item_a` it runs, reads
//   it, and its admission record lists it.
// - `tk1_h5_...`: the package converted between a dispatch and its run: the kernel refuses the run
//   with `contained_task_variable_value_missing` before any input; the next evaluation after the
//   workbench's check holds the pair `goal_value_rule_missing` and dispatches it no more.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{
    DeclaredInitiator, FactContent, FactValue, GoalEdit, GoalView, Initiator, InstanceFactContext,
    InstanceFactSnapshot, RuntimeChannel,
};
use actingcommand_policy::{
    PolicyEvaluation, RULE_VARIABLES_SUBSYSTEM, SchedulingDecisionState, TaskDecision,
};
use serde_json::{Value, json};
use std::fs;

const PACKAGE: &str = "fixture.tk1";
/// The rule first in the file, producing two units.
const RULE_B: &str = "fixture.tk1.b";
/// The rule second in the file, producing one unit.
const RULE_A: &str = "fixture.tk1.a";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const PARAMETERS: &str = "policy_dispatch_parameters";
const EXCLUDES: &str = "goal_value_excludes_rule";
const MISSING: &str = "goal_value_rule_missing";
const KERNEL_MISSING: &str = "contained_task_variable_value_missing";
const PICK_KEY: &str = "session.dispatch.parameter.pick";
/// The default `standard_packs.check_interval_ms`: a check reads the roots again after it.
const CHECK_INTERVAL_MS: u64 = 60_000;

// ---- The game root.

// The variable `pick`: only a rule gives it when `rule_given` (no default), else a person's
// variable with the default `item_c`.
fn pick(rule_given: bool) -> Value {
    let mut declared = json!({"name": "pick", "type": "choice", "title": {"en": "Item"},
                              "options": [{"value": "item_a", "title": {"en": "Item A"}},
                                          {"value": "item_b", "title": {"en": "Item B"}},
                                          {"value": "item_c", "title": {"en": "Item C"}}]});
    if rule_given {
        declared["given_by"] = json!("rule");
    } else {
        declared["default"] = json!("item_c");
    }
    declared
}

// The rule `id` of the package, due at every evaluation, producing `amount` units of pool A and
// carrying `pick` = `carried` when given.
fn carrying(id: &str, amount: u64, carried: Option<&str>) -> Value {
    let mut written = rule(id, PACKAGE, 100, Some(POOL_A));
    written["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    written["loop_budget"]["window_iteration_limit"] = json!(8);
    written["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    written["produces"][0]["amount"] = json!(amount);
    if let Some(carried) = carried {
        written["variables"] = json!({"pick": carried});
    }
    written
}

// Writes the root's `schedule.json` below `packs` with `rules`, naming `rule_variables` when one
// of them carries the member.
fn write_schedule(packs: &Path, rules: Vec<Value>) {
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(8);
    }
    let requires = if rules
        .iter()
        .any(|written| written.get("variables").is_some())
    {
        json!([RULE_VARIABLES_SUBSYSTEM])
    } else {
        json!([])
    };
    write_json(
        &packs.join(GAME_A).join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": requires,
            "rules": rules,
            "pools": [pool(POOL_A)],
            "activity": activity,
            "timeline": [],
            "held": []
        }),
    );
}

// Rewrites the package's control declaration below `packs` in place: its `variables` are
// `declared`, or it declares none.
fn declare(packs: &Path, declared: Option<Value>) {
    let control_path = packs.join(GAME_A).join(PACKAGE).join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    if let Some(declared) = declared {
        control["variables"] = declared;
    } else if let Some(object) = control.as_object_mut() {
        object.remove("variables");
    }
    write_json(&control_path, &control);
}

// Places the game root below `packs`: its bundle, its `schedule.json` with `rules`, and the
// package, whose control declaration declares `declared`.
fn place_game_root(packs: &Path, rules: Vec<Value>, declared: Option<Value>) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[PACKAGE]),
    );
    write_schedule(packs, rules);
    write_package(&game.join(PACKAGE), PACKAGE, None, None);
    declare(packs, declared);
}

// The fixture's facts: the one instance bound to game A on server A, able to run the Runtime's
// package operation, and no recorded outcome.
fn facts() -> EvaluationFacts {
    let mut bound = policy_facts();
    bound.outcomes.clear();
    bound.instances = vec![InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: SERVER_A.to_owned(),
        game_id: GAME_A.to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec![actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned()],
        preferred_task_ids: Vec::new(),
    }];
    bound
}

// ---- The fixture.

struct Scene {
    _state_root: TempDir,
    packs: TempDir,
    device: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

impl Scene {
    // The host over a root of `rules` whose package declares `declared`. With `physical` the
    // instance's display is observed, as a direct run needs (Workflow #392 DA-A); otherwise it is
    // a fixture simulation, as a scheduled run's.
    fn new(rules: Vec<Value>, declared: Option<Value>, physical: bool) -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path(), rules, declared);
        let device = Arc::new(FakeState::default());
        let provider = if physical {
            device.physical_task_geometry.store(true, Ordering::Release);
            *device
                .physical_task_extent
                .lock()
                .expect("fake physical task extent") = Some((2, 1));
            FakeProvider::one(INSTANCE, instance_id(), Arc::clone(&device))
        } else {
            FakeProvider::one(INSTANCE, instance_id(), Arc::clone(&device)).fixture_simulation()
        };
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = RuntimeHost::start(
            config(&state_root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_standard_packs_root(Some(packs.path().to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(provider),
        )
        .expect("start the composer-mode host");
        Self {
            _state_root: state_root,
            packs,
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

    // The package's directory, as a run's request names it.
    fn package(&self) -> String {
        self.packs
            .path()
            .join(GAME_A)
            .join(PACKAGE)
            .to_string_lossy()
            .into_owned()
    }

    // The device shows `home` until the run's tap, then `result`.
    fn script_device(&self) {
        let taps = self.device.input_count.load(Ordering::Acquire);
        *self.device.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                vec![(0, if inputs <= taps { HOME } else { RESULT })]
            }));
    }

    // The inputs the device has taken.
    fn taps(&self) -> usize {
        self.device.input_count.load(Ordering::Acquire)
    }

    // Admits the dispatch of `task_id` that `evaluation` made: the reason chain it carried, its
    // run's context and the run's request; the device scripted for the run.
    fn admit(
        &self,
        evaluation: &PolicyEvaluation,
        task_id: &str,
    ) -> (
        DecisionReasonChain,
        Box<PolicyRunContext>,
        ContainedTaskRequest,
    ) {
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id && intent.instance_id == INSTANCE)
            .unwrap_or_else(|| panic!("a dispatch of {task_id}: {evaluation:#?}"))
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
        let request = ContainedTaskRequest::new(self.package()).expect("contained task request");
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(&intent, &chain, &admission, &request)
            .unwrap_or_else(|error| panic!("admit {task_id}: {error}"))
        else {
            panic!("expected one policy run context")
        };
        self.script_device();
        (chain, context, request)
    }

    // Admits the dispatch of `task_id` that `evaluation` made, runs it and settles it: the reason
    // chain the dispatch carried, its run's context and the run's receipt.
    fn run(
        &self,
        evaluation: &PolicyEvaluation,
        task_id: &str,
    ) -> (DecisionReasonChain, Box<PolicyRunContext>, RuntimeReceipt) {
        let (chain, context, request) = self.admit(evaluation, task_id);
        let receipt = self
            .host
            .run_scheduled_contained_task(&context, &request)
            .expect("the scheduled run");
        self.host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        (chain, context, receipt)
    }

    // Admits the dispatch of `task_id` that `evaluation` made and runs it as a scheduled run that
    // fails without poisoning the Runtime (its live settlement settles it): the run's context.
    fn run_failing(&self, evaluation: &PolicyEvaluation, task_id: &str) -> Box<PolicyRunContext> {
        let (_, context, request) = self.admit(evaluation, task_id);
        let error = self
            .host
            .run_scheduled_contained_task(&context, &request)
            .expect_err("the scheduled run fails");
        assert!(!error.is_fatal(), "{error:?}");
        context
    }

    // One direct run of the package with the values `given`, on a connection of its own.
    fn direct(&self, given: &[(&str, Value)]) -> RuntimeReceipt {
        self.script_device();
        self.sample_capacity();
        let request = ContainedTaskRequest::new(self.package())
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
        let started = client.request(RuntimeOperation::run_contained_task(
            INSTANCE,
            client.ids.mint_holder_id().expect("holder"),
            request,
        ));
        let receipt = client.send(&started);
        drop(client);
        receipt
    }

    // `EditGoal` of `op`, on a connection bound `lab` with `Lab-CLI` and the Lab's envelope, the
    // Runtime clock at `at` first: its receipt.
    fn edit(&self, op: GoalEdit, at: u64) -> RuntimeReceipt {
        advance_manual_clock_to(&self.clock, at);
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator: Some(DeclaredInitiator::from(Initiator::LabCli)),
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
        let request = RuntimeRequest::new(
            client.ids.mint_request_id().expect("request id"),
            client.ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::Lab,
            EventSource::Lab,
            1,
            RuntimeOperation::EditGoal {
                instance_alias: INSTANCE.to_owned(),
                expected_version: None,
                op,
            },
        )
        .expect("lab runtime request");
        client.send(&request)
    }

    // The instance's goal page, read on a connection of the CLI.
    fn view(&self) -> GoalView {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::GoalView {
            instance_alias: INSTANCE.to_owned(),
        });
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        let Some(RuntimeResult::GoalViewed { view }) = receipt.result() else {
            panic!("a goal view: {receipt:?}");
        };
        (**view).clone()
    }

    // The payload of the last admission record (`task.requested`) of the host's ledger.
    fn last_admission(&self) -> EventPayload {
        let mut events = self
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::TaskRequested),
                ..EventQuery::default()
            })
            .expect("the ledger's admission records");
        events.sort_by_key(PersistedEvent::sequence);
        events
            .last()
            .map(|event| event.payload().clone())
            .expect("an admission record")
    }

    // The outcomes of the failed terminals of the host's ledger, in order.
    fn failed_terminals(&self) -> Vec<String> {
        let mut events = super::instance_workers::all_events(&self.host);
        events.sort_by_key(PersistedEvent::sequence);
        events
            .iter()
            .filter(|event| event.event_type() == EventType::TaskFailed)
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted {
                        outcome: Some(outcome),
                        ..
                    } => Some(outcome.code().as_str().to_owned()),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

fn set_pick(value: &str) -> GoalEdit {
    GoalEdit::Set {
        name: "pick".to_owned(),
        value_json: json!(value).to_string(),
    }
}

fn clear_pick() -> GoalEdit {
    GoalEdit::Clear {
        name: "pick".to_owned(),
    }
}

// The receipt is Completed.
#[track_caller]
fn completed(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

// The edit is refused `goal_value_invalid` with `goal_problem` `rule_given`.
#[track_caller]
fn rule_given_refusal(receipt: &RuntimeReceipt) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        "goal_value_invalid",
        "{receipt:?}"
    );
    assert_eq!(
        receipt.outcome().value("goal_problem"),
        Some(&json!("rule_given")),
        "{receipt:?}"
    );
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

// The value of `session.dispatch.parameter.pick` in a snapshot overlaid with what `host`'s last run
// read (`None` when it holds none).
fn overlaid_pick(host: &RuntimeHost) -> Option<FactValue> {
    let runs = host
        .run_parameters_for_test()
        .expect("the parameters of the host's runs");
    let parameters = runs.last().expect("a run read its parameters");
    let mut snapshot = InstanceFactSnapshot {
        snapshot_id: "snapshot-tk1-host".to_owned(),
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

// The decision `evaluation` made of `task_id` on the instance.
fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|made| made.task_id == task_id && made.instance_id.as_deref() == Some(INSTANCE))
        .unwrap_or_else(|| panic!("the decision of {task_id}: {evaluation:#?}"))
}

// Whether a reason of `task_id`'s decision has the code `code`.
fn held_by(evaluation: &PolicyEvaluation, task_id: &str, code: &str) -> bool {
    decision(evaluation, task_id)
        .reasons
        .iter()
        .any(|reason| reason.outcome.code().as_str() == code)
}

// Whether `evaluation` dispatched `task_id` on the instance.
fn dispatched(evaluation: &PolicyEvaluation, task_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id && intent.instance_id == INSTANCE)
}

// The decision of `task_id` in `evaluation` is held back by `goal_value_rule_missing`: deferred, a
// warning naming the instance, the task, the package and `pick`, no wake of its own (the mark the
// idle plan reads as a hold with no time) and no dispatch.
#[track_caller]
fn missing_hold(evaluation: &PolicyEvaluation, task_id: &str) {
    let held = decision(evaluation, task_id);
    assert_eq!(held.state, SchedulingDecisionState::Deferred, "{held:#?}");
    assert_eq!(held.next_wake_unix_ms, None, "no wake of its own");
    let warning = held
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == MISSING)
        .unwrap_or_else(|| panic!("the hold: {held:#?}"));
    assert_eq!(warning.outcome.category(), Category::Warning);
    for (key, expected) in [
        ("instance_id", INSTANCE),
        ("task_id", task_id),
        ("package_id", PACKAGE),
        ("variable_name", "pick"),
    ] {
        assert_eq!(
            warning.outcome.value(key).and_then(Value::as_str),
            Some(expected),
            "{key}: {}",
            warning.outcome
        );
    }
    assert!(
        !dispatched(evaluation, task_id),
        "a held rule takes no dispatch"
    );
}

// The gate tests' seam of the workbench (as `gate_500_sv2`'s fix round 1 takes it): a refusal of
// this root's `schedule.json` keeps the generation in force, as for a root no round composed.
fn seam() -> std::sync::MutexGuard<'static, BTreeSet<std::path::PathBuf>> {
    crate::host::REFUSALS_NAME_UNCOMPOSED_FOR_TEST
        .lock()
        .expect("the refusal seam")
}

// Test 3, rulings 2 and 7: both rules carry `pick`. While it is a person's variable `item_a` is
// stored; then the package is converted in place, `pick` only a rule gives. At the next check
// neither rule is held back by the stored value (nor by anything of this slice), the first is
// dispatched and its run reads its own `item_b`, source `rule`, no setter named. The page lists no
// `pick`; the stored value is an orphan. `goals set` of `pick` is refused `rule_given`; `goals
// clear` removes the orphan; a second clear, nothing stored, answers `rule_given`.
#[test]
fn tk1_h1_an_orphan_stored_before_the_conversion_blocks_no_rule_and_clear_removes_it() {
    let scene = Scene::new(
        vec![
            carrying(RULE_B, 2, Some("item_b")),
            carrying(RULE_A, 1, Some("item_a")),
        ],
        Some(json!([pick(false)])),
        false,
    );
    let first = scene.evaluate(POLICY_NOW_UNIX_MS, 6);
    let (_, context, receipt) = scene.run(&first, RULE_B);
    completed(&receipt);
    let set_at = next_eligible(&context);
    completed(&scene.edit(set_pick("item_a"), set_at));

    declare(scene.packs.path(), Some(json!([pick(true)])));
    let converted = scene.evaluate(set_at + CHECK_INTERVAL_MS, 7);
    for task_id in [RULE_B, RULE_A] {
        assert!(
            !held_by(&converted, task_id, EXCLUDES) && !held_by(&converted, task_id, MISSING),
            "{task_id}: {:#?}",
            decision(&converted, task_id)
        );
    }
    let (chain, context, receipt) = scene.run(&converted, RULE_B);
    completed(&receipt);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "rule",
                "value_json": "\"item_b\""}])
    );
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_b"))]);
    assert_eq!(
        overlaid_pick(&scene.host),
        Some(FactValue::String("item_b".to_owned()))
    );

    let view = scene.view();
    assert!(
        view.entries.iter().all(|entry| entry.name != "pick"),
        "{view:#?}"
    );
    assert_eq!(
        view.orphans
            .iter()
            .map(|orphan| (orphan.name.as_str(), orphan.value_json.as_str()))
            .collect::<Vec<_>>(),
        [("pick", "\"item_a\"")]
    );

    let edit_at = next_eligible(&context);
    rule_given_refusal(&scene.edit(set_pick("item_b"), edit_at));
    completed(&scene.edit(clear_pick(), edit_at));
    assert!(scene.view().orphans.is_empty());
    rule_given_refusal(&scene.edit(clear_pick(), edit_at));
    scene.close();
}

// The root of tests 4: the first rule carries nothing, the second `item_a`, `pick` a person's
// variable. The first runs once (on the default); then `pick` is converted in place while the
// generation in force stays (the seam). Answers the scene, the evaluation of the next check and
// its time.
fn held_after_conversion() -> (Scene, PolicyEvaluation, u64) {
    let scene = Scene::new(
        vec![
            carrying(RULE_B, 2, None),
            carrying(RULE_A, 1, Some("item_a")),
        ],
        Some(json!([pick(false)])),
        false,
    );
    let first = scene.evaluate(POLICY_NOW_UNIX_MS, 6);
    let (_, context, receipt) = scene.run(&first, RULE_B);
    completed(&receipt);
    declare(scene.packs.path(), Some(json!([pick(true)])));
    let root = scene.packs.path().to_path_buf();
    seam().insert(root.clone());
    let held_at = next_eligible(&context).max(POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS);
    let held = scene.evaluate(held_at, 7);
    seam().remove(&root);
    (scene, held, held_at)
}

// Test 4, ruling 4, the package mended: the first rule, which carries no `pick`, waits
// `goal_value_rule_missing` with no wake and no dispatch, and the second, which carries `item_a`,
// is dispatched and its run reads `item_a`. The package's `pick` a person's variable again, the
// next check lifts the hold and the first rule is dispatched.
#[test]
fn tk1_h2_a_rule_without_the_value_is_held_and_the_mended_package_lifts_it() {
    let (scene, held, held_at) = held_after_conversion();
    missing_hold(&held, RULE_B);
    assert!(
        !held_by(&held, RULE_A, MISSING),
        "{:#?}",
        decision(&held, RULE_A)
    );
    let (_, context, receipt) = scene.run(&held, RULE_A);
    completed(&receipt);
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_a"))]);

    declare(scene.packs.path(), Some(json!([pick(false)])));
    let mended = scene.evaluate(next_eligible(&context).max(held_at + CHECK_INTERVAL_MS), 8);
    assert!(
        !held_by(&mended, RULE_B, MISSING),
        "{:#?}",
        decision(&mended, RULE_B)
    );
    assert!(
        dispatched(&mended, RULE_B),
        "the first rule is dispatched again: {:#?}",
        mended.dispatch_intents
    );
    scene.close();
}

// Test 4, ruling 4, the rule mended: the same hold; `schedule.json` rewritten so that the first
// rule carries `item_b`; the next check composes it, the hold lifts, the first rule is dispatched
// and its run reads `item_b`, source `rule`.
#[test]
fn tk1_h3_the_hold_lifts_when_the_rule_carries_the_value() {
    let (scene, held, held_at) = held_after_conversion();
    missing_hold(&held, RULE_B);
    let (_, context, receipt) = scene.run(&held, RULE_A);
    completed(&receipt);

    write_schedule(
        scene.packs.path(),
        vec![
            carrying(RULE_B, 2, Some("item_b")),
            carrying(RULE_A, 1, Some("item_a")),
        ],
    );
    let mended = scene.evaluate(next_eligible(&context).max(held_at + CHECK_INTERVAL_MS), 8);
    assert!(
        !held_by(&mended, RULE_B, MISSING),
        "{:#?}",
        decision(&mended, RULE_B)
    );
    let (chain, context, receipt) = scene.run(&mended, RULE_B);
    completed(&receipt);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "rule",
                "value_json": "\"item_b\""}])
    );
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_b"))]);
    scene.close();
}

// Test 5's direct run, ruling 6: on an instance whose display is observed, with `pick` only a rule
// gives, a direct run given no value is Denied `run_variable_missing` naming the package and
// `pick`, nothing pressed and no admission recorded; given `item_a` it completes, reads `item_a`,
// and its admission record lists `item_a` under `pick`, the value its request handed it.
#[test]
fn tk1_h4_a_direct_run_without_the_value_is_refused_and_one_with_it_records_it() {
    let scene = Scene::new(
        vec![
            carrying(RULE_B, 2, Some("item_b")),
            carrying(RULE_A, 1, Some("item_a")),
        ],
        Some(json!([pick(true)])),
        true,
    );
    // The workbench checks the game root at the start of a policy cycle; what the cycle
    // dispatches is not this test's.
    let _first = scene.evaluate(POLICY_NOW_UNIX_MS, 7);
    let taps = scene.taps();
    let refused = scene.direct(&[]);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    let outcome = refused.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "run_variable_missing",
        "{refused:?}"
    );
    assert_eq!(outcome.value("package_id"), Some(&json!(PACKAGE)));
    assert_eq!(outcome.value("variable_name"), Some(&json!("pick")));
    assert_eq!(scene.taps(), taps, "nothing was pressed");

    let given = scene.direct(&[("pick", json!("item_a"))]);
    completed(&given);
    assert_eq!(
        overlaid_pick(&scene.host),
        Some(FactValue::String("item_a".to_owned()))
    );
    let admission = scene.last_admission();
    let EventPayload::Task(TaskPayload::Semantic(semantic)) = &admission else {
        panic!("an admission record: {admission:?}");
    };
    let TaskSemanticFact::PackageAdmitted { variables, .. } = semantic.fact() else {
        panic!("a package admission: {admission:?}");
    };
    assert_eq!(
        serde_json::to_value(variables).expect("the values encode"),
        json!([{"variable_name": "pick", "value_json": "\"item_a\""}])
    );
    scene.close();
}

// Test 5's kernel half, ruling 5: the package declares no variable and its one rule carries none;
// between the rule's dispatch and its run the package is converted to declare `pick`, only a rule
// gives. The run fails with `contained_task_variable_value_missing` before any input (nothing
// pressed). The next evaluation after the workbench's check (the generation in force kept) holds
// the pair `goal_value_rule_missing` and dispatches it no more: no rerun of the refused run.
#[test]
fn tk1_h5_a_run_without_the_value_is_refused_before_any_input_and_its_pair_is_held() {
    let scene = Scene::new(vec![carrying(RULE_B, 2, None)], None, false);
    let first = scene.evaluate(POLICY_NOW_UNIX_MS, 6);
    declare(scene.packs.path(), Some(json!([pick(true)])));
    let taps = scene.taps();
    let context = scene.run_failing(&first, RULE_B);
    assert_eq!(scene.taps(), taps, "the run pressed nothing");
    assert_eq!(
        scene.failed_terminals().last().map(String::as_str),
        Some(KERNEL_MISSING)
    );

    let root = scene.packs.path().to_path_buf();
    seam().insert(root.clone());
    let held = scene.evaluate(
        next_eligible(&context).max(POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS),
        7,
    );
    seam().remove(&root);
    missing_hold(&held, RULE_B);
    scene.close();
}
