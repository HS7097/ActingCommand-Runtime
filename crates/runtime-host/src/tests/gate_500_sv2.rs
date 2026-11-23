// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #500 SV-2, host half (0.12.81; register §1.511; contracts/package-variables.md, "The
// run", "A value set on the application decides which rules run"; contracts/scheduling-blackout.md,
// "Order of the deferrals", position 5a; coordinator work order rd-1123/brief-500-sv2.md, its
// rulings 3, 4, 5 and 7 and its tests 1, 3, 4, 7 and 8).
//
// A neutral fixture, built as `gate_463_v2` builds its own. The host runs in composer mode over one
// game root on disk, a manual Runtime clock and one fake instance. The root's one package goes from
// `home` to `result` in one tap and declares the variable `pick` (a `choice` of `item_a`, `item_b`
// and `item_c`, default `item_c`); its `schedule.json` names `rule_variables` and holds two rules
// of that package, both due at every evaluation: `fixture.sv2.b`, first in the file and producing
// two units, which carries `pick` = `item_b`, and `fixture.sv2.a`, producing one, which carries
// `item_a`. So with nothing set the first is the one the evaluation picks, and it is the one the
// closure is asked about first. Values are set and cleared by name (`EditGoal`) on a connection
// bound `lab`. Every expected value is the work order's.
// - `sv2_h1_each_rule_hands_its_run_its_value_and_a_set_value_holds_the_other_back`: nothing set,
//   neither rule is held back and the first runs with `item_b`, source `rule`; `pick` set to
//   `item_c`, which neither carries, both wait `goal_value_excludes_rule` (the instance, the task,
//   `pick`), deferred with no wake and no dispatch; set to `item_a`, the first rule waits so while
//   the second is dispatched and runs with `item_a`, source `rule`, its record naming who set the
//   value; cleared, neither is held back and the first is a candidate again (tests 1, 3, 4).
// - `sv2_h2_the_goal_view_names_a_set_value_and_leaves_the_rules_choice_unnamed`: `GoalView` of
//   `pick` with nothing set names no value, source or display (the rules decide), with `item_a`
//   set the set value as the next dispatch's (test 8).
// - `sv2_h3_a_direct_run_reads_its_request_s_values_and_no_rule_s`: on a physical instance whose
//   display is observed, as `gate_463_s38` runs one, a direct run given `item_a` reads it and one
//   given none reads the default `item_c`, no rule's value (test 7).
// - `sv2_h4_a_rule_value_the_declaration_in_force_refuses_holds_its_rule_back` (fix round 1, the
//   review's required fix): with `item_b` set, the package is replaced in place by one that no
//   longer offers `item_b` while the generation in force stays; the first rule waits
//   `goal_value_rule_refused` with no wake and nothing is dispatched; the package repaired, the
//   first rule is dispatched again and its run reads `item_b`.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{
    DeclaredInitiator, FactContent, FactValue, GoalEdit, GoalEntryState, GoalValueSource, GoalView,
    GoalViewEntry, Initiator, InstanceFactContext, InstanceFactSnapshot, RuntimeChannel,
};
use actingcommand_policy::{
    EligibilityState, PolicyEvaluation, RULE_VARIABLES_SUBSYSTEM, SchedulingDecisionState,
    TaskDecision,
};
use serde_json::{Value, json};
use std::fs;

const PACKAGE: &str = "fixture.sv2";
/// The rule first in the file, producing two units: it carries `pick` = `item_b`.
const RULE_B: &str = "fixture.sv2.b";
/// The rule second in the file, producing one unit: it carries `pick` = `item_a`.
const RULE_A: &str = "fixture.sv2.a";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const PARAMETERS: &str = "policy_dispatch_parameters";
const EXCLUDES: &str = "goal_value_excludes_rule";
const REFUSED: &str = "goal_value_rule_refused";
const PICK_KEY: &str = "session.dispatch.parameter.pick";
/// The default `standard_packs.check_interval_ms`: a check reads the roots again after it.
const CHECK_INTERVAL_MS: u64 = 60_000;

// ---- The game root.

// The variable the package declares.
fn variables() -> Value {
    json!([{"name": "pick", "type": "choice", "title": {"en": "Item"},
            "options": [{"value": "item_a", "title": {"en": "Item A"}},
                        {"value": "item_b", "title": {"en": "Item B"}},
                        {"value": "item_c", "title": {"en": "Item C"}}],
            "default": "item_c"}])
}

// The rule `id` of the package, due at every evaluation, producing `amount` units of pool A and
// carrying `pick` = `pick`.
fn carrying(id: &str, amount: u64, pick: &str) -> Value {
    let mut written = rule(id, PACKAGE, 100, Some(POOL_A));
    written["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    written["loop_budget"]["window_iteration_limit"] = json!(8);
    written["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    written["produces"][0]["amount"] = json!(amount);
    written["variables"] = json!({"pick": pick});
    written
}

// Places the game root below `packs`: its bundle, its `schedule.json` with the two rules, and the
// package, whose control declaration declares [`variables`].
fn place_game_root(packs: &Path) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[PACKAGE]),
    );
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(8);
    }
    write_json(
        &game.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": [RULE_VARIABLES_SUBSYSTEM],
            "rules": [carrying(RULE_B, 2, "item_b"), carrying(RULE_A, 1, "item_a")],
            "pools": [pool(POOL_A)],
            "activity": activity,
            "timeline": [],
            "held": []
        }),
    );
    let package = game.join(PACKAGE);
    write_package(&package, PACKAGE, None, None);
    let control_path = package.join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    control["variables"] = variables();
    write_json(&control_path, &control);
}

// Fix round 1: rewrites the package's control declaration below `packs` in place, `pick` as
// [`variables`] declares it, less the option `item_b` when `offers_item_b` is false.
fn declare(packs: &Path, offers_item_b: bool) {
    let control_path = packs.join(GAME_A).join(PACKAGE).join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    let mut declared = variables();
    if !offers_item_b {
        declared[0]["options"]
            .as_array_mut()
            .expect("the options")
            .retain(|option| option["value"] != "item_b");
    }
    control["variables"] = declared;
    write_json(&control_path, &control);
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
    fn new() -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path());
        let device = Arc::new(FakeState::default());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = RuntimeHost::start(
            config(&state_root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_standard_packs_root(Some(packs.path().to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(
                FakeProvider::one(INSTANCE, instance_id(), Arc::clone(&device))
                    .fixture_simulation(),
            ),
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

    // Admits the dispatch of `task_id` that `evaluation` made, runs it (`home`, one tap, `result`)
    // and settles it: the reason chain the dispatch carried and its run's context.
    fn run(
        &self,
        evaluation: &PolicyEvaluation,
        task_id: &str,
    ) -> (DecisionReasonChain, Box<PolicyRunContext>) {
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
        let request = ContainedTaskRequest::new(
            self.packs
                .path()
                .join(GAME_A)
                .join(PACKAGE)
                .to_string_lossy()
                .into_owned(),
        )
        .expect("contained task request");
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(&intent, &chain, &admission, &request)
            .unwrap_or_else(|error| panic!("admit {task_id}: {error}"))
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
            .run_scheduled_contained_task(&context, &request)
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

    // `EditGoal` of `op`, on a connection bound `lab` with `Lab-CLI` and the Lab's envelope, the
    // Runtime clock at `at` first. It is taken.
    fn edit(&self, op: GoalEdit, at: u64) {
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
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "the edit is taken: {receipt:?}"
        );
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
// read (`None` when it holds none): what the run's snapshots hold.
fn overlaid_pick(host: &RuntimeHost) -> Option<FactValue> {
    let runs = host
        .run_parameters_for_test()
        .expect("the parameters of the host's runs");
    let parameters = runs.last().expect("a run read its parameters");
    let mut snapshot = InstanceFactSnapshot {
        snapshot_id: "snapshot-sv2-host".to_owned(),
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

// The string fact `word`.
fn text(word: &str) -> Option<FactValue> {
    Some(FactValue::String(word.to_owned()))
}

// The decision `evaluation` made of `task_id` on the instance.
fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|made| made.task_id == task_id && made.instance_id.as_deref() == Some(INSTANCE))
        .unwrap_or_else(|| panic!("the decision of {task_id}: {evaluation:#?}"))
}

// Whether a reason of `task_id`'s decision is that a set value excludes its rule.
fn excluded(evaluation: &PolicyEvaluation, task_id: &str) -> bool {
    decision(evaluation, task_id)
        .reasons
        .iter()
        .any(|reason| reason.outcome.code().as_str() == EXCLUDES)
}

// Whether `evaluation` dispatched `task_id` on the instance.
fn dispatched(evaluation: &PolicyEvaluation, task_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id && intent.instance_id == INSTANCE)
}

// The page's entry of `name`.
fn listed<'a>(view: &'a GoalView, name: &str) -> &'a GoalViewEntry {
    view.entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("the entry {name}: {view:#?}"))
}

// Tests 1, 3 and 4 (rulings 3 and 4): with nothing set both rules are candidates and the first is
// dispatched; its run reads its rule's `item_b`, which its record says with the source `rule`. With
// `pick` set to `item_c`, which neither rule carries, each rule's candidate waits
// `goal_value_excludes_rule` naming the instance, its task and `pick`, deferred with no wake of its
// own and no dispatch (so no admission and no budget). With `item_a` set the first rule waits so,
// while the second rule, which carries the set value, is dispatched and its run reads `item_a`,
// its record with the source `rule` and who set the application's value. Cleared, neither rule is
// held back at the next evaluation: the first is a candidate again, and one of them is dispatched.
#[test]
fn sv2_h1_each_rule_hands_its_run_its_value_and_a_set_value_holds_the_other_back() {
    let scene = Scene::new();
    let first = scene.evaluate(POLICY_NOW_UNIX_MS, 6);
    assert!(!excluded(&first, RULE_B) && !excluded(&first, RULE_A));
    let (chain, context) = scene.run(&first, RULE_B);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "rule",
                "value_json": "\"item_b\""}])
    );
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_b"))]);
    assert_eq!(overlaid_pick(&scene.host), text("item_b"));

    // A value neither rule carries holds both back, each with the reason and no wake.
    let none_at = next_eligible(&context);
    scene.edit(set_pick("item_c"), none_at);
    let neither = scene.evaluate(none_at, 7);
    for task_id in [RULE_B, RULE_A] {
        held_back(&neither, task_id);
    }

    let set_at = none_at + 60_000;
    scene.edit(set_pick("item_a"), set_at);
    let second = scene.evaluate(set_at, 8);
    held_back(&second, RULE_B);
    assert!(!excluded(&second, RULE_A));
    let (chain, context) = scene.run(&second, RULE_A);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "rule",
                "value_json": "\"item_a\"", "set_unix_ms": set_at, "set_channel": "lab"}])
    );
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_a"))]);
    assert_eq!(overlaid_pick(&scene.host), text("item_a"));

    let clear_at = next_eligible(&context);
    scene.edit(
        GoalEdit::Clear {
            name: "pick".to_owned(),
        },
        clear_at,
    );
    let third = scene.evaluate(clear_at, 9);
    assert!(!excluded(&third, RULE_B) && !excluded(&third, RULE_A));
    assert_eq!(
        decision(&third, RULE_B).eligibility,
        EligibilityState::True,
        "a candidate again"
    );
    assert!(
        dispatched(&third, RULE_B) || dispatched(&third, RULE_A),
        "the evaluation picks among them: {:#?}",
        third.dispatch_intents
    );
    scene.close();
}

// The decision of `task_id` in `evaluation` is held back by the value set on the application:
// deferred with `goal_value_excludes_rule` naming the instance, the task and `pick`, no wake of its
// own and no dispatch (so no admission and no budget).
#[track_caller]
fn held_back(evaluation: &PolicyEvaluation, task_id: &str) {
    let held = decision(evaluation, task_id);
    assert_eq!(held.state, SchedulingDecisionState::Deferred, "{held:#?}");
    assert_eq!(held.next_wake_unix_ms, None, "no wake of its own");
    let reason = held.reasons.last().expect("the reason that holds it");
    assert_eq!(reason.outcome.code().as_str(), EXCLUDES, "{held:#?}");
    for (key, expected) in [
        ("instance_id", INSTANCE),
        ("task_id", task_id),
        ("variable_name", "pick"),
    ] {
        assert_eq!(
            reason.outcome.value(key).and_then(Value::as_str),
            Some(expected),
            "{key}: {}",
            reason.outcome
        );
    }
    assert!(
        !dispatched(evaluation, task_id),
        "a held rule takes no dispatch"
    );
}

// Test 8 (ruling 7): of `pick`, which both bound rules carry a value for, the goal page names no
// value, source or display while nothing is set, since the rule of the task that runs decides, and
// says so as in effect with the default it would fall back to; with `item_a` set, every dispatch
// hands `item_a` (the rule that carries another is held back), and the page names it as set, as
// before.
#[test]
fn sv2_h2_the_goal_view_names_a_set_value_and_leaves_the_rules_choice_unnamed() {
    let scene = Scene::new();
    let _first = scene.evaluate(POLICY_NOW_UNIX_MS, 6);
    let view = scene.view();
    let pick = listed(&view, "pick");
    assert_eq!(pick.packages, [PACKAGE]);
    assert_eq!(
        (
            pick.default_json.as_deref(),
            pick.value_json.as_deref(),
            pick.effective_json.as_deref(),
            pick.source,
            pick.display.is_some(),
            pick.state
        ),
        (
            Some("\"item_c\""),
            None,
            None,
            None,
            false,
            Some(GoalEntryState::InEffect)
        )
    );

    let set_at = POLICY_NOW_UNIX_MS + 60_000;
    scene.edit(set_pick("item_a"), set_at);
    let view = scene.view();
    let pick = listed(&view, "pick");
    assert_eq!(
        (
            pick.value_json.as_deref(),
            pick.effective_json.as_deref(),
            pick.source,
            pick.state
        ),
        (
            Some("\"item_a\""),
            Some("\"item_a\""),
            Some(GoalValueSource::Set),
            Some(GoalEntryState::InEffect)
        )
    );
    scene.close();
}

// ---- A direct run (test 7), on an instance whose display is observed, as `gate_463_s38` runs one.

struct DirectScene {
    _state_root: TempDir,
    packs: TempDir,
    device: Arc<FakeState>,
    host: RuntimeHost,
}

impl DirectScene {
    // The host over the game root, after one evaluation: the workbench composed and judged it.
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
        // The workbench checks the game root at the start of a policy cycle; what the cycle
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
                .join(PACKAGE)
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
        let started = client.request(RuntimeOperation::run_contained_task(
            INSTANCE,
            client.ids.mint_holder_id().expect("holder"),
            request,
        ));
        let receipt = client.send(&started);
        drop(client);
        receipt
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

// Test 7 (ruling 3, must not change): the root's rules carry `item_a` and `item_b`; a direct run of
// their package given `item_a` reads `item_a`, and one given no value reads the default `item_c`,
// as before: a direct run reads its request's values and the package's defaults, never a rule's.
#[test]
fn sv2_h3_a_direct_run_reads_its_request_s_values_and_no_rule_s() {
    let scene = DirectScene::new();
    let given = scene.run(&[("pick", json!("item_a"))]);
    assert_eq!(given.state(), RuntimeReceiptState::Completed, "{given:?}");
    assert_eq!(overlaid_pick(&scene.host), text("item_a"));
    let none = scene.run(&[]);
    assert_eq!(none.state(), RuntimeReceiptState::Completed, "{none:?}");
    assert_eq!(
        overlaid_pick(&scene.host),
        text("item_c"),
        "the default, no rule's value"
    );
    scene.close();
}

// Fix round 1 (the review's required fix; package-variables.md, "Carried by the rule"): after the
// first rule's run, `item_b` is set, and the package's declaration is replaced in place by one
// that no longer offers `item_b`; the check that reads it keeps the generation in force (the gate
// tests' seam `REFUSALS_NAME_UNCOMPOSED_FOR_TEST`, for this test's own root, takes the composer's
// refusal of the file for one no round composed). The first rule's candidate, whose rule carries
// `item_b`, waits `goal_value_rule_refused`, a warning naming the instance, its task, the package,
// `pick` and `not_an_option`, deferred with no wake of its own; the second waits for the set value
// as before; neither is dispatched. The package repaired, the first rule is
// dispatched again at the next check (the second still waits for the set value) and its run reads
// `item_b`, source `rule`, with who set the application's value.
#[test]
fn sv2_h4_a_rule_value_the_declaration_in_force_refuses_holds_its_rule_back() {
    let scene = Scene::new();
    let root = scene.packs.path().to_path_buf();
    let seam = || {
        crate::host::REFUSALS_NAME_UNCOMPOSED_FOR_TEST
            .lock()
            .expect("the refusal seam")
    };
    let first = scene.evaluate(POLICY_NOW_UNIX_MS, 6);
    let (_, context) = scene.run(&first, RULE_B);
    let set_at = next_eligible(&context);
    scene.edit(set_pick("item_b"), set_at);

    declare(scene.packs.path(), false);
    seam().insert(root.clone());
    let refused_at = set_at + CHECK_INTERVAL_MS;
    let refused = scene.evaluate(refused_at, 7);
    seam().remove(&root);
    let held = decision(&refused, RULE_B);
    assert_eq!(held.state, SchedulingDecisionState::Deferred, "{held:#?}");
    assert_eq!(held.next_wake_unix_ms, None, "no wake of its own");
    let warning = held
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == REFUSED)
        .unwrap_or_else(|| panic!("the warning: {held:#?}"));
    assert_eq!(warning.outcome.category(), Category::Warning);
    for (key, expected) in [
        ("instance_id", INSTANCE),
        ("task_id", RULE_B),
        ("package_id", PACKAGE),
        ("variable_name", "pick"),
        ("variable_problem", "not_an_option"),
    ] {
        assert_eq!(
            warning.outcome.value(key).and_then(Value::as_str),
            Some(expected),
            "{key}: {}",
            warning.outcome
        );
    }
    let waits = decision(&refused, RULE_A);
    assert_eq!(waits.state, SchedulingDecisionState::Deferred, "{waits:#?}");
    assert!(excluded(&refused, RULE_A), "{waits:#?}");
    assert!(
        !dispatched(&refused, RULE_B) && !dispatched(&refused, RULE_A),
        "neither rule is dispatched: {:#?}",
        refused.dispatch_intents
    );

    declare(scene.packs.path(), true);
    let repaired = scene.evaluate(refused_at + CHECK_INTERVAL_MS, 8);
    assert!(
        decision(&repaired, RULE_B)
            .reasons
            .iter()
            .all(|reason| reason.outcome.code().as_str() != REFUSED),
        "{:#?}",
        decision(&repaired, RULE_B)
    );
    assert!(excluded(&repaired, RULE_A));
    let (chain, context) = scene.run(&repaired, RULE_B);
    assert_eq!(
        variable_values(&chain),
        json!([{"variable_name": "pick", "variable_source": "rule",
                "value_json": "\"item_b\"", "set_unix_ms": set_at, "set_channel": "lab"}])
    );
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_b"))]);
    scene.close();
}
