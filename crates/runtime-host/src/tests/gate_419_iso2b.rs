// SPDX-License-Identifier: AGPL-3.0-only

//! ISO-2b (board #419, the ISO-2 acceptance follow-ups 6399693120; frozen model
//! `rd-1024/iso-design/model.md` §5.2 R1, rows E08, E11, E12; outcome-code spec §7, amendment
//! 10-24; Alice 10-24: one emulator's fault must not stop the whole ActingCommand):
//!
//! - a, b: an instance's device session that dies twice in a row, with no capture of the
//!   instance succeeding in between, holds only that instance (it is fenced); one death, or a
//!   death after a capture succeeded again, fails only the run;
//! - c, d, e: the Nemu input check reports an input that the run's deadline, the run's cancel or
//!   the lease stopped as that stop, not as a device failure, and the input path makes it the
//!   run's or the lease's own non-fatal failure of the operation alone;
//! - f: a scheduled run whose session dies once fails only that run: its policy settlement is
//!   an ordinary failure, and its task is not paused.
//!
//! Every test checks that the Runtime does not latch. The session tests a and b run two
//! instances, X with the fault and Y normal, and check that only X is fenced and that Y's run
//! completes.

use super::gate_403::{fence_records, status_fence};
use super::instance_workers::{admit_ladder_run, all_events, ladder_setup};
use super::*;
use crate::FailureScope;
use actingcommand_contract::{EffectiveDeviceConfiguration, InstanceStuckRecovery};
use actingcommand_device::{DeviceResult, InputCheckPhase, InputCheckStop};
use actingcommand_execution_kernel::ExecutionKernelCode;

/// One Nemu input check's answer, and the failure the input path made of a stopped input.
type Checked = (DeviceResult<Duration>, Option<RuntimeHostError>);

const X_ALIAS: &str = "iso2b.instance.x";
const Y_ALIAS: &str = "iso2b.instance.y";

/// The kernel codes of a dead session (`failure_scope::SESSION_DEATH_CODES`), as a fence's cause.
const SESSION_DEATHS: [&str; 5] = [
    "execution_session_panicked",
    "execution_session_state_poisoned",
    "execution_session_closed",
    "execution_session_unavailable",
    "execution_session_response_lost",
];

/// The Runtime holds no fatal state and no shutdown was requested.
fn assert_running(host: &RuntimeHost) {
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "one instance's failure latched the Runtime: {:?}",
        host.fatal_error()
            .expect("runtime health")
            .map(|error| error.daemon_outcome())
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown state"),
        "one instance's failure requested a shutdown"
    );
}

/// `alias`'s direct run of `package`, as a client sends it.
fn direct_run(host: &RuntimeHost, alias: &str, package: &str, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            alias,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.to_owned()).expect("task request"),
        ),
    );
    host.process_request_for_test(&run, ConnectionId::new(connection).expect("connection"))
        .expect("the run's receipt")
}

/// `alias`'s direct run of `package` completes with success.
fn assert_run_completes(host: &RuntimeHost, alias: &str, package: &str, connection: u64) {
    let receipt = direct_run(host, alias, package, connection);
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

/// The code on top of the failure of a capture whose session thread panicked, the death the
/// tests inject: always the kernel's `execution_session_response_lost`, a session death. The
/// panic unwinds the session thread and drops the capture's reply sender unsent, so the waiting
/// capture's reply fails first (`ExecutionSession::capture_frame_retained`); the thread's own
/// `execution_session_panicked` comes from the join after it and is merged in as the secondary.
const RESPONSE_LOST: &str = ExecutionKernelCode::ExecutionSessionResponseLost.as_str();

/// X's direct run whose capture panics inside X's session thread: the run fails, non-fatal,
/// with `code` on top of its receipt.
fn run_with_a_dead_session(fixture: &Pair, connection: u64, code: &str) -> RuntimeReceipt {
    fixture
        .x_state
        .panic_capture_once
        .store(true, Ordering::Release);
    let receipt = direct_run(&fixture.host, X_ALIAS, &fixture.x_package, connection);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let projection = receipt.error_projection().expect("the run's failure");
    assert!(!projection.fatal, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
    receipt
}

/// X's run whose session dies for the first time: it fails only itself, as a capture failure
/// (#411 A7, ISO-2): the kernel's `execution_session_response_lost` on top of its receipt,
/// projected as a non-fatal `capture_failed`. That also says the death's close was confirmed:
/// an unconfirmed one would have fenced X (#403) and ended the run with the fence's code.
fn first_death(fixture: &Pair, connection: u64) {
    let receipt = run_with_a_dead_session(fixture, connection, RESPONSE_LOST);
    assert_eq!(
        receipt.error_projection().expect("the run's failure").code,
        RuntimeErrorCode::CaptureFailed,
        "{receipt:?}"
    );
}

/// Two started instances with direct-run packages on a manual clock: X (with the fault) and Y.
/// X's stuck recovery is off, so its fence waits for a person and no ladder acts on X.
struct Pair {
    host: RuntimeHost,
    x: InstanceId,
    x_state: Arc<FakeState>,
    x_package: String,
    y_package: String,
    _root: TempDir,
}

fn pair() -> Pair {
    let root = TempDir::new().expect("tempdir");
    let x_state = Arc::new(FakeState::default());
    let y_state = Arc::new(FakeState::default());
    let x_package = super::gate_369s5::direct_run_package(&root, &x_state);
    let y_package = super::gate_369s5::direct_run_package(&root, &y_state);
    let x = instance_id();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0)))
            .with_stuck_recovery(BTreeMap::from([(
                X_ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )])),
        Arc::new(FakeProvider::from_entries([
            (X_ALIAS.to_owned(), x, Arc::clone(&x_state)),
            (Y_ALIAS.to_owned(), instance_id(), y_state),
        ])),
    )
    .expect("runtime host");
    Pair {
        host,
        x,
        x_state,
        x_package,
        y_package,
        _root: root,
    }
}

/// a (R1, rows E11, E12; spec §7 amendment 10-24): X's session dies at a run's capture, and the
/// fresh session of X's next run dies again before any capture of X succeeded. The first death
/// fails only its run; the second fences X, caused by the dead session, and its run fails with
/// the fence's non-fatal failure. The Runtime does not latch, only X is fenced, and Y's run
/// completes.
#[test]
fn iso2b_a_a_second_session_death_in_a_row_fences_only_x() {
    let fixture = pair();

    first_death(&fixture, 4401);
    assert_eq!(
        status_fence(&fixture.host, X_ALIAS),
        None,
        "one death fails only its run"
    );
    // The second death ends the run with the fence's non-fatal failure.
    run_with_a_dead_session(
        &fixture,
        4402,
        crate::codes::HostCode::InstanceResourcesFenced.as_str(),
    );

    let fence = status_fence(&fixture.host, X_ALIAS).expect("X is fenced");
    assert!(
        SESSION_DEATHS.contains(&fence.cause.as_str()),
        "the fence's cause is the dead session: {fence:?}"
    );
    assert_eq!(
        status_fence(&fixture.host, Y_ALIAS),
        None,
        "Y is not fenced"
    );
    let events = all_events(&fixture.host);
    let fences = fence_records(&events);
    assert!(!fences.is_empty(), "X's fence is recorded");
    for (record, _) in &fences {
        assert_eq!(record.links().instance_id(), Some(&fixture.x), "{record:?}");
    }
    assert_running(&fixture.host);
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4403);
    fixture.host.close().expect("close host");
}

/// b (R1): "in a row" counts deaths with no succeeding capture of X between them. X's session
/// dies, X's next run completes (its fresh session captured), and X's session dies again: that
/// death is a first one, so X is not fenced and only its run fails. The Runtime does not latch
/// and Y's run completes.
#[test]
fn iso2b_b_a_capture_between_two_deaths_keeps_x_unfenced() {
    let fixture = pair();

    first_death(&fixture, 4404);
    assert_run_completes(&fixture.host, X_ALIAS, &fixture.x_package, 4405);
    first_death(&fixture, 4406);

    assert_eq!(
        status_fence(&fixture.host, X_ALIAS),
        None,
        "a death after a successful capture is a first one"
    );
    assert_eq!(
        status_fence(&fixture.host, Y_ALIAS),
        None,
        "Y is not fenced"
    );
    assert_running(&fixture.host);
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4407);
    fixture.host.close().expect("close host");
}

/// One started instance X whose capture and input are the Nemu IPC (neutral test values), on a
/// manual clock, and a client connection that holds X's lease.
struct NemuLease {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    token: LeaseToken,
    client: TestClient,
    _root: TempDir,
}

fn nemu_lease() -> NemuLease {
    let root = TempDir::new().expect("tempdir");
    let x = instance_id();
    let provider = FakeProvider::one(X_ALIAS, x, Arc::new(FakeState::default()));
    let resolved =
        provider
            .resolve(X_ALIAS)
            .expect("X")
            .with_configuration(EffectiveDeviceConfiguration {
                input_backend: "nemu_ipc".to_owned(),
                capture_backend: "nemu_ipc".to_owned(),
                input_adb: "<sealed-test>".to_owned(),
                capture_adb: "<sealed-test>".to_owned(),
                configured_serial: None,
                resolved_serial: "TESTSERIAL0081".to_owned(),
                input_command_timeout_ms: 5_000,
                capture_command_timeout_ms: 5_000,
                capture_timeout_ms: 5_000,
                configured_mumu_root: None,
                configured_capture_dll: None,
            });
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(provider.with_resolved_for(X_ALIAS, resolved)),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire(X_ALIAS);
    NemuLease {
        host,
        clock,
        token,
        client,
        _root: root,
    }
}

impl NemuLease {
    /// One check of X's Nemu input for a run whose deadline is `deadline_after_ms` from now
    /// (`cancelled`: its client cancelled it), after the clock moved `advance_ms`.
    fn check(
        &self,
        deadline_after_ms: u64,
        cancelled: bool,
        advance_ms: u64,
        phase: InputCheckPhase,
    ) -> Checked {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let run = runtime_request(&ids, RuntimeOperation::Health).request_id();
        self.host
            .nemu_input_check_for_test(
                X_ALIAS,
                &self.token,
                run,
                deadline_after_ms,
                cancelled,
                || self.clock.advance(advance_ms),
                phase,
            )
            .expect("the check")
    }

    /// Checks that the Runtime runs and, with `input_unfenced`, that X is not fenced (a lapsed
    /// lease is the lease sweep's to settle, so test e does not ask it), then closes the host.
    fn close(self, input_unfenced: bool) {
        assert_running(&self.host);
        if input_unfenced {
            assert_eq!(status_fence(&self.host, X_ALIAS), None, "X is not fenced");
        }
        drop(self.client);
        self.host.close().expect("close host");
    }
}

/// The stop the check answered, and the operation's own failure the input path made of it:
/// non-fatal, of the operation alone, halting nothing.
fn stopped_failure((answer, failure): Checked, stop: InputCheckStop) -> RuntimeHostError {
    let answer = answer.expect_err("the check stops the input");
    assert_eq!(answer.input_check_stop(), Some(stop), "{answer:?}");
    let failure = failure.expect("the input's own failure");
    assert!(!failure.is_fatal(), "{:?}", failure.outcome());
    assert!(!failure.halts_runtime(), "{:?}", failure.outcome());
    assert_eq!(
        failure.scope_at(None),
        FailureScope::Operation,
        "{:?}",
        failure.outcome()
    );
    failure
}

/// c (row E08): the run's deadline passes while its Nemu input goes on. The check answers the
/// stop `Deadline`, not a device failure, and the input fails with the run's own
/// `contained_task_deadline_exceeded`, so the run ends as on its deadline. X is not fenced.
#[test]
fn iso2b_c_a_nemu_input_past_the_run_deadline_fails_with_the_run_deadline_code() {
    let fixture = nemu_lease();

    let failure = stopped_failure(
        fixture.check(1_000, false, 2_000, InputCheckPhase::Continue),
        InputCheckStop::Deadline,
    );

    assert_eq!(
        failure.code(),
        RuntimeErrorCode::ContainedTaskDeadlineExceeded.as_str()
    );
    assert_eq!(
        failure.projection().code,
        RuntimeErrorCode::ContainedTaskDeadlineExceeded
    );
    fixture.close(true);
}

/// d (row E08): the run's client cancelled it while its Nemu input goes on. The next gesture's
/// check answers the stop `Cancelled` and the input fails with the run's own
/// `contained_task_cancelled`; the final up stays allowed, so a finger that is down is
/// released. X is not fenced.
#[test]
fn iso2b_d_a_cancelled_run_stops_its_nemu_input_with_the_cancel_code() {
    let fixture = nemu_lease();

    let failure = stopped_failure(
        fixture.check(60_000, true, 0, InputCheckPhase::Continue),
        InputCheckStop::Cancelled,
    );
    let (final_up, final_up_failure) = fixture.check(60_000, true, 0, InputCheckPhase::FinalUp);

    assert_eq!(
        failure.code(),
        RuntimeErrorCode::ContainedTaskCancelled.as_str()
    );
    assert!(final_up.is_ok(), "the final up is allowed: {final_up:?}");
    assert!(final_up_failure.is_none());
    fixture.close(true);
}

/// e (row E08): the input's lease lapses while its Nemu input goes on (the run's deadline is
/// later). The check answers the stop `Lease` and the input fails with the scheduler's own
/// refusal of the lease, non-fatal. What the lapsed lease does to X is the lease sweep's.
#[test]
fn iso2b_e_a_lapsed_lease_stops_the_nemu_input_with_the_lease_refusal() {
    let fixture = nemu_lease();

    let failure = stopped_failure(
        fixture.check(7_200_000, false, 3_600_000, InputCheckPhase::Continue),
        InputCheckStop::Lease,
    );

    // The lapsed lease's own refusal: `lease_expired` (the sweep defers an expiry while the
    // input's step is in flight, so the lease is still there to refuse), or `lease_missing`.
    assert!(
        [
            RuntimeErrorCode::LeaseExpired,
            RuntimeErrorCode::LeaseMissing
        ]
        .iter()
        .any(|code| failure.code() == code.as_str()),
        "the lapsed lease's own refusal: {:?}",
        failure.outcome()
    );
    assert_eq!(
        failure.operation(),
        crate::codes::HostLocation::ExecuteInputBackend.as_str()
    );
    fixture.close(false);
}

/// f (R1, row A01; ISO-2: a dead session fails only the run): X's scheduled run meets its
/// session dead at its first capture. The run fails only itself, non-fatal, with the kernel's
/// `execution_session_response_lost`, and the policy settles it as an ordinary failure: class
/// `Recoverable` and a retry scheduled, so the task is not paused for a person (a session-death
/// code is no longer on the contract's always-severe list). X is not fenced and the Runtime does
/// not latch. X's stuck recovery is off, so no ladder acts on X while the test reads it.
#[test]
fn iso2b_f_a_single_session_death_does_not_pause_the_scheduled_task() {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        host_config.with_stuck_recovery(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            InstanceStuckRecovery {
                enabled: false,
                cooldown_secs: 600,
            },
        )])),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let context = admit_ladder_run(&host, &request);
    state.panic_capture_once.store(true, Ordering::Release);

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("X's scheduled run fails");

    assert!(!error.is_fatal(), "{:?}", error.outcome());
    assert_eq!(error.code(), RESPONSE_LOST);
    let failures = all_events(&host)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::ExecutionRecorded(execution)) => {
                match execution.policy_result() {
                    PolicyExecutionOutcome::Failed { failure } => Some(failure.clone()),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [failure] = failures.as_slice() else {
        panic!("one failed policy execution: {failures:?}");
    };
    assert_eq!(
        failure.outcome.code().as_str(),
        RESPONSE_LOST,
        "{failure:?}"
    );
    assert_eq!(
        failure.original_class,
        PolicyFailureClass::Recoverable,
        "{failure:?}"
    );
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled,
        "{failure:?}"
    );
    assert_eq!(
        status_fence(&host, POLICY_INSTANCE_ALIAS),
        None,
        "one death holds nothing"
    );
    assert_running(&host);
    host.close().expect("close host");
}
