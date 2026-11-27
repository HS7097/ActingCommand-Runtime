// SPDX-License-Identifier: AGPL-3.0-only

// #503 GOAL-1, the host's half (0.12.88; register §1.531; contracts/resource-targets.md, "A
// target's pool" and "One value by name"; contracts/runtime-interface.md, "Goal request kinds"): on
// a game with two instances a goal names its pool as the game root's files write it, and no value
// of a request stops the Runtime. The host runs in composer mode over one game root on disk with
// two instances of its game on one server, a manual Runtime clock and fake devices; the first
// policy cycle composes the root, which gives every rule and pool two copies, `<id>@<alias>`. The
// root's one rule produces its one pool (valued: step 1, gap weight 100; capacity 120), whose
// reading is 10 on each instance; its `goals.json` holds one `resource` entry on that pool by the
// file's id. No package runs; no clock is read. Every name is made up.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use crate::goals::GoalRefusal;
use actingcommand_contract::{
    DeclaredInitiator, GoalEdit, GoalEdited, GoalValueSource, GoalView, Initiator,
    ResourceTargetConditionState, ResourceTargetsRejectionReason, RuntimeChannel,
};
use actingcommand_policy::{GoalRecordEdit, GoalRecordRefusal, TargetSetter, edit_goal_record};
use serde_json::{Value, json};

const INSTANCE_A: &str = POLICY_INSTANCE_ALIAS;
const INSTANCE_B: &str = POLICY_INSTANCE_ALIAS_B;
/// The root's one rule and its package.
const TASK: &str = "fixture.goals";
/// The `resource` entry of the root's `goals.json`, on [`POOL_A`] by the file's id.
const ENTRY: &str = "stock";
const VIEWED: &str = "request_goal_view_completed";
const EDITED: &str = "request_edit_goal_completed";
const INVALID: &str = "goal_value_invalid";
const RECORD_INVALID: &str = "resource_targets_record_invalid";
const TARGETS_REASON: &str = "resource_targets";

/// The copy of `id` the composition makes for `alias`.
fn copy(id: &str, alias: &str) -> String {
    format!("{id}@{alias}")
}

/// Places the game root below `packs`: its bundle, its `schedule.json` with the one rule (due at
/// every evaluation, producing the pool, whose valuation names its step and its importance), the
/// package, and `goals.json` with the `resource` entry [`ENTRY`] on the pool.
fn place_game_root(packs: &Path) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[TASK]),
    );
    let mut produces = rule(TASK, TASK, 100, Some(POOL_A));
    produces["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    produces["loop_budget"]["window_iteration_limit"] = json!(16);
    produces["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(16);
    }
    let mut valued = pool(POOL_A);
    valued["valuation"]["gap"] = json!({"rule": "shortfall_linear", "weight_milli": 100});
    write_json(
        &game.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": [],
            "rules": [produces],
            "pools": [valued],
            "activity": activity,
            "timeline": [],
            "held": []
        }),
    );
    write_package(&game.join(TASK), TASK, None, None);
    write_json(
        &game.join("goals.json"),
        &json!({"schema_version": "actingcommand.goals.v2", "game": GAME_A, "entries": [
            {"name": ENTRY, "type": "resource", "governs": {"pool": POOL_A},
             "title": {"en": "Stock"}}
        ]}),
    );
}

/// The two instances of the game on its one server, each with its reading of the pool
/// (`resource.primary`, catalog-a's pool's key): 10, a minute before the fixture's time.
fn facts() -> EvaluationFacts {
    let mut inputs = policy_facts();
    inputs.outcomes.clear();
    inputs.instances = [INSTANCE_A, INSTANCE_B]
        .into_iter()
        .map(|alias| InstanceSnapshot {
            instance_id: alias.to_owned(),
            server_id: SERVER_A.to_owned(),
            game_id: GAME_A.to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec![
                actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned(),
            ],
            preferred_task_ids: Vec::new(),
        })
        .collect();
    for alias in [INSTANCE_A, INSTANCE_B] {
        inputs.facts.push(actingcommand_policy::ObservedFact {
            scope: actingcommand_policy::ScopeSelector::Instance {
                instance_id: alias.to_owned(),
            },
            fact_key: "resource.primary".to_owned(),
            value: actingcommand_policy::FactValue::Integer(10),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS - 60_000,
            expires_at_unix_ms: Some(POLICY_NOW_UNIX_MS + 86_400_000),
            confidence_milli: 1000,
        });
    }
    inputs
}

fn lab_request(client: &TestClient, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        client.ids.mint_request_id().expect("request id"),
        client.ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        operation,
    )
    .expect("lab runtime request")
}

/// The host over the game root, its clock at the fixture's time, and the files it keeps.
struct Scene {
    _state: TempDir,
    _packs: TempDir,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

impl Scene {
    fn new() -> Self {
        let state = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let provider = FakeProvider::from_entries([INSTANCE_A, INSTANCE_B].map(|alias| {
            (
                alias.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            )
        }));
        let host = RuntimeHost::start(
            config(&state)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_standard_packs_root(Some(packs.path().to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(provider),
        )
        .expect("start the composer-mode host");
        Self {
            _state: state,
            _packs: packs,
            clock,
            host,
        }
    }

    /// One policy cycle at `at`, the Runtime clock moved there first; the first composes the root.
    fn cycle(&self, at: u64) -> actingcommand_policy::PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample");
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                7,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy at {at}: {error}"))
            .evaluation
            .unwrap_or_else(|| panic!("a full evaluation at {at}"))
    }

    /// `alias`'s goal page, read on a connection of the CLI.
    fn view(&self, alias: &str) -> GoalView {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::GoalView {
            instance_alias: alias.to_owned(),
        });
        let receipt = client.send(&request);
        assert_eq!(receipt.outcome().code().as_str(), VIEWED, "{receipt:?}");
        let Some(RuntimeResult::GoalViewed { view }) = receipt.result() else {
            panic!("a goal view: {receipt:?}");
        };
        (**view).clone()
    }

    /// A connection bound `lab` with `Lab-CLI`, the Runtime clock at `at` first.
    fn lab_client(&self, at: u64) -> TestClient {
        advance_manual_clock_to(&self.clock, at);
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator: Some(DeclaredInitiator::from(Initiator::LabCli)),
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
        client
    }

    /// `EditGoal` of `op` on `alias` with `expected_version`, from the Lab at `at`.
    fn edit(
        &self,
        alias: &str,
        op: GoalEdit,
        expected_version: Option<u64>,
        at: u64,
    ) -> RuntimeReceipt {
        let mut client = self.lab_client(at);
        let request = lab_request(
            &client,
            RuntimeOperation::EditGoal {
                instance_alias: alias.to_owned(),
                expected_version,
                op,
            },
        );
        client.send(&request)
    }

    /// `ApplyResourceTargets` of `document`, from the Lab at `at`.
    fn apply(&self, document: &Value, at: u64) -> RuntimeReceipt {
        let mut client = self.lab_client(at);
        let request = lab_request(
            &client,
            RuntimeOperation::ApplyResourceTargets {
                document_json: document.to_string(),
                expected_version: None,
            },
        );
        client.send(&request)
    }

    /// How many `fact.published` records the ledger holds.
    fn publications(&self) -> usize {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            })
            .expect("ledger events")
            .len()
    }

    /// Nothing is latched: the Runtime holds no fatal condition.
    #[track_caller]
    fn goes_on(&self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime holds no fatal condition"
        );
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

fn set_op(name: &str, value: &Value) -> GoalEdit {
    GoalEdit::Set {
        name: name.to_owned(),
        value_json: value.to_string(),
    }
}

fn clear_op(name: &str) -> GoalEdit {
    GoalEdit::Clear {
        name: name.to_owned(),
    }
}

/// What a taken `EditGoal` answered.
#[track_caller]
fn taken(receipt: &RuntimeReceipt) -> GoalEdited {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(receipt.outcome().code().as_str(), EDITED, "{receipt:?}");
    let Some(RuntimeResult::GoalEdited { edited }) = receipt.result() else {
        panic!("an edited goal: {receipt:?}");
    };
    edited.clone()
}

/// A refused request: Denied with `code`, no result; its outcome.
#[track_caller]
fn denied(receipt: &RuntimeReceipt, code: &str) -> actingcommand_contract::outcome::Outcome {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
    assert!(receipt.result().is_none(), "{receipt:?}");
    receipt.outcome().clone()
}

/// The `resource_targets` reason of the decision of the rule's copy on `alias`, if it has one.
fn targets_reason(
    evaluation: &actingcommand_policy::PolicyEvaluation,
    alias: &str,
) -> Option<String> {
    let task_id = copy(TASK, alias);
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {alias}: {evaluation:#?}"));
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == TARGETS_REASON)
        .map(|reason| reason.detail.clone())
}

/// The gap the goal page shows on its resource entry, and the stored target's id there.
fn entry_goal(view: &GoalView) -> (Option<String>, Option<u64>, Option<GoalValueSource>) {
    let entry = view
        .entries
        .iter()
        .find(|entry| entry.name == ENTRY)
        .unwrap_or_else(|| panic!("the entry {ENTRY}: {view:?}"));
    let state = entry.resource.as_ref().expect("a resource entry's state");
    (state.target_id.clone(), state.gap, entry.source)
}

/// The goal of `target_id` on instance A is in effect on A's copy alone: the view of A shows the
/// target active with its gap (at least 50 over the reading 10: 40) on the entry, and its stored
/// row names the pool by the file's id; the cycle at `at` gives the producing candidate of A's
/// copy the target, applied with that gap; instance B has no record and B's candidate no target.
#[track_caller]
fn in_effect_on_a_alone(scene: &Scene, target_id: &str, at: u64) {
    let a = scene.view(INSTANCE_A);
    assert_eq!(
        entry_goal(&a),
        (
            Some(target_id.to_owned()),
            Some(40),
            Some(GoalValueSource::Set)
        ),
        "{a:?}"
    );
    let active = a.targets.active.as_ref().expect("an active record on A");
    assert_eq!(active.targets.len(), 1, "{active:?}");
    assert_eq!(
        active.targets[0]["resource"],
        json!(POOL_A),
        "the file's id"
    );
    assert_eq!(active.targets[0]["id"], json!(target_id));
    assert_eq!(active.conditions.len(), 1, "{active:?}");
    assert_eq!(active.conditions[0].target_id, target_id);
    assert!(
        matches!(
            active.conditions[0].state,
            ResourceTargetConditionState::Computed {
                current: 10,
                gap: 40,
                ..
            }
        ),
        "{active:?}"
    );
    let b = scene.view(INSTANCE_B);
    assert_eq!(b.version, 0, "{b:?}");
    assert!(b.targets.active.is_none(), "{b:?}");
    assert_eq!(
        entry_goal(&b),
        (None, None, Some(GoalValueSource::Default)),
        "{b:?}"
    );

    let evaluation = scene.cycle(at);
    let applied = targets_reason(&evaluation, INSTANCE_A)
        .unwrap_or_else(|| panic!("A's candidate carries the target: {evaluation:#?}"));
    let expected = format!(
        "{target_id}@{}:applied current=10 at_least=50 gap=40",
        copy(POOL_A, INSTANCE_A)
    );
    assert!(applied.starts_with(&expected), "{applied}");
    assert_eq!(
        targets_reason(&evaluation, INSTANCE_B),
        None,
        "B's candidate has no target"
    );
}

// The halt of 11-27 (the work order's first test). The page of instance A lists its pool once, by
// the file's id, with its producing task (A's copy) and the pool valuation's step and importance;
// no other instance's copy is listed. An other target that names the pool by A's copy id
// (`<pool>@<alias A>`), a name no entry has, `expected_version` 0, as production sent it: refused
// `goal_value_invalid`, `wrong_type` at `/resource`, nothing appended, the record's version still 0,
// nothing latched, the next request answered and a policy cycle evaluated. How this shows that
// 0.12.87 stopped here: its page listed the pool by that copy id, so its check passed this value
// and made the edit below of it; the record cannot hold that edit (its read-back fails,
// `Inconsistent`), and 0.12.87 answered `Inconsistent` (no refusal of the record's,
// `GoalRefusal::of_record`) with the fatal `resource_targets_record_invalid`, which latched.
#[test]
fn goal1_t1_an_other_target_by_a_copy_id_is_refused_and_the_runtime_goes_on() {
    let scene = Scene::new();
    scene.cycle(POLICY_NOW_UNIX_MS);
    let view = scene.view(INSTANCE_A);
    assert_eq!(view.version, 0);
    assert_eq!(
        view.targets
            .targetable
            .iter()
            .map(|targetable| (
                targetable.resource.clone(),
                targetable.producing_tasks.clone(),
                targetable.default_scale,
                targetable.default_importance_milli
            ))
            .collect::<Vec<_>>(),
        [(
            POOL_A.to_owned(),
            vec![copy(TASK, INSTANCE_A)],
            Some(1),
            Some(100)
        )],
        "{view:?}"
    );
    assert!(view.targets.not_targetable.is_empty(), "{view:?}");

    let copy_id = copy(POOL_A, INSTANCE_A);
    let premise = edit_goal_record(
        None,
        None,
        INSTANCE_A,
        &GoalRecordEdit::SetTarget {
            id: "spare".to_owned(),
            resource: copy_id.clone(),
            at_least: 50,
        },
        TargetSetter {
            set_unix_ms: POLICY_NOW_UNIX_MS,
            channel: RuntimeChannel::Lab,
            initiator: Some(Initiator::LabCli),
        },
    );
    assert_eq!(premise.err(), Some(GoalRecordRefusal::Inconsistent));
    assert_eq!(
        GoalRefusal::of_record(GoalRecordRefusal::Inconsistent),
        None
    );

    let before = scene.publications();
    let refused = denied(
        &scene.edit(
            INSTANCE_A,
            set_op("spare", &json!({"resource": copy_id, "at_least": 50})),
            Some(0),
            POLICY_NOW_UNIX_MS + 1_000,
        ),
        INVALID,
    );
    assert_eq!(refused.value("goal_problem"), Some(&json!("wrong_type")));
    assert_eq!(refused.value("json_pointer"), Some(&json!("/resource")));
    assert_eq!(scene.publications(), before, "nothing appended");
    scene.goes_on();
    let after = scene.view(INSTANCE_A);
    assert_eq!(after.version, 0, "the record's version is unchanged");
    assert!(after.other_targets.is_empty(), "{after:?}");
    scene.cycle(POLICY_NOW_UNIX_MS + 2_000);
    scene.goes_on();
    scene.close();
}

// The work order's second test, the first failure of 11-27: the `resource` entry, whose
// `governs.pool` is the file's id, is set `{at_least: 50}` on instance A (0.12.87 refused it
// `pool_not_targetable`): taken; the stored row names the pool by the file's id; A's evaluation
// applies the target to A's copy and B has no target.
#[test]
fn goal1_t2_a_resource_entry_is_set_by_the_files_id_and_applies_to_its_own_copy_alone() {
    let scene = Scene::new();
    scene.cycle(POLICY_NOW_UNIX_MS);
    let edited = taken(&scene.edit(
        INSTANCE_A,
        set_op(ENTRY, &json!({"at_least": 50})),
        Some(0),
        POLICY_NOW_UNIX_MS + 1_000,
    ));
    assert_eq!(edited.replaced_target_id, None);
    assert_eq!(scene.view(INSTANCE_A).version, edited.version);
    in_effect_on_a_alone(&scene, ENTRY, POLICY_NOW_UNIX_MS + 2_000);
    scene.goes_on();
    scene.close();
}

// The work order's third test: an other target `{resource: <the file's id>, at_least: 50}` under a
// name no entry has is taken, with the same result (the entry on its pool shows it), and is cleared
// by its name: A's record then holds no target and A's candidate carries none.
#[test]
fn goal1_t3_an_other_target_by_the_files_id_is_taken_and_cleared_by_its_name() {
    let scene = Scene::new();
    scene.cycle(POLICY_NOW_UNIX_MS);
    let edited = taken(&scene.edit(
        INSTANCE_A,
        set_op("reserve", &json!({"resource": POOL_A, "at_least": 50})),
        None,
        POLICY_NOW_UNIX_MS + 1_000,
    ));
    assert_eq!(edited.replaced_target_id, None);
    in_effect_on_a_alone(&scene, "reserve", POLICY_NOW_UNIX_MS + 2_000);

    let cleared = taken(&scene.edit(
        INSTANCE_A,
        clear_op("reserve"),
        None,
        POLICY_NOW_UNIX_MS + 3_000,
    ));
    let view = scene.view(INSTANCE_A);
    assert_eq!(view.version, cleared.version);
    assert!(view.targets.active.is_none(), "{view:?}");
    assert_eq!(
        entry_goal(&view),
        (None, None, Some(GoalValueSource::Default)),
        "{view:?}"
    );
    let evaluation = scene.cycle(POLICY_NOW_UNIX_MS + 4_000);
    assert_eq!(targets_reason(&evaluation, INSTANCE_A), None);
    scene.goes_on();
    scene.close();
}

// The work order's fifth test: a v3 document through `ApplyResourceTargets` that names the pool by
// the file's id for instance A is taken and applied to A's copy alone; one that names A's copy id,
// or B's, is refused `resource_targets_rejected` with `unknown_resource` at `/targets/0/resource`
// (until 0.12.88 the first was taken and left a record its reader refuses, the second refused
// `resource_out_of_scope`); nothing is appended for them and the Runtime goes on.
#[test]
fn goal1_t5_a_v3_document_names_the_files_id_and_a_copy_id_is_refused() {
    let scene = Scene::new();
    scene.cycle(POLICY_NOW_UNIX_MS);
    let document = |resource: &str| {
        json!({
            "schema_version": "actingcommand.resource-targets.v3",
            "instance": INSTANCE_A,
            "targets": [{
                "id": "floor",
                "resource": resource,
                "condition": {"kind": "at_least", "amount": 50},
                "apply": {"mode": "adjust", "weight": "score_stage"}
            }]
        })
    };
    let before = scene.publications();
    for (index, refused) in [copy(POOL_A, INSTANCE_A), copy(POOL_A, INSTANCE_B)]
        .iter()
        .enumerate()
    {
        let at = POLICY_NOW_UNIX_MS + 1_000 + u64::try_from(index).expect("an index");
        let receipt = scene.apply(&document(refused.as_str()), at);
        denied(&receipt, "resource_targets_rejected");
        let rejection = receipt
            .resource_targets_rejection()
            .unwrap_or_else(|| panic!("{refused}: the rejection: {receipt:?}"));
        assert_eq!(
            (rejection.field_path.as_str(), rejection.reason),
            (
                "/targets/0/resource",
                ResourceTargetsRejectionReason::UnknownResource
            ),
            "{refused}"
        );
    }
    assert_eq!(scene.publications(), before, "nothing appended");
    scene.goes_on();

    let receipt = scene.apply(&document(POOL_A), POLICY_NOW_UNIX_MS + 3_000);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    in_effect_on_a_alone(&scene, "floor", POLICY_NOW_UNIX_MS + 4_000);
    scene.goes_on();
    scene.close();
}

// The work order's sixth test: a composed record that does not read back, forced by the test-only
// switch (`unreadable_goal_edit_for_test`, which makes the edit name A's copy id as 0.12.87's
// checks did, so the record's read-back fails): the entry's `set` is refused
// `resource_targets_record_invalid`, not `goal_value_invalid`, nothing appended, the version still 0,
// nothing latched, the next request answered and a policy cycle evaluated; with the switch off the
// same request is taken.
#[test]
fn goal1_t6_a_composed_record_that_does_not_read_back_is_refused_and_the_runtime_goes_on() {
    let scene = Scene::new();
    scene.cycle(POLICY_NOW_UNIX_MS);
    scene
        .host
        .unreadable_goal_edit_for_test(true)
        .expect("the test switch");
    let before = scene.publications();
    denied(
        &scene.edit(
            INSTANCE_A,
            set_op(ENTRY, &json!({"at_least": 50})),
            Some(0),
            POLICY_NOW_UNIX_MS + 1_000,
        ),
        RECORD_INVALID,
    );
    assert_eq!(scene.publications(), before, "nothing appended");
    scene.goes_on();
    assert_eq!(scene.view(INSTANCE_A).version, 0);
    scene.cycle(POLICY_NOW_UNIX_MS + 2_000);
    scene.goes_on();

    scene
        .host
        .unreadable_goal_edit_for_test(false)
        .expect("the test switch");
    let edited = taken(&scene.edit(
        INSTANCE_A,
        set_op(ENTRY, &json!({"at_least": 50})),
        Some(0),
        POLICY_NOW_UNIX_MS + 3_000,
    ));
    assert_eq!(scene.view(INSTANCE_A).version, edited.version);
    scene.close();
}
