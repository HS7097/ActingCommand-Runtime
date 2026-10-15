// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 EXEC-3 (forced-compromises mapping row E-02; FCM §2.4 row EXEC-3; coordinator
// ruling 5 of wave 14): a page-graph task is rerun at once by its failure identity (R22), as a
// `linear_steps` task is. The live settlement of a scheduled page-graph run writes the failure
// identity (format v2, contracts/policy-suspension.md, "Failure identity"): its base the
// failure code, its main package the run's `package_id`. A failure scheduled for a retry with
// that identity is admitted again at the next evaluation after its backoff, and after the
// activity's sampled interval, with the reason `failure_retry_immediate`, before its trigger is
// due ("Immediate rerun (R22)").
//
// Wave 14 EXEC-3 rulings 3 and 4 (fix round):
// - a page-graph run that checked its required entry takes that check as its gate and its
//   entry recovery package as its layer ("Scope and step"): a failure of the check is `gate`, a
//   failure while the entry recovery package is open is `pre:<I>`;
// - `contained_task_step_unconfirmed` in the `main` scope, outside the restart segment, pauses
//   by repeating (R21, "Which failures accumulate"): two identical failures pause the pair, and
//   a changed package lifts the pause ("Lifting").

use super::*;
use crate::failure_identity::{
    FailureIdentity, failure_detail_fingerprint, failure_key, package_id_hash,
};

// The fixture catalog's one task.
const TASK: &str = "fixture.observe";
// The fixture catalog's bound procedure.
const PROCEDURE: &str = "procedure.observe";
// The `package_id` of the page-graph package the task's procedure is bound to.
const PACKAGE_ID: &str = "neutral.semantic.task";
// The trigger's next hourly clock occurrence after the first run (the clock predicate's local
// interval, anchored at 0, has an occurrence at POLICY_NOW_UNIX_MS).
const NEXT_OCCURRENCE_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000;
// The default least time between two lift-check measurements of one package.
const LIFT_INTERVAL_MS: u64 = 60_000;
// The entry recovery package of the scope tests.
const RECOVERY_ID: &str = "fixture01.return-home";

#[test]
fn exec3_a_scheduled_page_graph_failure_records_its_identity_and_is_rerun_at_once() {
    let root = TempDir::new().expect("tempdir");
    // A page-graph package (`navigable_route`) whose step is never confirmed.
    let package = neutral_region_retrying_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("contained task request");
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(
            &intent,
            &reasons,
            &policy_context(&host, &intent),
            &request,
        )
        .expect("policy admission")
    else {
        panic!("expected one policy run context")
    };
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the page-graph run fails");
    assert!(!error.is_fatal());

    // The settlement records the run's failure identity.
    let failure = recorded_failure(&host, context.run_id());
    let identity = failure
        .failure_identity
        .as_deref()
        .and_then(FailureIdentity::parse)
        .unwrap_or_else(|| {
            panic!(
                "a page-graph failure records a format v2 failure identity: {:?}",
                failure.failure_identity
            )
        });
    assert_eq!(identity.base, error.code());
    assert_eq!(identity.main.id_hash, package_id_hash(PACKAGE_ID));
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );

    // The next evaluation once the backoff and the activity's sampled interval after the first
    // admission have both passed (R22: the activity budgets and windows still apply) reruns the
    // task at once, before the trigger's next occurrence.
    let rerun_at = rerun_time(&failure, &context);
    assert!(
        rerun_at < NEXT_OCCURRENCE_UNIX_MS,
        "the rerun comes before the trigger is due: {rerun_at}"
    );
    advance_manual_clock_to(&clock, rerun_at);
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: rerun_at,
                monotonic_ms: rerun_at,
            },
            8,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy");
    let evaluation = cycle.evaluation.expect("an evaluation");
    let rerun = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("the failed task is rerun: {evaluation:#?}"));
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == rerun.reason_chain_id)
        .expect("the rerun's reason chain");
    assert!(
        chain.reasons.iter().any(|reason| {
            reason
                .outcome
                .is(actingcommand_policy::PolicyCode::FailureRetryImmediate)
        }),
        "the rerun is the immediate rerun: {chain:#?}"
    );
    host.close().expect("close runtime host");
}

// Ruling 4 (R21): a page-graph step unconfirmed after all its retries, in the `main` scope and
// outside the restart segment, accumulates. Two identical failures (escalation threshold 2)
// pause the pair: the second writes the first's identity again and counts it twice. The pause
// keeps the pair when it is due again, and the main package changed in place lifts it at the
// first due evaluation after the lift check's interval.
#[test]
fn exec3_two_identical_page_graph_step_failures_pause_the_pair_and_a_changed_package_lifts_it() {
    let root = TempDir::new().expect("tempdir");
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(
        &package_path,
        neutral_region_retrying_contained_task_package(),
    )
    .expect("write package");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start_bound(&root, &clock, &package_path);
    host.activate_policy_catalog(&escalating_policy_sources())
        .expect("activate policy catalog");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("contained task request");

    // The first failure accumulates: its identity's last segment names its error frame, not its
    // decision.
    let (first_context, first_error) =
        failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
    assert_eq!(first_error.code(), "contained_task_step_unconfirmed");
    let first = recorded_failure(&host, first_context.run_id());
    let first_identity = first
        .failure_identity
        .clone()
        .expect("the first failure's identity");
    assert!(
        FailureIdentity::parse(&first_identity).is_some_and(|identity| identity.accumulates()),
        "a main-scope page-graph step failure accumulates: {first_identity}"
    );
    assert_eq!(first.consecutive_same_error, 1);
    assert_eq!(first.disposition, PolicyFailureDisposition::RetryScheduled);

    // The same failure at the immediate rerun pauses the pair.
    let rerun_at = rerun_time(&first, &first_context);
    let (second_context, second_error) = failed_scheduled_run(&host, &clock, &request, rerun_at, 8);
    assert_eq!(second_error.code(), first_error.code());
    let second = recorded_failure(&host, second_context.run_id());
    assert_eq!(
        second.failure_identity.as_deref(),
        Some(first_identity.as_str()),
        "the same problem keeps the first identity"
    );
    assert_eq!(second.consecutive_same_error, 2);
    assert_eq!(second.disposition, PolicyFailureDisposition::PausedTask);

    // The pause keeps the pair when it is due again, and the package changed in place lifts it.
    assert_paused(
        &pair_state(&host, &clock, NEXT_OCCURRENCE_UNIX_MS, 31),
        "the package is unchanged",
    );
    fs::write(&package_path, neutral_contained_task_package(false))
        .expect("change the package in place");
    assert_lifted(
        &pair_state(
            &host,
            &clock,
            NEXT_OCCURRENCE_UNIX_MS + LIFT_INTERVAL_MS + 2_000,
            32,
        ),
        "the package changed",
    );
    host.close().expect("close runtime host");
}

// Ruling 3, gate scope: a page-graph run whose required entry is not shown and that has no entry
// recovery package fails in its entry check, before the entry started the package. That check is
// the run's gate: the failure key names the scope `gate` and the step `entry`.
#[test]
fn exec3_a_page_graph_entry_check_failure_is_in_the_gate_scope() {
    let root = TempDir::new().expect("tempdir");
    // The target's home is blue; the screen shows its other page (red).
    let package_path = root.path().join("target.zip");
    fs::write(
        &package_path,
        explicit_home_contained_task_package(false, "fixture01.target", [0, 0, 255], [255, 0, 0]),
    )
    .expect("write the target package");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start_bound(&root, &clock, &package_path);
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("contained task request");
    let (context, error) = failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
    assert_eq!(error.code(), "contained_task_home_recovery_binding_missing");

    let facts = run_facts(&host, context.run_id());
    assert!(
        facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::EntryRecoveryDecision { required: true }
        )),
        "the run checked its required entry: {facts:#?}"
    );
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::EntryRecoveryPackageAdmitted { .. }
                | TaskSemanticFact::StepStarted { .. }
        )),
        "no package opened and no step started: {facts:#?}"
    );
    let failure = recorded_failure(&host, context.run_id());
    assert_failure_key(&failure, &error, "gate", "entry");
    host.close().expect("close runtime host");
}

// Ruling 3, layer scope: a page-graph run whose required entry is not shown opens its entry
// recovery package, whose own step then fails. That package is the run's layer: the failure key
// names the scope `pre:<I>`, `I` the id hash of the package's `package_id`, and the package's
// last started step.
#[test]
fn exec3_a_page_graph_entry_recovery_failure_is_in_its_package_layer_scope() {
    let root = TempDir::new().expect("tempdir");
    // The target's home is blue; the screen shows red, which the recovery package knows as its
    // other page. Its step to its home (yellow) never sees that page.
    let package_path = root.path().join("target.zip");
    let recovery_path = root.path().join("return-home.zip");
    fs::write(
        &package_path,
        explicit_home_contained_task_package(false, "fixture01.target", [0, 0, 255], [255, 0, 0]),
    )
    .expect("write the target package");
    fs::write(
        &recovery_path,
        explicit_home_contained_task_package(false, RECOVERY_ID, [255, 255, 0], [255, 0, 0]),
    )
    .expect("write the recovery package");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start_bound(&root, &clock, &package_path);
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| {
            request.with_recovery(ContainedTaskRecoveryBinding::new(
                recovery_path.to_string_lossy().into_owned(),
            )?)
        })
        .expect("contained task request with its recovery package");
    let (context, error) = failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);

    let facts = run_facts(&host, context.run_id());
    assert!(
        facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::EntryRecoveryPackageAdmitted { package_label, .. }
                if package_label == RECOVERY_ID
        )),
        "the run opened its entry recovery package: {facts:#?}"
    );
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::EntryRecoveryCompleted { .. }
                | TaskSemanticFact::EntryTargetDisposition {
                    disposition: TaskEntryTargetDisposition::Started,
                    ..
                }
        )),
        "the package did not complete and the target did not start: {facts:#?}"
    );
    // Every step of the run is the recovery package's: the target never started.
    let step = facts
        .iter()
        .rev()
        .find_map(|fact| match fact {
            TaskSemanticFact::StepStarted {
                operation_label, ..
            } => Some(operation_label.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "entry".to_owned());
    let failure = recorded_failure(&host, context.run_id());
    assert_failure_key(
        &failure,
        &error,
        &format!("pre:{}", package_id_hash(RECOVERY_ID)),
        &step,
    );
    host.close().expect("close runtime host");
}

// The fixture catalog, its task paused by its second identical failure (escalation threshold 2).
fn escalating_policy_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("policy tasks");
    tasks["tasks"][0]["on_failure"]["escalation_threshold"] = serde_json::json!(2);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("policy task bytes");
    sources
}

// The fixture manifest with the task's procedure bound at `package`, as actingd binds a
// scheduled procedure (`scheduled_execution.package_path`), so a lift check can measure it.
fn manifest_at(package: &Path) -> ProcedureManifest {
    ProcedureManifest::new([PROCEDURE, "procedure.observe-b", "procedure.detect"].map(
        |procedure_ref| {
            let binding = ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding");
            if procedure_ref == PROCEDURE {
                binding.with_package_path(package.to_string_lossy())
            } else {
                binding
            }
        },
    ))
    .expect("procedure manifest")
}

// A host on the test's clock with the task's procedure bound at `package` and one fixture
// simulation instance.
fn start_bound(root: &TempDir, clock: &Arc<ManualRuntimeClock>, package: &Path) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(manifest_at(package)),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id(),
                Arc::new(FakeState::default()),
            )
            .fixture_simulation(),
        ),
    )
    .expect("runtime host")
}

// Evaluates at `at` (the Runtime clock moved there first), admits the task's dispatch and runs
// it as a scheduled run that fails without poisoning the Runtime.
fn failed_scheduled_run(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    request: &ContainedTaskRequest,
    at: u64,
    seed: u64,
) -> (Box<PolicyRunContext>, RuntimeHostError) {
    advance_manual_clock_to(clock, at);
    let (_, intent, reasons) =
        evaluated_policy_dispatch_at(host, PolicyTrigger::FactsChanged, at, seed);
    assert_eq!(intent.task_id, TASK);
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(&intent, &reasons, &policy_context(host, &intent), request)
        .expect("policy admission")
    else {
        panic!("expected one policy run context")
    };
    let error = host
        .run_scheduled_contained_task(&context, request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{error:?}");
    (context, error)
}

// The failure the policy execution record of `run_id` settled.
fn recorded_failure(
    host: &RuntimeHost,
    run_id: RunId,
) -> actingcommand_contract::PolicyFailureRecord {
    run_events(host, run_id)
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
                    match payload.policy_result() {
                        PolicyExecutionOutcome::Failed { failure } => Some(failure.clone()),
                        PolicyExecutionOutcome::Succeeded { .. } => None,
                    }
                }
                _ => None,
            },
            _ => None,
        })
        .expect("the failed execution record")
}

// The forensic events of `run_id`, in ledger order.
fn run_events(host: &RuntimeHost, run_id: RunId) -> Vec<ProjectedEvent> {
    let mut client = TestClient::connect(host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        },
    );
    drop(client);
    events
}

// The task facts of `run_id`, in ledger order.
fn run_facts(host: &RuntimeHost, run_id: RunId) -> Vec<TaskSemanticFact> {
    run_events(host, run_id)
        .iter()
        .filter_map(projected_task_semantic_fact)
        .cloned()
        .collect()
}

// R22's rerun time: the retry backoff and the activity's sampled interval after the admission of
// `context` have both passed.
fn rerun_time(
    failure: &actingcommand_contract::PolicyFailureRecord,
    context: &PolicyRunContext,
) -> u64 {
    failure
        .retry_at_unix_ms
        .expect("a failure scheduled for a retry has its retry time")
        .max(context.admission().activity.next_eligible_unix_ms)
}

// The failure key (contracts/policy-suspension.md, "Failure identity", K) is the one of `scope`
// and `step`, with the failure's own code and detail′.
fn assert_failure_key(
    failure: &actingcommand_contract::PolicyFailureRecord,
    error: &RuntimeHostError,
    scope: &str,
    step: &str,
) {
    let identity = failure
        .failure_identity
        .as_deref()
        .and_then(FailureIdentity::parse)
        .unwrap_or_else(|| {
            panic!(
                "a page-graph failure records a format v2 failure identity: {:?}",
                failure.failure_identity
            )
        });
    let code = actingcommand_contract::scheduled_failure_code_str(&failure.outcome);
    assert_eq!(code.as_str(), error.code());
    let detail = failure_detail_fingerprint(error.native_detail());
    assert_eq!(
        identity.key,
        failure_key(&code, scope, step, &detail),
        "the failure key names the scope {scope} and the step {step}; main would be {}",
        failure_key(&code, "main", step, &detail)
    );
}

// What one evaluation decided for the task's pair.
struct PairState {
    dispatched: bool,
    // Whether a pause kept it.
    paused: bool,
}

// Evaluates at `at` (the Runtime clock moved there first) and reads the pair's decision.
fn pair_state(host: &RuntimeHost, clock: &ManualRuntimeClock, at: u64, seed: u64) -> PairState {
    advance_manual_clock_to(clock, at);
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy");
    let evaluation = cycle.evaluation.expect("an evaluation");
    let dispatched = evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == TASK && intent.instance_id == POLICY_INSTANCE_ALIAS);
    let paused = evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .any(|reason| reason.outcome.code().as_str() == "policy_task_paused");
    PairState { dispatched, paused }
}

fn assert_paused(state: &PairState, case: &str) {
    assert!(!state.dispatched, "{case}: the pair is not dispatched");
    assert!(state.paused, "{case}: the pause keeps the pair");
}

fn assert_lifted(state: &PairState, case: &str) {
    assert!(state.dispatched, "{case}: the lifted pair is dispatched");
    assert!(!state.paused, "{case}: no pause keeps the pair");
}
