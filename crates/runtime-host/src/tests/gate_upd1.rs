// SPDX-License-Identifier: AGPL-3.0-only

// UPD-1 (0.12.30; design `rd-1029/design-upd1.md` §3, §4, §9 rows A7, A8 and B1, B3, B5, B6;
// Alice's rulings 95 and 96 of 2026-10-29; register §1.215): the instance fact
// `health.application.update_required` and the scheduling gate it holds (position 4a).
//
// - A7: a start seeds `false` for each configured instance without a record, once; a record
//   already there (`true` included) is kept.
// - A8: a client's publication of the key is refused `application_update_key_reserved`, and
//   nothing is appended.
// - B1, B3: while the fact is true, the instance's ordinary task is deferred
//   `application_update_pending` with no wake, and an intent evaluated before the fact turned true
//   is stale at its admission; a task whose bound package declares `store_update` still runs. The
//   gate holds only the instances whose fact is true (#419).
// - B4 (fix round 1, review M3): the store has no new version yet. The store update task runs
//   while the fact is true, its "not needed" success clears it, the next ordinary run meets the
//   dialog and the fact is true again: the store update task looks again only once its own
//   `cooldown_ms` from its last dispatch is over, and every other task stays held meanwhile.
// - B5: a decision the gate holds never makes the idle plan due nor gives it a wake; fix round 1
//   (review M1): nor does any other decision of a waiting instance but a store update task's, a
//   task in its cooldown or before its clock trigger included.
// - B6: a store update package placed in place of a task's package (the catalog unchanged) is
//   known as one once the remedy table is rebuilt, at `lift_check_min_interval_ms`; a package the
//   table cannot read is no store task.
//
// The policy fixture's one instance; a catalog of one task (`t_update`, due at every evaluation)
// whose procedure is bound to a package location, as actingd binds a scheduled procedure; B4 adds
// a store update task bound to a store update package.

use super::*;
use crate::codes::{ApplicationUpdateCode, HostCode};
use crate::schedule_explanation::ScheduleExplanationTable;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{
    CandidateEligibility, EligibilityState, ObservedFact, PolicyCode, PolicyEvaluation,
    SchedulingDecisionState, ScopeSelector, TaskDecision,
};
use serde_json::{Value, json};

const KEY: &str = "health.application.update_required";
const PENDING: &str = "application_update_pending";
const RESERVED: &str = "application_update_key_reserved";
const SEED: &str = "application_update:seed";
const STALE: &str = "policy_facts_stale";
const TASK: &str = "t_update";
const PROCEDURE: &str = "procedure.update";
const MINUTE_MS: u64 = 60_000;
/// B4: the store update task, its procedure and its rule's cooldown (the design's example).
const STORE_TASK: &str = "t_store_update";
const STORE_PROCEDURE: &str = "procedure.store_update";
const STORE_COOLDOWN_MS: u64 = 30 * MINUTE_MS;

/// Every publication of the key for the policy instance, in ledger order: its value and its
/// `source_snapshot_id`.
fn publications(host: &RuntimeHost) -> Vec<(bool, String)> {
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::FactPublished),
        ..EventQuery::default()
    })
    .expect("the fact publications")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Fact(actingcommand_contract::FactPayload::Published(payload)) => {
            Some(payload.record().clone())
        }
        _ => None,
    })
    .filter(|record| record.key == KEY && record.scope == scope)
    .filter_map(|record| match record.content {
        FactContent::Inline {
            value: ContractFactValue::Boolean(value),
        } => Some((value, record.source_snapshot_id)),
        _ => None,
    })
    .collect()
}

/// A host on `root` whose configuration names the policy instance (its inputs), as actingd
/// starts with a policy configuration.
fn configured_host(root: &TempDir) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

// ---- A7, A8.

/// A7: the first start seeds `false` from `application_update:seed`; a second start appends
/// nothing; a `true` a run left survives the next start, which seeds nothing over it.
#[test]
fn upd1_a7_a_start_seeds_false_once_and_keeps_a_record_already_there() {
    let root = TempDir::new().expect("tempdir");
    let host = configured_host(&root);
    assert_eq!(
        host.application_update_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance fact"),
        Some((false, SEED.to_owned()))
    );
    assert_eq!(publications(&host), [(false, SEED.to_owned())]);
    host.close().expect("close the host");

    let host = configured_host(&root);
    assert_eq!(
        publications(&host),
        [(false, SEED.to_owned())],
        "a second start appends nothing"
    );
    host.record_application_update_for_test(POLICY_INSTANCE_ALIAS, true)
        .expect("a run met the dialog");
    host.close().expect("close the host");

    let host = configured_host(&root);
    assert_eq!(
        host.application_update_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance fact")
            .map(|(value, _)| value),
        Some(true),
        "a restart keeps the true a run left"
    );
    assert_eq!(publications(&host).len(), 2, "no seed over the record");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// A8: an agent's `PublishFact` of the key is denied `application_update_key_reserved` (the key
/// as `fact_key`), as is the Runtime's own ordinary publication path; neither appends anything.
#[test]
fn upd1_a8_a_client_publish_of_the_key_is_refused() {
    let root = TempDir::new().expect("tempdir");
    let host = configured_host(&root);
    let before = publications(&host);
    let record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        KEY,
        ContractFactValue::Boolean(false),
        "snapshot:upd1-client",
        Vec::new(),
    );
    let mut client = TestClient::connect(&host);
    let request = client.agent_request(RuntimeOperation::PublishFact {
        record: record.clone(),
    });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), RESERVED, "{receipt:?}");
    assert_eq!(
        receipt.outcome().value("fact_key"),
        Some(&json!(KEY)),
        "{receipt:?}"
    );
    let error = host
        .publish_fact(record)
        .expect_err("only the Runtime's own write takes the key");
    assert_eq!(error.code(), RESERVED, "{error:?}");
    assert!(!error.is_fatal(), "{error:?}");
    assert_eq!(publications(&host), before, "nothing is appended");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close the host");
}

// ---- B1, B3, B6: the gate on a host.

/// The fixture catalog with the one task [`TASK`], due at every evaluation.
fn catalog() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let mut task = tasks["tasks"][0].clone();
    task["id"] = json!(TASK);
    task["procedure_ref"] = json!(PROCEDURE);
    task["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    tasks["tasks"] = json!([task]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    sources
}

/// B4: [`catalog`] and the store update task [`STORE_TASK`], whose rule is the store update rule
/// of `standard-pack-schedule.md`: triggered while the instance's
/// `health.application.update_required` is true, with a cooldown of [`STORE_COOLDOWN_MS`].
fn store_catalog() -> CatalogSources {
    let mut sources = catalog();
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let mut store = tasks["tasks"][0].clone();
    store["id"] = json!(STORE_TASK);
    store["procedure_ref"] = json!(STORE_PROCEDURE);
    store["trigger"] = json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": KEY,
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": null
    });
    store["cooldown_ms"] = json!(STORE_COOLDOWN_MS);
    store["produces"] = json!([]);
    store["instance_overrides"] = json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("fixture task list")
        .push(store);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    sources
}

/// The fixture's facts without a scheduling outcome (the catalog references none).
fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts
}

struct Bench {
    _root: TempDir,
    package: PathBuf,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

impl Bench {
    /// A host whose task [`TASK`] is bound to `package`, written at its location, the catalog
    /// active and the clock at the fixture's time.
    fn new(package: &[u8]) -> Self {
        Self::bound(&[(PROCEDURE, package)], &catalog())
    }

    /// A host whose procedures are each bound to its package (`bindings`, each written at a
    /// location of its own; [`Bench::package`] is the first one's), `sources` the active catalog
    /// and the clock at the fixture's time.
    fn bound(bindings: &[(&str, &[u8])], sources: &CatalogSources) -> Self {
        let root = TempDir::new().expect("tempdir");
        let mut paths = Vec::new();
        let mut procedures = Vec::new();
        for (index, &(procedure, package)) in bindings.iter().enumerate() {
            let path = root.path().join(format!("bound-{index}.zip"));
            fs::write(&path, package).expect("write the package");
            procedures.push(
                ProcedureBinding::new(
                    procedure,
                    "operation.observe",
                    vec!["after_observation".to_owned()],
                )
                .expect("procedure binding")
                .with_package_path(path.to_string_lossy()),
            );
            paths.push(path);
        }
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let manifest = ProcedureManifest::new(procedures).expect("procedure manifest");
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_procedure_manifest(manifest),
            Arc::new(
                FakeProvider::one(
                    POLICY_INSTANCE_ALIAS,
                    instance_id(),
                    Arc::new(FakeState::default()),
                )
                .fixture_simulation(),
            ),
        )
        .expect("runtime host");
        host.activate_policy_catalog(sources)
            .expect("activate the catalog");
        Self {
            _root: root,
            package: paths.swap_remove(0),
            clock,
            host,
        }
    }

    /// One full evaluation at `at`, the Runtime clock moved there first.
    fn evaluate(&self, at: u64, seed: u64) -> PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                seed,
                PolicyTrigger::FactsChanged,
            )
            .expect("evaluate the policy")
            .evaluation
            .expect("a full evaluation")
    }

    fn set(&self, required: bool) {
        self.host
            .record_application_update_for_test(POLICY_INSTANCE_ALIAS, required)
            .expect("record the instance fact");
    }

    /// [`Bench::set`] with the Runtime clock moved to `at` first.
    fn set_at(&self, at: u64, required: bool) {
        advance_manual_clock_to(&self.clock, at);
        self.set(required);
    }

    /// Admits `intent` (the catalog approved before), settles its run as `input` and ends its
    /// lease, as a scheduled run that ran to its end does.
    fn run(
        &self,
        intent: &DispatchIntent,
        chain: &DecisionReasonChain,
        input: &PolicyExecutionInput,
    ) {
        let admission = self
            .host
            .admit_policy_dispatch(intent, chain, &policy_context(&self.host, intent))
            .unwrap_or_else(|error| panic!("admit {}: {error}", intent.task_id));
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("{} is granted", intent.task_id);
        };
        self.host
            .record_policy_dispatch_outcome(&intent.decision_id, input)
            .unwrap_or_else(|error| panic!("settle {}: {error}", intent.task_id));
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close the host");
    }
}

/// The dispatch of [`TASK`] on the policy instance, with its reason chain.
fn dispatched(evaluation: &PolicyEvaluation) -> Option<(DispatchIntent, DecisionReasonChain)> {
    dispatched_task(evaluation, TASK)
}

/// The dispatch of `task_id` on the policy instance, with its reason chain.
fn dispatched_task(
    evaluation: &PolicyEvaluation,
    task_id: &str,
) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == POLICY_INSTANCE_ALIAS)?
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    Some((intent, chain))
}

/// The decision of [`TASK`] on the policy instance.
fn decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    decision_of(evaluation, TASK)
}

/// The decision of `task_id` on the policy instance.
fn decision_of<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .unwrap_or_else(|| panic!("the decision of {task_id}: {evaluation:#?}"))
}

/// B4: asserts `task_id` waits at position 4a in `evaluation`: deferred and not dispatched, one of
/// its reasons `application_update_pending` (an owed rerun's decision carries its own reasons
/// before it).
fn assert_pending(evaluation: &PolicyEvaluation, task_id: &str, case: &str) {
    assert!(
        dispatched_task(evaluation, task_id).is_none(),
        "{case}: {task_id} is not dispatched"
    );
    let held = decision_of(evaluation, task_id);
    assert_eq!(
        held.state,
        SchedulingDecisionState::Deferred,
        "{case}: {:?}",
        held.reasons
    );
    assert!(
        held.reasons
            .iter()
            .any(|reason| reason.outcome.code().as_str() == PENDING),
        "{case}: {:?}",
        held.reasons
    );
}

/// Asserts the decision of [`TASK`] waits at position 4a: its last reason is
/// `application_update_pending` naming the instance and the key, with no wake of its own.
fn assert_held(evaluation: &PolicyEvaluation, case: &str) {
    assert!(dispatched(evaluation).is_none(), "{case}: no dispatch");
    let held = decision(evaluation);
    assert_eq!(held.state, SchedulingDecisionState::Deferred, "{case}");
    let reason = &held.reasons.last().expect("a reason").outcome;
    assert_eq!(
        reason.code().as_str(),
        PENDING,
        "{case}: {:?}",
        held.reasons
    );
    assert_eq!(
        reason.value("instance_id"),
        Some(&json!(POLICY_INSTANCE_ALIAS)),
        "{case}"
    );
    assert_eq!(reason.value("fact_key"), Some(&json!(KEY)), "{case}");
    assert_eq!(held.next_wake_unix_ms, None, "{case}: no wake of its own");
}

/// B1 and B3: an ordinary task is dispatched while no update waits. Its intent, evaluated before
/// the fact turned true, is refused `policy_facts_stale` at admission (no run starts in the gap);
/// the next evaluation defers it `application_update_pending`. Once the fact is false again it is
/// dispatched. A task whose bound package declares `store_update` is dispatched while the fact is
/// true.
#[test]
fn upd1_b1_b3_while_the_fact_is_true_only_the_store_update_task_runs() {
    let bench = Bench::new(&neutral_contained_task_package(true));
    let first = bench.evaluate(POLICY_NOW_UNIX_MS, 1);
    let (intent, chain) = dispatched(&first)
        .unwrap_or_else(|| panic!("dispatched while no update waits: {first:#?}"));
    bench.set(true);
    record_policy_approval(&bench.host, &intent);
    let refused = bench
        .host
        .admit_policy_dispatch(&intent, &chain, &policy_context(&bench.host, &intent))
        .expect_err("an intent evaluated before the fact turned true");
    assert_eq!(refused.code(), STALE);
    let held = bench.evaluate(POLICY_NOW_UNIX_MS + MINUTE_MS, 2);
    assert_held(&held, "the ordinary task");
    bench.set(false);
    let after = bench.evaluate(POLICY_NOW_UNIX_MS + 2 * MINUTE_MS, 3);
    assert!(
        dispatched(&after).is_some(),
        "dispatched again once the update is done: {after:#?}"
    );
    bench.close();

    let bench = Bench::new(&super::gate_store_fg2::store_package());
    let first = bench.evaluate(POLICY_NOW_UNIX_MS, 4);
    assert!(dispatched(&first).is_some(), "{first:#?}");
    let table = bench
        .host
        .remedy_table_for_test()
        .expect("the policy host")
        .expect("the remedy table");
    assert!(table.store_tasks().contains(TASK), "a store update task");
    bench.set(true);
    let store = bench.evaluate(POLICY_NOW_UNIX_MS + MINUTE_MS, 5);
    assert!(
        dispatched(&store).is_some(),
        "the store update task passes the gate: {store:#?}"
    );
    bench.close();
}

/// B6: the task's package is replaced in place while the fact is true (the catalog unchanged).
/// Until the remedy table is rebuilt the gate still knows it as ordinary; at the rebuild, one
/// `lift_check_min_interval_ms` (60 s shipped) after the last build, an unreadable package is no
/// store task (the table lists it unreadable) and a store update package is one.
#[test]
fn upd1_b6_a_store_update_package_placed_in_place_is_known_at_the_next_rebuild() {
    let bench = Bench::new(&neutral_contained_task_package(true));
    bench.evaluate(POLICY_NOW_UNIX_MS, 1);
    bench.set(true);
    assert_held(
        &bench.evaluate(POLICY_NOW_UNIX_MS + 1_000, 2),
        "the ordinary package",
    );

    fs::write(&bench.package, b"not a package").expect("replace the package");
    let at = POLICY_NOW_UNIX_MS + 2 * MINUTE_MS;
    assert_held(&bench.evaluate(at, 3), "an unreadable package");
    let table = bench
        .host
        .remedy_table_for_test()
        .expect("the policy host")
        .expect("the remedy table");
    assert!(
        !table.unreadable.is_empty(),
        "the table names the unreadable package"
    );
    assert!(table.store_tasks().is_empty());

    fs::write(&bench.package, super::gate_store_fg2::store_package())
        .expect("place the store update package");
    assert_held(&bench.evaluate(at + 1_000, 4), "before the next rebuild");
    let store = bench.evaluate(at + 2 * MINUTE_MS, 5);
    assert!(
        dispatched(&store).is_some(),
        "known as a store update task at the rebuild: {store:#?}"
    );
    bench.close();
}

/// B4 (design §6 item 5; fix round 1, review M3): the store has no new version yet, on a fake
/// clock.
/// 1. While the fact is true the store update task is dispatched (the ordinary task held); its run
///    ends "not needed", a success.
/// 2. That success cleared the fact: the ordinary task is dispatched, the store update task's
///    trigger is false.
/// 3. The ordinary run meets the dialog again: it ends `contained_task_application_update_required`
///    and the fact is true again.
/// 4. The store update task waits its own `cooldown_ms` from its last dispatch
///    (`task_cooldown_active`, its wake there): nothing is dispatched, 1 ms before that time too.
/// 5. At that time it is dispatched again.
///
/// The ordinary task stays held (`application_update_pending`) whenever the fact is true.
#[test]
fn upd1_b4_without_a_new_version_the_store_task_looks_again_only_after_its_cooldown() {
    let ordinary = neutral_contained_task_package(true);
    let store = super::gate_store_fg2::store_package();
    let bench = Bench::bound(
        &[
            (PROCEDURE, ordinary.as_slice()),
            (STORE_PROCEDURE, store.as_slice()),
        ],
        &store_catalog(),
    );
    bench.set_at(POLICY_NOW_UNIX_MS, true);
    let first_at = POLICY_NOW_UNIX_MS + 1_000;
    let due = first_at + STORE_COOLDOWN_MS;

    // 1.
    let first = bench.evaluate(first_at, 1);
    assert_pending(&first, TASK, "the dialog met");
    let (intent, chain) = dispatched_task(&first, STORE_TASK)
        .unwrap_or_else(|| panic!("the store update task is dispatched: {first:#?}"));
    record_policy_approval(&bench.host, &intent);
    bench.run(&intent, &chain, &PolicyExecutionInput::Succeeded);

    // 2.
    let cleared = POLICY_NOW_UNIX_MS + 10 * MINUTE_MS;
    bench.set_at(cleared, false);
    let second = bench.evaluate(cleared, 2);
    assert!(
        dispatched_task(&second, STORE_TASK).is_none(),
        "no store update while the fact is false: {second:#?}"
    );
    let (intent, chain) = dispatched_task(&second, TASK)
        .unwrap_or_else(|| panic!("the ordinary task is dispatched: {second:#?}"));

    // 3.
    bench.run(
        &intent,
        &chain,
        &PolicyExecutionInput::Failed {
            outcome: Outcome::new(ApplicationUpdateCode::RunEnded).page_id("notice"),
            failure_identity: None,
            class: PolicyFailureClass::Recoverable,
        },
    );
    bench.set_at(cleared + MINUTE_MS, true);

    // 4.
    for (at, seed) in [(cleared + 2 * MINUTE_MS, 3), (due - 1, 4)] {
        let waiting = bench.evaluate(at, seed);
        assert!(
            waiting.dispatch_intents.is_empty(),
            "{at}: nothing is dispatched: {waiting:#?}"
        );
        let store = decision_of(&waiting, STORE_TASK);
        assert_eq!(store.state, SchedulingDecisionState::Deferred, "{at}");
        assert!(
            store
                .reasons
                .iter()
                .any(|reason| reason.outcome.is(PolicyCode::TaskCooldownActive)),
            "{at}: {:?}",
            store.reasons
        );
        assert_eq!(
            store.next_wake_unix_ms,
            Some(due),
            "{at}: the last dispatch plus the cooldown"
        );
        assert_pending(&waiting, TASK, &format!("{at}: the dialog met again"));
    }

    // 5.
    let again = bench.evaluate(due, 5);
    assert!(
        dispatched_task(&again, STORE_TASK).is_some(),
        "the store update task looks again: {again:#?}"
    );
    assert_pending(&again, TASK, "the store update task runs");
    bench.close();
}

// ---- #419, B5: the gate's parts.

/// #419: only an instance whose fact is true (with a confidence above zero) is held; another
/// instance, a `false`, a fact of another key or of a wider scope hold nothing.
#[test]
fn upd1_b1_the_gate_holds_only_the_instances_whose_fact_is_true() {
    let fact = |instance: &str, key: &str, value: bool, confidence_milli: u16| ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: instance.to_owned(),
        },
        fact_key: key.to_owned(),
        value: actingcommand_policy::FactValue::Boolean(value),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: None,
        confidence_milli,
    };
    let mut facts = policy_facts();
    facts.facts = vec![
        fact("instance.a", KEY, true, 1_000),
        fact("instance.b", KEY, false, 1_000),
        fact("instance.c", KEY, true, 0),
        fact("instance.d", "health.application.other", true, 1_000),
        ObservedFact {
            scope: ScopeSelector::Game {
                game_id: "fixture-game-a".to_owned(),
            },
            ..fact("instance.e", KEY, true, 1_000)
        },
    ];
    assert_eq!(
        crate::application_update::pending_instances(&facts),
        BTreeSet::from(["instance.a".to_owned()])
    );
    let CandidateEligibility::Deferred {
        reason,
        next_wake_unix_ms: None,
    } = crate::application_update::deferral("instance.a", false)
    else {
        panic!("a deferral with no wake");
    };
    assert!(reason.outcome.is(ApplicationUpdateCode::Pending));
    let CandidateEligibility::Refresh {
        gate,
        next_wake_unix_ms: None,
        limit: None,
        ..
    } = crate::application_update::deferral("instance.a", true)
    else {
        panic!("the fact refresh's deferral names the gate");
    };
    assert_eq!(gate, ApplicationUpdateCode::Pending.code());
}

/// A decision of the instance `node-a` for the idle plan.
fn idle_decision(
    task_id: &str,
    state: SchedulingDecisionState,
    reasons: Vec<Outcome>,
) -> TaskDecision {
    TaskDecision {
        task_id: task_id.to_owned(),
        instance_id: Some("node-a".to_owned()),
        eligibility: EligibilityState::True,
        state,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: reasons
            .into_iter()
            .map(|outcome| actingcommand_policy::DecisionReason {
                outcome,
                detail: String::new(),
            })
            .collect(),
        next_wake_unix_ms: None,
    }
}

/// The idle plan of `node-a` from an evaluation of `decisions` at `at`.
fn idle_plan(decisions: Vec<TaskDecision>, at: u64) -> crate::idle_action::IdlePlan {
    let mut table = ScheduleExplanationTable::default();
    table.record(
        &PolicyEvaluation {
            decisions,
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        },
        "sha256:upd1",
        at,
        &|_: &str, _: &str| -> Option<(u64, String)> { None },
    );
    table.idle_plan("node-a").expect("an evaluated plan")
}

/// B5: a task and the fact refresh that position 4a holds are neither due nor a wake, so the
/// idle plan never wakes the instance for them (its startup package would only meet the dialog
/// again); the same deferral without the gate would be due. A selected store update task is due.
#[test]
fn upd1_b5_a_decision_the_update_holds_never_wakes_the_idle_instance() {
    let at = POLICY_NOW_UNIX_MS;
    let pending = || {
        Outcome::new(ApplicationUpdateCode::Pending)
            .instance_id("node-a")
            .fact_key(KEY)
    };
    let refresh_pending = || {
        Outcome::new(PolicyCode::FactRefreshPending)
            .instance_id("node-a")
            .fact_key("resource.primary")
            .gate_code(ApplicationUpdateCode::Pending.code())
    };
    let held = vec![
        idle_decision(
            "task.ordinary",
            SchedulingDecisionState::Deferred,
            vec![pending()],
        ),
        idle_decision(
            actingcommand_policy::FACT_REFRESH_TASK_ID,
            SchedulingDecisionState::Deferred,
            vec![refresh_pending()],
        ),
    ];
    let plan = idle_plan(held.clone(), at);
    assert!(!plan.due_now, "{plan:?}");
    assert_eq!(plan.next_wake_unix_ms, None, "{plan:?}");
    assert_eq!(plan.reading_due_unix_ms, None, "{plan:?}");

    let ungated = vec![idle_decision(
        "task.ordinary",
        SchedulingDecisionState::Deferred,
        vec![Outcome::new(HostCode::DispatchInstanceHeld)],
    )];
    assert!(
        idle_plan(ungated, at).due_now,
        "a deferral with no wake is due"
    );

    let mut with_store = held;
    with_store.push(idle_decision(
        "task.store_update",
        SchedulingDecisionState::Selected,
        Vec::new(),
    ));
    assert!(idle_plan(with_store, at).due_now, "the store update task");

    // Fix round 1 (review M1): on the waiting instance an ordinary task in its cooldown, one whose
    // clock trigger has not come and a fact refresh waiting for its time are no candidates yet:
    // their decisions name their own wakes and no position 4a. None of them wakes the instance;
    // the store update task's cooldown still does. Where no instance waits, or another one does,
    // the same decisions wake it at their own times.
    let cooling = || {
        later_decision(
            "task.ordinary",
            SchedulingDecisionState::Deferred,
            EligibilityState::True,
            PolicyCode::TaskCooldownActive,
            at + HOUR_MS,
        )
    };
    let clock = || {
        later_decision(
            "task.clock",
            SchedulingDecisionState::Blocked,
            EligibilityState::False,
            PolicyCode::TriggerFalse,
            at + 2 * HOUR_MS,
        )
    };
    let reading = || {
        later_decision(
            actingcommand_policy::FACT_REFRESH_TASK_ID,
            SchedulingDecisionState::Deferred,
            EligibilityState::True,
            PolicyCode::TaskCooldownActive,
            at + 10 * MINUTE_MS,
        )
    };
    let store_cooling = later_decision(
        "task.store_update",
        SchedulingDecisionState::Deferred,
        EligibilityState::True,
        PolicyCode::TaskCooldownActive,
        at + 30 * MINUTE_MS,
    );
    let store_tasks = ["task.store_update"];

    let waiting = idle_plan_waiting(
        vec![cooling(), clock(), reading(), store_cooling],
        at,
        &["node-a"],
        &store_tasks,
    );
    assert!(!waiting.due_now, "{waiting:?}");
    assert_eq!(
        waiting.next_wake_unix_ms,
        Some(at + 30 * MINUTE_MS),
        "only the store update task's cooldown wakes the waiting instance: {waiting:?}"
    );
    assert_eq!(waiting.reading_due_unix_ms, None, "{waiting:?}");

    let waiting = idle_plan_waiting(
        vec![cooling(), clock(), reading()],
        at,
        &["node-a"],
        &store_tasks,
    );
    assert_eq!(
        (
            waiting.due_now,
            waiting.next_wake_unix_ms,
            waiting.reading_due_unix_ms
        ),
        (false, None, None),
        "no wake for a task the gate would hold: {waiting:?}"
    );

    let none: [&str; 0] = [];
    for pending in [&none[..], &["node-b"][..]] {
        let free = idle_plan_waiting(
            vec![cooling(), clock(), reading()],
            at,
            pending,
            &store_tasks,
        );
        assert_eq!(
            (
                free.due_now,
                free.next_wake_unix_ms,
                free.reading_due_unix_ms
            ),
            (false, Some(at + HOUR_MS), Some(at + 10 * MINUTE_MS)),
            "{pending:?}: {free:?}"
        );
    }
}

const HOUR_MS: u64 = 3_600_000;

/// A decision of `node-a` for `task_id` in `state` and `eligibility`, waiting for `wake` by
/// `reason` (no position 4a among its reasons).
fn later_decision(
    task_id: &str,
    state: SchedulingDecisionState,
    eligibility: EligibilityState,
    reason: PolicyCode,
    wake: u64,
) -> TaskDecision {
    let mut decision = idle_decision(task_id, state, vec![Outcome::new(reason)]);
    decision.eligibility = eligibility;
    decision.next_wake_unix_ms = Some(wake);
    decision
}

/// The idle plan of `node-a` from an evaluation of `decisions` at `at` in which position 4a found
/// the instances `pending` waiting and the store update tasks `store_tasks`, as the policy host
/// records it.
fn idle_plan_waiting(
    decisions: Vec<TaskDecision>,
    at: u64,
    pending: &[&str],
    store_tasks: &[&str],
) -> crate::idle_action::IdlePlan {
    let pending = pending
        .iter()
        .map(|alias| (*alias).to_owned())
        .collect::<BTreeSet<_>>();
    let store_tasks = store_tasks
        .iter()
        .map(|task| (*task).to_owned())
        .collect::<BTreeSet<_>>();
    let mut table = ScheduleExplanationTable::default();
    table.record_held_back(
        &PolicyEvaluation {
            decisions,
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        },
        "sha256:upd1",
        at,
        &|_: &str, _: &str| -> Option<(u64, String)> { None },
        &crate::schedule_explanation::IdleHeldBack::new(),
        crate::application_update::IdleUpdateHold {
            pending: &pending,
            store_tasks: &store_tasks,
        },
    );
    table.idle_plan("node-a").expect("an evaluated plan")
}
