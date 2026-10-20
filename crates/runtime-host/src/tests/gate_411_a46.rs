// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A46 (0.12.8; the 10-21 06:00 incident): one instance's application-backend failure must
//! not stop the host. On the production host an instance's Android hung, its recovery ladder's
//! application-restart rung sent `application.stop`, the stop failed with a fatal device error
//! whose resources were confirmed (`application_backend_operation_failed`), and the whole Runtime
//! stopped; every restart that met the same instance stopped again, at the policy monitor's setup
//! or at its closeout. Requirements of the slice brief:
//!
//! - T1 (requirement 2): in a running host, X's `application.stop` inside a ladder rung fails;
//!   the host stays up, X is fenced with the application-backend failure as the cause, and
//!   instance Y still dispatches;
//! - T2 (requirement 3): X's application backend fails when actingd's policy monitor sets up
//!   right after the start (`wait_policy_work`, then `open_policy_wakes`); the setup goes on,
//!   X is fenced with the cause, and Y dispatches;
//! - T3 (requirement 2, Fail Loud): the closeout path after X's failure does not escalate to a
//!   host shutdown: nothing requests one, the failure is recorded with its code and cause, and the
//!   host closes without a failure when its close is requested.
//!
//! X is the policy fixture's instance with its startup package (so its ladder has an
//! application-restart rung) and an admitted scheduled run whose captures stay unknown, so the run
//! fails and hands X to its ladder; every application command of X's fails with a fatal device
//! error and no unconfirmed resource. Y is a second instance with a direct-run package. Both are
//! the policy's configured identities (the registered aliases); only X is available to it.
//!
//! Existing tests missed this because none failed one instance's application backend while the
//! host and another instance ran: the #403 gates (`gate_403.rs`) fail an application command only
//! with an unconfirmed close (`with_resource_summary(Unconfirmed, ..)`), which the fence already
//! took; the fake's `fail_application` (a fatal error with confirmed resources) was set by no
//! test; every ladder test's application stop succeeds; and actingd's cycle harness, whose fake
//! fails every application command, configures no startup package, so its ladders never reach
//! the stop. Every wait returns as soon as what it waits for is recorded; the bound only stops a
//! broken run.

use super::gate_403::{assert_fence, fence_records, status_fence};
use super::instance_workers::{
    all_events, closed_ledger_events, ladder_finished, ladder_setup, phases,
};
use super::*;
use actingcommand_contract::RuntimeLifecyclePhase;
use actingcommand_contract::codes::SharedCode;

const WAIT: Duration = Duration::from_secs(120);
const FAILING_ALIAS: &str = POLICY_INSTANCE_ALIAS;
const OTHER_ALIAS: &str = "a46.instance.y";

/// The started host of X (`FAILING_ALIAS`) and Y (`OTHER_ALIAS`), with X's scheduled run admitted.
struct TwoInstances {
    root: TempDir,
    host: RuntimeHost,
    failing: InstanceId,
    failing_state: Arc<FakeState>,
    other_package: String,
    context: Box<PolicyRunContext>,
    request: ContainedTaskRequest,
}

/// Starts X and Y, admits X's scheduled run, then lets X's captures stay unknown (the run and
/// each rung run fail) and X's application backend fail every command with a fatal device error
/// whose resources are confirmed, as the hung Android of the incident did.
fn two_instances() -> TwoInstances {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let failing_state = Arc::new(FakeState::default());
    failing_state
        .physical_task_geometry
        .store(true, Ordering::Release);
    let other_state = Arc::new(FakeState::default());
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let failing = instance_id();
    // The policy's configured identities are the registered aliases (both instances); only X is
    // available to the policy.
    let mut facts = policy_facts();
    let mut other_identity = facts.instances[0].clone();
    other_identity.instance_id = OTHER_ALIAS.to_owned();
    other_identity.available = false;
    facts.instances.push(other_identity);
    let host = RuntimeHost::start(
        host_config.with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources())),
        Arc::new(FakeProvider::from_entries([
            (
                FAILING_ALIAS.to_owned(),
                failing,
                Arc::clone(&failing_state),
            ),
            (OTHER_ALIAS.to_owned(), instance_id(), other_state),
        ])),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate policy dispatch");
    let evaluation = cycle.evaluation.as_ref().expect("policy evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == FAILING_ALIAS)
        .expect("X's dispatch intent")
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("dispatch reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(
            &intent,
            &reasons,
            &policy_context(&host, &intent),
            &request,
        )
        .expect("policy admission")
    else {
        panic!("expected a policy run context")
    };
    failing_state.unknown_capture.store(true, Ordering::Release);
    failing_state
        .fail_application
        .store(true, Ordering::Release);
    TwoInstances {
        root,
        host,
        failing,
        failing_state,
        other_package,
        context,
        request,
    }
}

impl TwoInstances {
    /// Runs X's scheduled run, which fails and hands X to its ladder, and returns once the
    /// ladder has ended and released X, or once the Runtime is fatal (the failure this slice
    /// removes, which ends the climb before its end is recorded).
    fn fail_x_and_climb(&self) {
        let error = self
            .host
            .run_scheduled_contained_task(&self.context, &self.request)
            .expect_err("X's scheduled run fails");
        assert!(!error.is_fatal(), "{error:?}");
        wait_until(WAIT, || {
            self.host.fatal_error().expect("runtime health").is_some()
                || (ladder_finished(&all_events(&self.host)).is_some()
                    && self
                        .host
                        .instance_claims_for_test(FAILING_ALIAS)
                        .expect("X's claims")
                        .0
                        .is_none())
        });
        assert!(
            self.failing_state.application_count.load(Ordering::Acquire) >= 1,
            "X's ladder sent its application stop"
        );
    }

    /// X is fenced, and its fence names the application-backend failure as its cause, in the
    /// status and in every fence record, each of which names X.
    fn assert_x_fenced_with_the_cause(&self) {
        let application_failure = SharedCode::ApplicationBackendOperationFailed.as_str();
        let fence = status_fence(&self.host, FAILING_ALIAS).expect("X's fence in the status");
        assert_eq!(fence.cause.as_str(), application_failure, "{fence:?}");
        assert_eq!(status_fence(&self.host, OTHER_ALIAS), None);
        let events = all_events(&self.host);
        let fences = fence_records(&events);
        assert!(!fences.is_empty(), "X's fence is recorded");
        for (record, outcome) in &fences {
            assert_eq!(record.links().instance_id(), Some(&self.failing));
            assert_fence(outcome, self.failing, application_failure);
        }
    }

    /// Y's direct run is dispatched and completes.
    fn assert_y_dispatches(&self) {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let run = runtime_request(
            &ids,
            RuntimeOperation::run_contained_task(
                OTHER_ALIAS,
                ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(self.other_package.clone()).expect("task request"),
            ),
        );
        let receipt = self
            .host
            .process_request_for_test(&run, ConnectionId::new(461).expect("connection"))
            .expect("Y's run receipt");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        assert!(
            matches!(
                receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    ..
                })
            ),
            "{receipt:?}"
        );
    }
}

/// T1 (requirement 2): in a running host, X's `application.stop` inside its ladder's
/// application-restart rung fails with a fatal device error. The host stays up (no fatal state,
/// no shutdown requested), X is fenced with `application_backend_operation_failed` as the cause
/// (and Y is not), and Y's run still dispatches and completes.
#[test]
fn a46_t1_a_failed_application_stop_in_a_ladder_rung_fences_x_and_the_host_stays_up() {
    let fixture = two_instances();
    fixture.fail_x_and_climb();

    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none(),
        "one instance's application-backend failure ended the Runtime"
    );
    assert!(
        !fixture
            .host
            .is_shutdown_requested()
            .expect("shutdown state"),
        "one instance's application-backend failure requested a host shutdown"
    );
    fixture.assert_x_fenced_with_the_cause();
    fixture.assert_y_dispatches();
    fixture.host.close().expect("close host");
}

/// T2 (requirement 3): right after the start, X's application backend fails (its scheduled run's
/// ladder stops X's application) before actingd's policy monitor sets up. The setup goes on as
/// actingd's `monitor_policy` makes it, `wait_policy_work` giving the work and
/// `open_policy_wakes` the wakes, instead of the monitor setup failing; X is fenced with the
/// cause, and Y dispatches.
#[test]
fn a46_t2_monitor_setup_after_x_application_failure_goes_on_and_y_dispatches() {
    let fixture = two_instances();
    fixture.fail_x_and_climb();

    let setup = fixture.host.wait_policy_work();
    assert!(
        matches!(setup, Ok(Some(_))),
        "the policy monitor's setup failed on one instance's application-backend failure: {:?}",
        setup.as_ref().err()
    );
    drop(setup);
    let wakes = fixture
        .host
        .open_policy_wakes()
        .expect("the policy monitor's setup opens the wakes");
    fixture.assert_x_fenced_with_the_cause();
    fixture.assert_y_dispatches();
    drop(wakes);
    fixture.host.close().expect("close host");
}

/// T3 (requirement 2, Fail Loud): after X's application-backend failure, the closeout path does
/// not escalate to a host shutdown. Nothing requests a shutdown and the Runtime holds no fatal
/// state, which are what actingd's policy monitor ends on; the failure is recorded with its code
/// and cause (X's fence, caused by `application_backend_operation_failed`); the requested close
/// ends the host without a failure, and the closed ledger records one shutdown request, the
/// close's own, after the fence.
#[test]
fn a46_t3_the_closeout_after_x_application_failure_requests_no_host_shutdown() {
    let fixture = two_instances();
    fixture.fail_x_and_climb();

    assert!(
        !fixture
            .host
            .is_shutdown_requested()
            .expect("shutdown state"),
        "the closeout after one instance's application-backend failure requested a host shutdown"
    );
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none(),
        "the closeout after one instance's application-backend failure left the Runtime fatal"
    );
    fixture.assert_x_fenced_with_the_cause();
    let last_fence = fence_records(&all_events(&fixture.host))
        .last()
        .map(|(record, _)| record.sequence())
        .expect("X's fence record");

    let TwoInstances { root, host, .. } = fixture;
    host.close()
        .expect("the requested close ends the host without a failure");
    let events = closed_ledger_events(root.path());
    let shutdowns = phases(&events)
        .into_iter()
        .filter(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::ShutdownRequested))
        .map(|(event, _)| event.sequence())
        .collect::<Vec<_>>();
    let [shutdown] = shutdowns.as_slice() else {
        panic!("one shutdown request, the close's own: {shutdowns:?}");
    };
    assert!(*shutdown > last_fence, "{shutdown} after {last_fence}");
}
