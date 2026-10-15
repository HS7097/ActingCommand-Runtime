// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 EXEC-3 (forced-compromises mapping row E-02; FCM §2.4 row EXEC-3; coordinator
// ruling 5 of wave 14): a page-graph task is rerun at once by its failure identity (R22), as a
// `linear_steps` task is. The live settlement of a scheduled page-graph run writes the failure
// identity (format v2, contracts/policy-suspension.md, "Failure identity"): its base the
// failure code, its main package the run's `package_id`. A failure scheduled for a retry with
// that identity is admitted again at the next evaluation after its backoff with the reason
// `failure_retry_immediate`, before its trigger is due ("Immediate rerun (R22)").

use super::*;
use crate::failure_identity::{FailureIdentity, package_id_hash};

// The fixture catalog's one task.
const TASK: &str = "fixture.observe";
// The `package_id` of the page-graph package the task's procedure is bound to.
const PACKAGE_ID: &str = "neutral.semantic.task";
// Two minutes after the failed run: past the retry backoff (30 s) and the activity profile's
// least interval (60 s), inside the same activity window, long before the trigger's next hourly
// clock occurrence.
const RERUN_AT_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 120_000;

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
    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    let failure = events
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
        .expect("the failed execution record");
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
    drop(client);

    // The next evaluation after the backoff reruns the task at once.
    advance_manual_clock_to(&clock, RERUN_AT_UNIX_MS);
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: RERUN_AT_UNIX_MS,
                monotonic_ms: RERUN_AT_UNIX_MS,
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
