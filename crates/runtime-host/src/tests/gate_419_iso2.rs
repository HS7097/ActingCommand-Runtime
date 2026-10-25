// SPDX-License-Identifier: AGPL-3.0-only

//! ISO-2 (board #419, frozen model `rd-1024/iso-design/model.md` §5.2 R1-R2, §8 row ISO-2;
//! Alice 10-24: one emulator's fault must not stop the whole ActingCommand): a device or kernel
//! failure of one instance stays with that instance. One test per row family of the slice:
//!
//! - a: a fatal input in a scheduled run (E02, A04; sinks D05, D32): the run fails with the
//!   fence's non-fatal failure, so actingd's driver goes on;
//! - b: a fatal input in a startup package run on the instance's worker (A04, the host package
//!   boundary; amplifier D38): no latch;
//! - c: a geometry read that failed after the emulator died (A03, E07): the run fails as a
//!   task-geometry failure;
//! - d: the instance's session thread panics (A01, E11): the run fails, the session is retired
//!   and the next run opens a fresh one;
//! - e: observer and run frame races on the real kernel (E14, E15, E16, A05): the refused commit
//!   or resolve fails only its operation; a kernel code on the Global list keeps halting;
//! - f: an application stop that fails with a kernel code other than A46's (C01, C34): X fenced;
//! - g: the emulator rung's pre-close of a session rung 2 left failed (C02, C01): the restart
//!   goes on;
//! - h: the autostart's pre-close of a kept session fails (C22): the start goes on;
//! - i: an operator restart's pre-close fails (C31): the restart goes on;
//! - j: a pause's session close fails (D54, C32): the pause holds;
//! - k: a handset's session close at its link loss fails (E22): the handset waits;
//! - l: a session close at a lease end fails with its resources confirmed (B35): the lease ends.
//!
//! Every test runs two instances, X with the fault and Y normal, and checks that the Runtime does
//! not latch, that only X is fenced or only X's operation failed, and that Y's run completes
//! after X's failure. X's stuck recovery is off except where the slice's row is the ladder's
//! own (g), so no ladder acts on X while a test reads it. Every close failure is a fatal device
//! error that leaves no resource unconfirmed (the emulator is gone). Every wait returns as soon
//! as what it waits for is recorded; the bound only stops a broken run.

use super::emulator_controller::stopped_state;
use super::gate_369_s6c::{connectivity, control, instance_scope, pause};
use super::gate_398_rd1b::{bound, resolved_handset};
use super::gate_403::{fence_records, status_fence};
use super::instance_workers::{
    all_events, ladder_setup, phases, rung_outcomes, wait_for_ladder_end,
};
use super::*;
use crate::FailureScope;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    ApplicationLifecycleAction, DeviceLinkState, EffectiveDeviceConfiguration,
    EmulatorInstanceAction, InputFrameReference, InstanceStuckRecovery, RecoveryRung,
    RecoveryRungOutcome, RuntimeLifecyclePhase,
};
use actingcommand_execution_kernel::{
    DeviceLinkWatchEvent, ExecutionKernel, ExecutionKernelError, FrameOwner,
};
use actingcommand_host_metrics::NetworkConnectivity;

const WAIT: Duration = Duration::from_secs(120);
/// The default polling interval of the emulator watch (`emulator_control.watch_interval_ms`).
const WATCH_INTERVAL_MS: u64 = 60_000;
const X_ALIAS: &str = "iso2.instance.x";
const Y_ALIAS: &str = "iso2.instance.y";

/// A fatal close failure of a device backend whose resources are confirmed gone: the emulator
/// is gone, and so is what the backend held.
fn gone_close_failure() -> DeviceError {
    DeviceError::fatal("injected close failure of a backend whose emulator is gone")
        .with_resource_summary(actingcommand_device::DeviceResourceQuiescence::Confirmed, 1)
}

/// X's stuck recovery off: its fence waits for a person and no ladder acts on X.
fn x_recovery_off(alias: &str) -> BTreeMap<String, InstanceStuckRecovery> {
    BTreeMap::from([(
        alias.to_owned(),
        InstanceStuckRecovery {
            enabled: false,
            cooldown_secs: 600,
        },
    )])
}

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

/// A read-only observe of `alias`, which opens (and keeps) its device session.
fn observe(host: &RuntimeHost, alias: &str, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.process_request_for_test(
        &runtime_request(
            &ids,
            RuntimeOperation::ObserveReadonly {
                instance_alias: alias.to_owned(),
            },
        ),
        ConnectionId::new(connection).expect("connection"),
    )
    .expect("the observe's receipt")
}

/// X is fenced, every fence record names X, and Y is not fenced.
fn assert_only_x_fenced(host: &RuntimeHost, x_alias: &str, x: InstanceId, y_alias: &str) {
    assert!(
        status_fence(host, x_alias).is_some(),
        "X is fenced in the status"
    );
    assert_eq!(status_fence(host, y_alias), None, "Y is not fenced");
    let events = all_events(host);
    let fences = fence_records(&events);
    assert!(!fences.is_empty(), "X's fence is recorded");
    for (record, _) in &fences {
        assert_eq!(record.links().instance_id(), Some(&x), "{record:?}");
    }
}

/// Closes the host of a test whose injected close failure may still sit on a session X holds:
/// the shutdown's close of that session then fails with the backend's close code
/// (`lifecycle.rs`), which is the fault's own record, not this test's subject.
fn close_after_injected_close_failure(host: RuntimeHost) {
    if let Err(error) = host.close() {
        assert!(error.code().ends_with("_close_failed"), "{error:?}");
    }
}

/// Neither instance is fenced.
fn assert_none_fenced(host: &RuntimeHost, x_alias: &str, y_alias: &str) {
    assert_eq!(status_fence(host, x_alias), None, "X is not fenced");
    assert_eq!(status_fence(host, y_alias), None, "Y is not fenced");
}

/// Two started instances with direct-run packages on a manual clock: X (with the fault) and Y.
struct Pair {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    x: InstanceId,
    x_state: Arc<FakeState>,
    x_package: String,
    y_package: String,
    _root: TempDir,
}

fn pair() -> Pair {
    pair_with(|provider| provider, |config| config)
}

/// `pair` with `provide` applied to the fake provider and `configure` to the host
/// configuration.
fn pair_with(
    provide: impl FnOnce(FakeProvider) -> FakeProvider,
    configure: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> Pair {
    let root = TempDir::new().expect("tempdir");
    let x_state = Arc::new(FakeState::default());
    let y_state = Arc::new(FakeState::default());
    let x_package = super::gate_369s5::direct_run_package(&root, &x_state);
    let y_package = super::gate_369s5::direct_run_package(&root, &y_state);
    let x = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let provider = provide(FakeProvider::from_entries([
        (X_ALIAS.to_owned(), x, Arc::clone(&x_state)),
        (Y_ALIAS.to_owned(), instance_id(), y_state),
    ]));
    let host = RuntimeHost::start(
        configure(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_stuck_recovery(x_recovery_off(X_ALIAS)),
        ),
        Arc::new(provider),
    )
    .expect("runtime host");
    Pair {
        host,
        clock,
        x,
        x_state,
        x_package,
        y_package,
        _root: root,
    }
}

/// A device configuration whose capture is the Nemu IPC (neutral test values): with two such
/// instances, each closes its device session at every lease end and at a pause (the multi-Nemu
/// gate).
fn nemu_configuration(serial: &str) -> EffectiveDeviceConfiguration {
    EffectiveDeviceConfiguration {
        input_backend: "adb_shell_input".to_owned(),
        capture_backend: "nemu_ipc".to_owned(),
        input_adb: "<sealed-test>".to_owned(),
        capture_adb: "<sealed-test>".to_owned(),
        configured_serial: None,
        resolved_serial: serial.to_owned(),
        input_command_timeout_ms: 5_000,
        capture_command_timeout_ms: 5_000,
        capture_timeout_ms: 5_000,
        configured_mumu_root: None,
        configured_capture_dll: None,
    }
}

/// `pair` whose two instances both capture through the Nemu IPC.
fn nemu_pair() -> Pair {
    pair_with(
        |provider| {
            let x = provider
                .resolve(X_ALIAS)
                .expect("X")
                .with_configuration(nemu_configuration("TESTSERIAL0041"));
            let y = provider
                .resolve(Y_ALIAS)
                .expect("Y")
                .with_configuration(nemu_configuration("TESTSERIAL0042"));
            provider
                .with_resolved_for(X_ALIAS, x)
                .with_resolved_for(Y_ALIAS, y)
        },
        |config| config,
    )
}

/// The policy fixture's instance X (with its startup package and ladder) and Y (with a
/// direct-run package), both the policy's configured identities; only X is available to it.
struct Scheduled {
    host: RuntimeHost,
    x: InstanceId,
    x_state: Arc<FakeState>,
    y_package: String,
    request: ContainedTaskRequest,
    _root: TempDir,
}

/// Starts X and Y; `emulator` gives X emulator control (its ladder's rung 3 runs on it);
/// `recovery` keeps X's stuck recovery on.
fn scheduled(emulator: bool, recovery: bool) -> Scheduled {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let x_state = Arc::new(FakeState::default());
    x_state
        .physical_task_geometry
        .store(true, Ordering::Release);
    let y_state = Arc::new(FakeState::default());
    let y_package = super::gate_369s5::direct_run_package(&root, &y_state);
    let x = instance_id();
    let mut facts = policy_facts();
    let mut other_identity = facts.instances[0].clone();
    other_identity.instance_id = Y_ALIAS.to_owned();
    other_identity.available = false;
    facts.instances.push(other_identity);
    let host_config =
        host_config.with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()));
    let host_config = if recovery {
        host_config
    } else {
        host_config.with_stuck_recovery(x_recovery_off(POLICY_INSTANCE_ALIAS))
    };
    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::from_entries([
            (POLICY_INSTANCE_ALIAS.to_owned(), x, Arc::clone(&x_state)),
            (Y_ALIAS.to_owned(), instance_id(), y_state),
        ])),
    )
    .expect("runtime host");
    if emulator {
        bind_fake_emulator(&host, POLICY_INSTANCE_ALIAS, x, &x_state);
    }
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    Scheduled {
        host,
        x,
        x_state,
        y_package,
        request,
        _root: root,
    }
}

impl Scheduled {
    /// Admits X's scheduled run.
    fn admit_x(&self) -> Box<PolicyRunContext> {
        let mut facts = policy_facts();
        let mut other_identity = facts.instances[0].clone();
        other_identity.instance_id = Y_ALIAS.to_owned();
        other_identity.available = false;
        facts.instances.push(other_identity);
        let cycle = self
            .host
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
            .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
            .expect("X's dispatch intent")
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("dispatch reason chain")
            .clone();
        record_policy_approval(&self.host, &intent);
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(
                &intent,
                &reasons,
                &policy_context(&self.host, &intent),
                &self.request,
            )
            .expect("policy admission")
        else {
            panic!("expected a policy run context")
        };
        context
    }
}

/// a (E02, A04; D05, D32): X's emulator stopped under its scheduled run, so the run's input fails
/// with a fatal device error. The run's boundary fences X and the run fails with the fence's
/// non-fatal failure: actingd's driver records a failed run for a non-fatal failure and goes on
/// (`execute_policy_cycle_with`), so it is not stopped. The Runtime does not latch, X alone is
/// fenced, and Y's run completes.
#[test]
fn iso2_a_a_fatal_input_in_a_scheduled_run_fences_x_and_the_driver_goes_on() {
    let fixture = scheduled(false, false);
    let context = fixture.admit_x();
    fixture.x_state.fail_input.store(true, Ordering::Release);

    let error = fixture
        .host
        .run_scheduled_contained_task(&context, &fixture.request)
        .expect_err("X's scheduled run fails");

    assert!(!error.is_fatal(), "the driver would stop: {error:?}");
    assert_eq!(error.code(), HostCode::InstanceResourcesFenced.as_str());
    assert_running(&fixture.host);
    assert_only_x_fenced(&fixture.host, POLICY_INSTANCE_ALIAS, fixture.x, Y_ALIAS);
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4191);
    fixture.host.close().expect("close host");
}

/// b (A04, the host package boundary; D38): X's startup package run fails its input with a fatal
/// device error on X's worker. The package's boundary fences X instead of latching, so the
/// Runtime does not latch (nothing for actingd's monitor setup or a restart to meet), and Y's
/// run completes.
#[test]
fn iso2_b_a_fatal_input_in_a_startup_package_run_does_not_latch() {
    let fixture = scheduled(false, false);
    fixture.x_state.fail_input.store(true, Ordering::Release);

    let _scheduled = fixture
        .host
        .schedule_startup_package_for_test(POLICY_INSTANCE_ALIAS)
        .expect("X's startup package is scheduled");
    wait_until(WAIT, || {
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_some()
            || (status_fence(&fixture.host, POLICY_INSTANCE_ALIAS).is_some()
                && fixture
                    .host
                    .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
                    .expect("X's claims")
                    .0
                    .is_none())
    });

    assert_eq!(
        fixture.x_state.input_count.load(Ordering::Acquire),
        0,
        "every input of X failed"
    );
    assert_running(&fixture.host);
    assert_only_x_fenced(&fixture.host, POLICY_INSTANCE_ALIAS, fixture.x, Y_ALIAS);
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4192);
    fixture.host.close().expect("close host");
}

/// c (A03, E07): X's emulator dies between its run's capture and the geometry read of that
/// frame (`wm size` fails with a fatal device error, nothing left unconfirmed). The run fails as
/// a task-geometry failure, non-fatal; X's device was not left held, so X is not fenced; the
/// Runtime does not latch and Y's run completes.
#[test]
fn iso2_c_a_failed_geometry_read_fails_only_the_run() {
    let fixture = pair();
    let reads = fixture.x_state.geometry_reads.load(Ordering::Acquire);
    // The run's first capture reads its frame's geometry; the host's read of it is the next.
    fixture
        .x_state
        .geometry_error_on_read
        .store(reads + 2, Ordering::Release);
    *fixture
        .x_state
        .geometry_error
        .lock()
        .expect("geometry error") = Some(DeviceError::fatal(
        "injected geometry read failure after the emulator died",
    ));

    let receipt = direct_run(&fixture.host, X_ALIAS, &fixture.x_package, 4193);

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let projection = receipt.error_projection().expect("the run's failure");
    assert!(!projection.fatal, "{receipt:?}");
    assert!(
        fixture
            .x_state
            .geometry_error
            .lock()
            .expect("geometry error")
            .is_none(),
        "the injected geometry read was made"
    );
    assert_running(&fixture.host);
    assert_none_fenced(&fixture.host, X_ALIAS, Y_ALIAS);
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4194);
    fixture.host.close().expect("close host");
}

/// d (A01, E11): X's capture panics inside X's session thread. X's run fails non-fatally and the
/// Runtime does not latch. The failed session is retired: unless the panic left X's resources
/// unconfirmed (then X is fenced, Workflow #403), X's next run opens a fresh capture backend and
/// completes. Y's run completes.
#[test]
fn iso2_d_a_session_thread_panic_retires_the_session_and_the_next_run_opens_a_fresh_one() {
    let fixture = pair();
    fixture
        .x_state
        .panic_capture_once
        .store(true, Ordering::Release);

    let receipt = direct_run(&fixture.host, X_ALIAS, &fixture.x_package, 4195);

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let projection = receipt.error_projection().expect("the run's failure");
    assert!(!projection.fatal, "{receipt:?}");
    assert_running(&fixture.host);
    assert_eq!(
        status_fence(&fixture.host, Y_ALIAS),
        None,
        "Y is not fenced"
    );
    if status_fence(&fixture.host, X_ALIAS).is_none() {
        let opened = fixture.x_state.capture_open_count.load(Ordering::Acquire);
        assert_run_completes(&fixture.host, X_ALIAS, &fixture.x_package, 4196);
        assert!(
            fixture.x_state.capture_open_count.load(Ordering::Acquire) > opened,
            "X's next run opened a fresh session"
        );
    }
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4197);
    fixture.host.close().expect("close host");
}

/// e (E14, E15, E16, A05): the frame races, on the real execution kernel. An observer's capture
/// then a run's capture on X's session: the run's capture drops the observer's pending frame, so
/// the observer's commit is refused (`input_frame_capture_mismatch`); a resolve of that frame is
/// refused (`input_frame_not_committed`); once X's session is retired, a commit finds none
/// (`input_frame_session_missing`). Each refusal fails only its own operation: non-fatal, its
/// scope X's. A kernel code on the Global list (`execution_kernel_closed`) keeps its fatality
/// and stays `Global`.
#[test]
fn iso2_e_a_refused_frame_commit_or_resolve_fails_only_its_operation() {
    let state = Arc::new(FakeState::default());
    let x = instance_id();
    let kernel = ExecutionKernel::new(Arc::new(FakeProvider::one(X_ALIAS, x, state)));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let memory = actingcommand_device::FrameMemoryBudget::new(|| Ok(u64::MAX));
    let observed = *ids.mint_frame_id().expect("frame id").transport();
    let frame = kernel
        .capture_frame_retained_with_registration_guard(
            X_ALIAS,
            Some(observed),
            FrameOwner::Observer,
            (),
            memory.clone(),
        )
        .expect("the observer's capture");
    let reference = InputFrameReference {
        frame_id: observed,
        width: frame.width,
        height: frame.height,
    };
    kernel
        .capture_frame_retained_with_geometry_session_and_registration_guard(
            X_ALIAS,
            Some(*ids.mint_frame_id().expect("frame id").transport()),
            None,
            (),
            memory.clone(),
        )
        .expect("the run's capture");

    let assert_operation_failure = |refusal: &ExecutionKernelError, code: &str| {
        assert_eq!(refusal.code().as_str(), code);
        let failure = RuntimeHostError::instance_operation(
            HostLocation::CommitCaptureInputFrame,
            refusal,
            Some(x),
        );
        assert_eq!(failure.code(), code);
        assert!(!failure.is_fatal(), "{code}");
        assert!(!failure.halts_runtime(), "{code}");
        assert_eq!(
            RuntimeHostError::execution(HostLocation::CommitCaptureInputFrame, refusal)
                .scope_at(Some(x)),
            FailureScope::Instance(x),
            "{code}"
        );
    };
    let replaced = kernel
        .commit_input_frame(X_ALIAS, reference, FrameOwner::Observer)
        .expect_err("the run's capture replaced the observer's frame");
    assert_operation_failure(&replaced, "input_frame_capture_mismatch");
    let unresolved = kernel
        .resolve_input_frame(X_ALIAS, reference)
        .expect_err("the frame was never committed");
    assert_operation_failure(&unresolved, "input_frame_not_committed");
    kernel
        .close_instance(x, actingcommand_device::DeviceCloseAuthority::LocalOnly)
        .expect("X's session is retired");
    let missing = kernel
        .commit_input_frame(X_ALIAS, reference, FrameOwner::Observer)
        .expect_err("no session is left to commit on");
    assert_operation_failure(&missing, "input_frame_session_missing");

    kernel.close().expect("the kernel closes");
    let closed = kernel
        .capture_frame_retained_with_registration_guard(
            X_ALIAS,
            Some(*ids.mint_frame_id().expect("frame id").transport()),
            FrameOwner::Observer,
            (),
            memory,
        )
        .expect_err("a closed kernel opens no session");
    assert_eq!(closed.code().as_str(), "execution_kernel_closed");
    let global = RuntimeHostError::instance_operation(
        HostLocation::CommitCaptureInputFrame,
        &closed,
        Some(x),
    );
    assert!(global.is_fatal());
    assert_eq!(global.scope_at(Some(x)), FailureScope::Global);
    assert!(global.halts_runtime());
}

/// f (C01, C34): X's application stop fails while the command closes X's retained session first,
/// with a kernel code other than A46's (`application_backend_operation_failed`): its capture
/// backend's close fails, then, on its own host, its input backend's. X is fenced with that
/// failure as the cause, the command fails non-fatally (A46 widened), the Runtime does not latch
/// and Y's run completes.
#[test]
fn iso2_f_an_application_stop_failing_with_another_kernel_code_fences_x() {
    for backend in ["capture", "input"] {
        let fixture = pair();
        let mut client = TestClient::connect(&fixture.host);
        if backend == "capture" {
            assert_eq!(
                observe(&fixture.host, X_ALIAS, 4198).state(),
                RuntimeReceiptState::Completed,
                "X's capture backend opens"
            );
            *fixture
                .x_state
                .capture_close_error
                .lock()
                .expect("capture close error") = Some(gone_close_failure());
        } else {
            let (_, token) = client.acquire(X_ALIAS);
            let input = client.request(RuntimeOperation::Input {
                frame: None,
                token: token.clone(),
                action: InputAction::Reset,
            });
            assert_eq!(
                client.send(&input).state(),
                RuntimeReceiptState::Completed,
                "X's input backend opens"
            );
            let release = client.request(RuntimeOperation::ReleaseLease { token });
            assert_eq!(
                client.send(&release).state(),
                RuntimeReceiptState::Completed
            );
            *fixture.x_state.close_error.lock().expect("close error") = Some(gone_close_failure());
        }
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let stop = runtime_request(
            &ids,
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: X_ALIAS.to_owned(),
                holder_id: *ids.mint_holder_id().expect("holder").transport(),
                action: ApplicationLifecycleAction::Stop,
            },
        );

        let receipt = fixture
            .host
            .process_request_for_test(&stop, ConnectionId::new(4199).expect("connection"))
            .expect("the stop's receipt");

        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Failed,
            "{backend}: {receipt:?}"
        );
        let projection = receipt.error_projection().expect("the stop's failure");
        assert!(!projection.fatal, "{backend}: {receipt:?}");
        assert_eq!(
            receipt.outcome().code().as_str(),
            HostCode::InstanceResourcesFenced.as_str(),
            "{backend}"
        );
        let fence = status_fence(&fixture.host, X_ALIAS).expect("X's fence");
        assert_ne!(
            fence.cause.as_str(),
            SharedCode::ApplicationBackendOperationFailed.as_str(),
            "{backend}: a kernel code other than A46's"
        );
        assert_running(&fixture.host);
        assert_only_x_fenced(&fixture.host, X_ALIAS, fixture.x, Y_ALIAS);
        assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4200);
        drop(client);
        close_after_injected_close_failure(fixture.host);
    }
}

/// g (C02, C01): X's scheduled run fails and hands X to its ladder. Rung 2's application stop
/// fails as it closes X's retained session (a capture backend whose emulator is gone), which
/// fences X (A46 widened) and leaves the session's close failed; rung 3's emulator restart
/// pre-closes that session, and the close fails again with nothing left unconfirmed. The restart
/// goes on: its Stop and Start run. The Runtime does not latch, and Y's run completes.
#[test]
fn iso2_g_a_failed_pre_close_in_the_emulator_rung_lets_the_restart_go_on() {
    let fixture = scheduled(true, true);
    let context = fixture.admit_x();
    fixture
        .x_state
        .unknown_capture
        .store(true, Ordering::Release);
    fixture
        .x_state
        .fail_application
        .store(true, Ordering::Release);
    fixture.x_state.observed_open.store(true, Ordering::Release);
    *fixture
        .x_state
        .capture_close_error
        .lock()
        .expect("capture close error") = Some(gone_close_failure());

    let error = fixture
        .host
        .run_scheduled_contained_task(&context, &fixture.request)
        .expect_err("X's scheduled run fails");
    assert!(!error.is_fatal(), "{error:?}");
    wait_for_ladder_end(&fixture.host, POLICY_INSTANCE_ALIAS);

    let rungs = rung_outcomes(&all_events(&fixture.host));
    assert!(
        rungs.contains(&(
            RecoveryRung::ApplicationRestart,
            RecoveryRungOutcome::Failed,
            Some(HostCode::InstanceResourcesFenced.as_str().to_owned()),
        )),
        "rung 2's failed stop fenced X: {rungs:?}"
    );
    let actions = fixture
        .x_state
        .emulator_actions
        .lock()
        .expect("emulator actions")
        .clone();
    assert_eq!(
        actions,
        [EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start],
        "the emulator rung went on past its failed pre-close"
    );
    assert_running(&fixture.host);
    assert_eq!(
        status_fence(&fixture.host, Y_ALIAS),
        None,
        "Y is not fenced"
    );
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4201);
    close_after_injected_close_failure(fixture.host);
}

/// A host whose X is emulator-controlled and configured for the autostart, beside Y.
fn emulator_pair() -> Pair {
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let fixture = pair_with(
        |provider| provider,
        |config| {
            config
                .with_emulator_autostart(vec![X_ALIAS.to_owned()])
                .with_network_connectivity_for_test(read)
        },
    );
    fixture.x_state.observed_open.store(true, Ordering::Release);
    bind_fake_emulator(&fixture.host, X_ALIAS, fixture.x, &fixture.x_state);
    fixture
}

/// h (C22; EMU-W's restart path): X's emulator stopped behind the Runtime's back while X's
/// session stayed open. The watch's poll finds it stopped and its autostart claim's start
/// pre-closes that session; the close fails with nothing left unconfirmed, and the start goes
/// on: its Start runs and the autostart reports it started. The Runtime does not latch, and Y's
/// run completes.
#[test]
fn iso2_h_a_failed_pre_close_in_the_autostart_lets_the_start_go_on() {
    let fixture = emulator_pair();
    assert_eq!(
        observe(&fixture.host, X_ALIAS, 4202).state(),
        RuntimeReceiptState::Completed,
        "X's session opens"
    );
    *fixture
        .x_state
        .capture_close_error
        .lock()
        .expect("capture close error") = Some(gone_close_failure());
    // The poll's read and the claim's read before its Start find the emulator stopped.
    fixture.x_state.emulator.script_read(stopped_state());
    fixture.x_state.emulator.script_read(stopped_state());

    fixture
        .host
        .start_emulator_watch()
        .expect("the watch starts");
    fixture.clock.advance(WATCH_INTERVAL_MS);
    fixture
        .host
        .await_emulator_watch_polls_for_test(1, WAIT)
        .expect("the first poll");
    let lines = fixture
        .host
        .emulator_autostart_reports()
        .await_lines_for_test(
            |lines| {
                lines.iter().any(|line| {
                    matches!(
                        line.code().as_str(),
                        "emulator_autostart_started" | "emulator_autostart_failed"
                    )
                })
            },
            WAIT,
        )
        .expect("the autostart reports");

    assert!(
        lines
            .iter()
            .any(|line| line.code().as_str() == "emulator_autostart_started"),
        "the start went on past its failed pre-close: {lines:?}"
    );
    assert!(
        fixture
            .x_state
            .emulator_actions
            .lock()
            .expect("emulator actions")
            .contains(&EmulatorInstanceAction::Start),
        "X's Start ran"
    );
    assert_running(&fixture.host);
    assert_eq!(
        status_fence(&fixture.host, Y_ALIAS),
        None,
        "Y is not fenced"
    );
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4203);
    close_after_injected_close_failure(fixture.host);
}

/// i (C31): an operator's restart of X pre-closes X's retained session, and the close fails with
/// nothing left unconfirmed. The restart goes on: its Stop and Start run and the receipt
/// completes. The Runtime does not latch, and Y's run completes.
#[test]
fn iso2_i_a_failed_pre_close_in_an_operator_restart_lets_the_restart_go_on() {
    let fixture = emulator_pair();
    assert_eq!(
        observe(&fixture.host, X_ALIAS, 4204).state(),
        RuntimeReceiptState::Completed,
        "X's session opens"
    );
    *fixture
        .x_state
        .capture_close_error
        .lock()
        .expect("capture close error") = Some(gone_close_failure());

    let receipt = control(
        &fixture.host,
        X_ALIAS,
        EmulatorInstanceAction::Restart,
        4205,
    );

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let actions = fixture
        .x_state
        .emulator_actions
        .lock()
        .expect("emulator actions")
        .clone();
    assert!(
        actions.contains(&EmulatorInstanceAction::Stop)
            && actions.contains(&EmulatorInstanceAction::Start),
        "the restart's Stop and Start ran: {actions:?}"
    );
    assert_running(&fixture.host);
    assert_none_fenced(&fixture.host, X_ALIAS, Y_ALIAS);
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4206);
    close_after_injected_close_failure(fixture.host);
}

/// j (D54, C32): X and Y both capture through the Nemu IPC, so a pause closes X's session. The
/// close fails with nothing left unconfirmed; the pause holds and its receipt completes. X is
/// not fenced, the Runtime does not latch, and Y's run completes.
#[test]
fn iso2_j_a_failed_session_close_at_a_pause_lets_the_pause_hold() {
    let fixture = nemu_pair();
    assert_eq!(
        observe(&fixture.host, X_ALIAS, 4207).state(),
        RuntimeReceiptState::Completed,
        "X's session opens"
    );
    *fixture
        .x_state
        .capture_close_error
        .lock()
        .expect("capture close error") = Some(gone_close_failure());

    let receipt = pause(&fixture.host, instance_scope(X_ALIAS), 4208);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(
        fixture.x_state.capture_close_count.load(Ordering::Acquire) >= 1,
        "the pause closed X's session"
    );
    assert_running(&fixture.host);
    assert_none_fenced(&fixture.host, X_ALIAS, Y_ALIAS);
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4209);
    close_after_injected_close_failure(fixture.host);
}

/// k (E22): handset X leaves `device` for `offline`; closing its retained session fails with
/// nothing left unconfirmed. The handset waits as after a clean close (`device_link_waiting`),
/// X is not fenced, the Runtime does not latch, and the other instance's run completes.
#[test]
fn iso2_k_a_failed_session_close_at_a_handset_link_loss_lets_the_handset_wait() {
    const SERIAL: &str = "TESTSERIAL0043";
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    let other_state = Arc::new(FakeState::default());
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let watches = Arc::new(FakeLinkWatches::default());
    let handset = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_policy_inputs(PolicyInputSnapshot::new(
            pending_policy_facts(),
            pending_policy_resources(),
        )),
        Arc::new(
            FakeProvider::from_entries([
                (
                    POLICY_INSTANCE_ALIAS.to_owned(),
                    handset,
                    Arc::clone(&state),
                ),
                (
                    POLICY_INSTANCE_ALIAS_B.to_owned(),
                    instance_id(),
                    other_state,
                ),
            ])
            .with_device_endpoints()
            .with_resolved_for(
                POLICY_INSTANCE_ALIAS,
                resolved_handset(
                    POLICY_INSTANCE_ALIAS,
                    handset,
                    bound(POLICY_INSTANCE_ALIAS, SERIAL),
                ),
            )
            .with_link_watch(watches),
        ),
    )
    .expect("runtime host with a handset");
    let list = |link: DeviceLinkState| {
        host.device_link_event_for_test(DeviceLinkWatchEvent::Links(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            link,
        )])))
        .expect("the host applies the list");
    };
    list(DeviceLinkState::Device);
    assert_eq!(
        observe(&host, POLICY_INSTANCE_ALIAS, 4210).state(),
        RuntimeReceiptState::Completed,
        "the handset's session opens"
    );
    *state
        .capture_close_error
        .lock()
        .expect("capture close error") = Some(gone_close_failure());

    list(DeviceLinkState::Offline);

    assert!(
        state.capture_close_count.load(Ordering::Acquire) >= 1,
        "the link loss closed the handset's session"
    );
    let events = all_events(&host);
    assert!(
        phases(&events).into_iter().any(|(_, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::DeviceLinkObserved { outcome }
                if outcome.code().as_str() == HostCode::DeviceLinkWaiting.as_str()
        )),
        "the handset waits"
    );
    assert_running(&host);
    assert_none_fenced(&host, POLICY_INSTANCE_ALIAS, POLICY_INSTANCE_ALIAS_B);
    assert_run_completes(&host, POLICY_INSTANCE_ALIAS_B, &other_package, 4211);
    close_after_injected_close_failure(host);
}

/// l (B35): X and Y both capture through the Nemu IPC, so X's session closes at its lease end.
/// The close fails with nothing left unconfirmed; the lease ends and its release completes. X is
/// not fenced, the Runtime does not latch, and Y's run completes.
#[test]
fn iso2_l_a_failed_session_close_at_a_lease_end_ends_the_lease() {
    let fixture = nemu_pair();
    let mut client = TestClient::connect(&fixture.host);
    let (_, token) = client.acquire(X_ALIAS);
    let input = client.request(RuntimeOperation::Input {
        frame: None,
        token: token.clone(),
        action: InputAction::Reset,
    });
    assert_eq!(
        client.send(&input).state(),
        RuntimeReceiptState::Completed,
        "X's input backend opens"
    );
    *fixture.x_state.close_error.lock().expect("close error") = Some(gone_close_failure());

    let release = client.request(RuntimeOperation::ReleaseLease { token });
    let receipt = client.send(&release);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(
        fixture.x_state.close_count.load(Ordering::Acquire) >= 1,
        "the lease end closed X's session"
    );
    assert_running(&fixture.host);
    assert_none_fenced(&fixture.host, X_ALIAS, Y_ALIAS);
    assert_run_completes(&fixture.host, Y_ALIAS, &fixture.y_package, 4212);
    drop(client);
    close_after_injected_close_failure(fixture.host);
}
