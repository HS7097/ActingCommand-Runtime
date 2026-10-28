// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A50 (the production host on 10-21 at 17:00, after an emulator restart). The game was
//! not running; the Nemu capture looks up the game's display by its package, found none, and
//! the Runtime reported only `capture_backend_open_failed`, without a cause. The operator's
//! `emulator restart` answered `startup_package: none`, so the game never started and every
//! self-check failed. During the restart the old VM still held the port, and the new instance
//! came up on the port after it. Requirements of the frozen model (coordinator, 10-21 17:2x):
//!
//! - T1 (item 1): a capture open whose package display does not exist (Nemu answered no display)
//!   fails `capture_backend_open_failed` with the cause `display_missing`, the package and the
//!   instance index;
//! - T2 (item 1): a capture open whose connection to the emulator failed has its own cause,
//!   `connection_failed`;
//! - T3 (item 2): an operator restart of an unpaused instance that has a startup package queues
//!   it, also when the restart's preparation failed because the game was not running;
//! - T4 (item 2; #411 A71 replaced its rule): the same on a paused instance queues the startup
//!   package too, its claim waiting gated behind the pause, and the answer says which pause holds
//!   it (`startup_package_withheld: instance_paused`);
//! - T5 (item 3): a restart whose Stop's readiness wait expired launches only after the status
//!   reads stopped (the reads answer running, running, stopped);
//! - T6 (item 3): the old VM never stops within the configured bound (a non-default
//!   `emulator_control.restart_exit_wait_ms`): the restart fails
//!   `emulator_restart_previous_instance_running` and launches nothing;
//! - T7 (item 4, the A38 follow-up): `prepare_recovery_connection` (here a fence's own recheck)
//!   first reads the emulator status; the emulator runs while the Runtime holds it stopped, so
//!   the device is recognized again and the recheck passes, instead of an open failing on the
//!   stale binding.
//!
//! The device and the emulator are fakes (the scripted controller; the fake provider's capture
//! open answers the typed failure a Nemu open reports); the restart's wait runs on a test clock
//! that a wait moves forward, so nothing waits for time. Existing tests missed this: every
//! capture open failure they scripted carried no cause, every operator start whose preparation
//! failed asserted no startup package (that was the rule), every restart whose Stop's wait
//! expired had the status already reading stopped, and no recovery preparation read the
//! emulator status (only a self-check on an operator claim did, A48).
//!
//! #411 A71 (#411 comment 6380024768, the production host on 10-24 at 12:50): an instance was
//! paused, then `ac_emulator start` started its emulator; the answer said
//! `startup_package_withheld: instance_paused`, and after the resume the startup package never
//! ran. A pause holds only dispatches (emu-watch, Alice's ruling 35), so the start queues the
//! package behind the pause, as a configured start does:
//!
//! - T8: a start during the instance's pause queues the startup package, nothing runs under the
//!   pause, and after the resume the package runs once.
//!
//! W2-B (0.12.26; brief rd-1028/brief-wave2a.md W2-B; review rd-1028/review-w1-6.md M1): a
//! deploy that restarts the Runtime between that start and the resume dropped the queued claim
//! (`lease.queue_disconnected`), and the resume then ran no startup package. Required state:
//!
//! - T9: a start during the instance's pause, then a restart, then the resume: the restart queues
//!   the startup package again behind the restored pause (kept in `host.idle_state`), nothing runs
//!   under the pause, the package runs once after the resume, and a further restart queues it no
//!   more. Without a restart T8 holds unchanged.

use super::emulator_controller::{
    EmulatorCall, FAKE_EMULATOR_PORT, ScriptedControl, running_state, stopped_state,
};
use super::gate_369_s6c::GATE_WAIT;
use super::gate_387_d1c::ladder_manifest;
use super::gate_403::status_fence;
use super::instance_workers::{all_events, phases, wait_until};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EmulatorInstanceAction, FactScalar, RequestId, RuntimeConfigManifest, RuntimeFactScope,
    RuntimeLifecyclePhase, RuntimePayload, RuntimeSubscriptionRequest, SchedulingPauseReason,
    SchedulingPauseScope, StartupPackageDisposition, StartupPackageWithheld, SubscriptionCursor,
};
use actingcommand_device::codes::DeviceStage;
use actingcommand_device::{
    CaptureOpenCause, CaptureOpenFailure, DeviceProcessPresence, DeviceResourceQuiescence,
};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

const ALIAS: &str = "a50.instance";
/// A neutral application package, as an instance's binding names it.
const PACKAGE: &str = "neutral.application";
/// The emulator instance index a Nemu open connects to.
const INSTANCE_INDEX: i64 = 3;
const CAPTURE_OPEN_FAILED: &str = "capture_backend_open_failed";
const STOP: EmulatorInstanceAction = EmulatorInstanceAction::Stop;
const START: EmulatorInstanceAction = EmulatorInstanceAction::Start;
const RESTART: EmulatorInstanceAction = EmulatorInstanceAction::Restart;

/// A Runtime clock for the restart's wait: every reading moves it 1 ms on (so records keep
/// strictly increasing times) and every wait moves it on by the wait, returning at once.
struct ExitWaitClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
}

impl ExitWaitClock {
    fn new() -> Self {
        Self {
            unix_ms: AtomicU64::new(1_000),
            monotonic_ms: AtomicU64::new(0),
        }
    }
}

impl RuntimeClock for ExitWaitClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        Ok(RuntimeClockSample {
            unix_ms: self.unix_ms.fetch_add(1, Ordering::SeqCst) + 1,
            monotonic_ms: self.monotonic_ms.fetch_add(1, Ordering::SeqCst) + 1,
        })
    }

    fn pause(&self, duration: Duration) {
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.unix_ms.fetch_add(duration_ms, Ordering::SeqCst);
        self.monotonic_ms.fetch_add(duration_ms, Ordering::SeqCst);
    }
}

/// `operation` from the CLI on `connection`.
fn send(host: &RuntimeHost, operation: RuntimeOperation, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.process_request_for_test(
        &runtime_request(&ids, operation),
        ConnectionId::new(connection).expect("connection"),
    )
    .expect("receipt")
}

/// `actingctl emulator <action>` of the instance.
fn control(host: &RuntimeHost, action: EmulatorInstanceAction, connection: u64) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::ControlEmulatorInstance {
            instance_alias: ALIAS.to_owned(),
            action,
        },
        connection,
    )
}

/// The outcome of the `runtime.failed` record of `code` under the request `request_id`.
fn failure_record(host: &RuntimeHost, request_id: RequestId, code: &str) -> Option<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        request_id: Some(request_id),
        ..EventQuery::default()
    })
    .expect("events of the request")
    .iter()
    .find_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(record))
            if record.outcome().code().as_str() == code =>
        {
            Some(record.outcome().clone())
        }
        _ => None,
    })
}

fn value(outcome: &Outcome, key: &str) -> Option<Value> {
    outcome.value(key).cloned()
}

// ---- Item 1: a capture open that cannot open says why.

/// A capture open failure as a Nemu open reports it: `cause`, the package and the instance
/// index, its one acquired resource closed.
fn nemu_capture_open_failure(cause: CaptureOpenCause) -> DeviceError {
    DeviceError::fatal("fixture capture open failed")
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
        .with_capture_open_failure(CaptureOpenFailure {
            cause,
            application_id: Some(PACKAGE.to_owned()),
            instance_index: Some(INSTANCE_INDEX),
        })
}

/// `actingctl selfcheck` of an instance whose capture open fails with `error`: the outcome of
/// the self-check's `capture_backend_open_failed` record.
fn self_check_capture_failure(error: DeviceError) -> Outcome {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    *state.capture_open_error.lock().expect("capture open error") = Some(error);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: ALIAS.to_owned(),
        },
    );
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(501).expect("connection"))
        .expect("self-check receipt");
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = receipt.result() else {
        panic!("a self-check result: {receipt:?}");
    };
    assert_eq!(
        selfcheck.failure_code.as_deref(),
        Some(CAPTURE_OPEN_FAILED),
        "{selfcheck:?}"
    );
    let outcome = failure_record(&host, request.request_id(), CAPTURE_OPEN_FAILED)
        .expect("the self-check records the failed capture open");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
    outcome
}

// T1: Nemu found no display for the package (the game is not running): the failure names the
// cause `display_missing`, the package and the instance index.
#[test]
fn a50_t1_a_missing_package_display_is_the_capture_open_failures_cause_with_package_and_index() {
    let outcome =
        self_check_capture_failure(nemu_capture_open_failure(CaptureOpenCause::DisplayMissing));

    assert_eq!(
        value(&outcome, "capture_open_cause"),
        Some(json!("display_missing")),
        "{outcome}"
    );
    assert_eq!(
        value(&outcome, "application_id"),
        Some(json!(PACKAGE)),
        "{outcome}"
    );
    assert_eq!(
        value(&outcome, "instance_index"),
        Some(json!(INSTANCE_INDEX)),
        "{outcome}"
    );
}

// T2: the connection to the emulator failed (Nemu answered no connection handle): its own cause.
#[test]
fn a50_t2_a_failed_connection_to_the_emulator_is_its_own_capture_open_cause() {
    let outcome = self_check_capture_failure(nemu_capture_open_failure(
        CaptureOpenCause::ConnectionFailed,
    ));

    assert_eq!(
        value(&outcome, "capture_open_cause"),
        Some(json!("connection_failed")),
        "{outcome}"
    );
}

// ---- Item 2: an operator start or restart queues the startup package.

/// A started host whose one physical instance has the neutral startup package and is
/// discovery-bound to the scripted emulator, running on its default port, with opens that pass a
/// preparation's self-check.
struct StartupHost {
    _root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
}

fn startup_host() -> StartupHost {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = start_startup_host(&root, instance_id(), &state);
    StartupHost {
        _root: root,
        host,
        state,
    }
}

/// [`startup_host`]'s start on `root` with `instance` and its scripted emulator `state`. W2-B: a
/// start on a root an earlier start left finds the emulator bound as that start left it.
fn start_startup_host(root: &TempDir, instance: InstanceId, state: &Arc<FakeState>) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let package = root.path().join("startup-package.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    let host = RuntimeHost::start(
        config(root).with_startup_packages(BTreeMap::from([(ALIAS.to_owned(), startup)])),
        Arc::new(FakeProvider::one(ALIAS, instance, Arc::clone(state))),
    )
    .expect("runtime host with a startup package");
    if !state.emulator_control.load(Ordering::Acquire) {
        bind_fake_emulator(&host, ALIAS, instance, state);
    }
    host
}

/// The emulator control's answer about the startup package: what was queued and why not.
fn startup_answer(
    receipt: &RuntimeReceipt,
) -> (StartupPackageDisposition, Option<StartupPackageWithheld>) {
    match receipt.result() {
        Some(RuntimeResult::EmulatorInstanceControlled {
            startup_package,
            startup_package_withheld,
            ..
        }) => (*startup_package, *startup_package_withheld),
        _ => panic!("an emulator control result: {receipt:?}"),
    }
}

fn startup_scheduled(host: &RuntimeHost) -> usize {
    phases(&all_events(host))
        .iter()
        .filter(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. }))
        .count()
}

// T3: the game is not running, so the restart's preparation fails its capture open (no display
// for the package); the instance is not paused and has a startup package, so the restart queues
// it, as the Runtime's own start does: that run starts the game.
#[test]
fn a50_t3_an_operator_restart_of_an_unpaused_instance_queues_its_startup_package() {
    let run = startup_host();
    *run.state
        .capture_open_error
        .lock()
        .expect("capture open error") =
        Some(nemu_capture_open_failure(CaptureOpenCause::DisplayMissing));

    let receipt = control(&run.host, RESTART, 503);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        startup_answer(&receipt),
        (StartupPackageDisposition::Scheduled, None),
        "{receipt:?}"
    );
    assert_eq!(
        startup_scheduled(&run.host),
        1,
        "one startup package queued"
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

// T4 (#411 A71 replaced A50's rule that a pause queues nothing): the same restart on an instance
// its own scheduling pause holds queues the startup package too; its claim waits gated behind the
// pause, and the answer names the pause with a typed reason.
#[test]
fn a50_t4_an_operator_restart_of_a_paused_instance_queues_its_startup_package_behind_the_pause() {
    let run = startup_host();
    let paused = send(
        &run.host,
        RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            pause_reason: SchedulingPauseReason::new("a50_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        },
        504,
    );
    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");

    let receipt = control(&run.host, RESTART, 505);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        startup_answer(&receipt),
        (
            StartupPackageDisposition::Scheduled,
            Some(StartupPackageWithheld::InstancePaused)
        ),
        "{receipt:?}"
    );
    assert_eq!(
        startup_scheduled(&run.host),
        1,
        "one startup package queued"
    );
    assert_eq!(
        run.host.instance_claims_for_test(ALIAS).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits gated behind the pause"
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

// ---- #411 A71: a start during a pause runs its startup package after the resume.

/// The records of `event_type`.
fn recorded(host: &RuntimeHost, event_type: EventType) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        ..EventQuery::default()
    })
    .expect("events of the type")
    .len()
}

/// Returns once a record of `event_type` after `after` is recorded (a ledger subscription that
/// answers as soon as it is; `GATE_WAIT` only bounds a broken run).
fn await_recorded(host: &RuntimeHost, event_type: EventType, after: u64) {
    let query = EventQuery {
        event_type: Some(event_type),
        from_sequence: Some(after + 1),
        ..EventQuery::default()
    };
    let subscription = RuntimeSubscriptionRequest::new(
        query,
        ProjectionProfile::Forensic,
        SubscriptionCursor::default(),
        u64::try_from(GATE_WAIT.as_millis()).expect("gate wait"),
        1,
    )
    .expect("subscription request");
    let receipt = send(
        host,
        RuntimeOperation::SubscribeEvents {
            request: subscription,
        },
        513,
    );
    let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
        panic!("an event batch: {receipt:?}");
    };
    assert!(!batch.timed_out(), "{event_type:?} after {after}");
}

// T8: the instance is paused, then the operator starts its emulator. The start queues the startup
// package and the answer names the pause; under the pause its claim only waits (also after a
// sweep tick) and nothing runs. The resume's reconnect hands the key on, the startup package runs
// once, and no second startup claim waits.
#[test]
fn a71_t8_a_start_during_a_pause_runs_its_startup_package_once_after_the_resume() {
    let run = startup_host();
    let paused = send(
        &run.host,
        RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            pause_reason: SchedulingPauseReason::new("a71_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        },
        510,
    );
    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");

    let started = control(&run.host, START, 511);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    assert_eq!(
        startup_answer(&started),
        (
            StartupPackageDisposition::Scheduled,
            Some(StartupPackageWithheld::InstancePaused)
        ),
        "{started:?}"
    );
    run.host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        run.host.instance_claims_for_test(ALIAS).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits behind the pause"
    );
    assert_eq!(
        recorded(&run.host, EventType::TaskRequested),
        0,
        "no startup run under the pause"
    );

    let resumed_at = all_events(&run.host)
        .last()
        .map_or(0, PersistedEvent::sequence);
    let resumed = send(
        &run.host,
        RuntimeOperation::ResumeScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            expected: None,
        },
        512,
    );
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    await_recorded(&run.host, EventType::TaskCompleted, resumed_at);
    run.host.expire_due_leases_for_test().expect("sweep tick");

    assert_eq!(
        recorded(&run.host, EventType::TaskRequested),
        1,
        "the startup package ran once"
    );
    assert_eq!(recorded(&run.host, EventType::TaskCompleted), 1);
    assert_eq!(
        startup_scheduled(&run.host),
        1,
        "one startup package queued"
    );
    assert!(
        !run.host
            .instance_claims_for_test(ALIAS)
            .expect("claims")
            .1
            .contains(&ClaimKind::StartupPackage),
        "no second startup claim waits"
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

// ---- W2-B: the startup package queued during a pause survives a restart.

/// The sequence of the host's latest record.
fn last_sequence(host: &RuntimeHost) -> u64 {
    all_events(host).last().map_or(0, PersistedEvent::sequence)
}

/// How many times the startup package was scheduled after `after` (`startup_package_scheduled`).
fn startup_scheduled_after(host: &RuntimeHost, after: u64) -> usize {
    phases(&all_events(host))
        .iter()
        .filter(|(event, phase)| {
            event.sequence() > after
                && matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. })
        })
        .count()
}

/// The aliases `host.idle_state` keeps as having a startup package queued behind a pause
/// (`startup_package_queued`).
fn kept_startup_queued(host: &RuntimeHost) -> Vec<String> {
    let record = host
        .runtime_fact_snapshot()
        .expect("the program facts")
        .records
        .into_iter()
        .find(|record| {
            record.scope == RuntimeFactScope::Runtime && record.key == "host.idle_state"
        });
    let Some(record) = record else {
        return Vec::new();
    };
    match record.value {
        actingcommand_contract::FactValue::RecordList(rows) => rows
            .iter()
            .filter(|row| row.get("startup_package_queued") == Some(&FactScalar::Boolean(true)))
            .map(|row| match row.get("instance_alias") {
                Some(FactScalar::String(alias)) => alias.clone(),
                other => panic!("a kept startup package names its instance: {other:?}"),
            })
            .collect(),
        other => panic!("host.idle_state is a record list: {other:?}"),
    }
}

// T9: T8's start during the instance's pause, then a restart before the resume (a deploy restarts
// the Runtime; the stop drops the queued claim). The idle record keeps the queued startup package,
// so the restart queues it again behind the restored pause; nothing runs under the pause. After the
// resume it runs once and is kept no more, and a further restart queues it no more.
#[test]
fn w2b_t9_a_start_during_a_pause_runs_its_startup_package_once_after_a_restart_and_the_resume() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let host = start_startup_host(&root, instance, &state);
    let paused = send(
        &host,
        RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            pause_reason: SchedulingPauseReason::new("w2b_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        },
        520,
    );
    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
    let started = control(&host, START, 521);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    assert_eq!(
        startup_answer(&started),
        (
            StartupPackageDisposition::Scheduled,
            Some(StartupPackageWithheld::InstancePaused)
        ),
        "{started:?}"
    );
    assert_eq!(
        kept_startup_queued(&host),
        [ALIAS],
        "the idle record keeps the startup package queued behind the pause"
    );

    // The restart before the resume.
    let stopped_at = last_sequence(&host);
    host.close().expect("close the paused host");
    let host = start_startup_host(&root, instance, &state);
    let (_, pauses) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(
        pauses.contains_key(ALIAS),
        "the instance's pause is restored"
    );
    host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        host.instance_claims_for_test(ALIAS).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the restart queues the startup package again behind the restored pause"
    );
    assert_eq!(
        startup_scheduled_after(&host, stopped_at),
        1,
        "queued again once"
    );
    assert_eq!(
        recorded(&host, EventType::TaskRequested),
        0,
        "no startup run under the pause"
    );
    assert_eq!(kept_startup_queued(&host), [ALIAS], "still kept");

    let resumed_at = last_sequence(&host);
    let resumed = send(
        &host,
        RuntimeOperation::ResumeScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            expected: None,
        },
        522,
    );
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    await_recorded(&host, EventType::TaskCompleted, resumed_at);
    host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        recorded(&host, EventType::TaskRequested),
        1,
        "the startup package ran once"
    );
    assert_eq!(recorded(&host, EventType::TaskCompleted), 1);
    assert_eq!(
        startup_scheduled_after(&host, stopped_at),
        1,
        "no other startup package queued"
    );
    assert!(
        !host
            .instance_claims_for_test(ALIAS)
            .expect("claims")
            .1
            .contains(&ClaimKind::StartupPackage),
        "no second startup claim waits"
    );
    assert!(
        kept_startup_queued(&host).is_empty(),
        "granted, the startup package is kept no more"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());

    // Never twice: a further restart queues nothing.
    let stopped_at = last_sequence(&host);
    host.close().expect("close the resumed host");
    let host = start_startup_host(&root, instance, &state);
    host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        startup_scheduled_after(&host, stopped_at),
        0,
        "a further restart queues no startup package"
    );
    assert!(
        !host
            .instance_claims_for_test(ALIAS)
            .expect("claims")
            .1
            .contains(&ClaimKind::StartupPackage),
        "no startup claim waits"
    );
    assert_eq!(
        recorded(&host, EventType::TaskRequested),
        1,
        "the startup package ran once in all"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// ---- Item 3: a restart waits for the old VM.

/// A started host whose one physical instance is discovery-bound to the scripted emulator,
/// running on its default port, on the restart wait's clock; `manifest` is its configuration.
fn restart_host(
    root: &TempDir,
    manifest: Option<RuntimeConfigManifest>,
) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let mut host_config = config(root).with_runtime_clock(Arc::new(ExitWaitClock::new()));
    if let Some(manifest) = manifest {
        host_config = host_config.with_config_manifest(manifest);
    }
    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(ALIAS, instance, Arc::clone(&state))),
    )
    .expect("runtime host");
    bind_fake_emulator(&host, ALIAS, instance, &state);
    (host, state)
}

/// A configuration whose `emulator_control` section is the defaults file's with the members
/// `section` names, as actingd assembles it.
fn emulator_control_manifest(section: Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "emulator_control": section }),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// The restart's Stop: its readiness wait expired while the old VM still ran.
fn stop_wait_expired_running() -> ScriptedControl {
    ScriptedControl::Failed {
        stage: DeviceStage::MumuManagerWaitTimeout.location(),
        exit_code: Some(0),
        last_state: Some(running_state(FAKE_EMULATOR_PORT)),
        device_process_present: Some(DeviceProcessPresence::Present),
    }
}

// T5: the status reads running, running, stopped (the first is the bounded read after the
// Stop's failure): the restart issues its launch only after the read that shows it stopped.
#[test]
fn a50_t5_a_restart_launches_only_after_the_status_reads_the_old_vm_stopped() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = restart_host(&root, None);
    let emulator = &state.emulator;
    emulator.script_control(STOP, stop_wait_expired_running());
    emulator.script_read(running_state(FAKE_EMULATOR_PORT));
    emulator.script_read(running_state(FAKE_EMULATOR_PORT));
    emulator.script_read(stopped_state());
    let mark = emulator.calls().len();

    let _ = control(&host, RESTART, 506);

    let calls = emulator.calls()[mark..].to_vec();
    let launch = calls
        .iter()
        .position(|call| *call == EmulatorCall::Control(START))
        .unwrap_or_else(|| panic!("the restart launched: {calls:?}"));
    let reads_before_launch = calls[..launch]
        .iter()
        .filter(|call| **call == EmulatorCall::ReadState)
        .count();
    assert_eq!(
        reads_before_launch, 3,
        "the launch follows the read that shows the old VM stopped: {calls:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// T6: with `emulator_control.restart_exit_wait_ms` 5000 (not the default 60 000) the old VM
// never stops: the restart fails with `emulator_restart_previous_instance_running` after the
// configured bound and launches nothing.
#[test]
fn a50_t6_an_old_vm_still_running_at_the_configured_bound_fails_the_restart_without_a_launch() {
    const BOUND_MS: u64 = 5_000;
    let root = TempDir::new().expect("tempdir");
    let (host, state) = restart_host(
        &root,
        Some(emulator_control_manifest(
            json!({"restart_exit_wait_ms": BOUND_MS}),
        )),
    );
    let emulator = &state.emulator;
    emulator.script_control(STOP, stop_wait_expired_running());
    for _ in 0..64 {
        emulator.script_read(running_state(FAKE_EMULATOR_PORT));
    }
    let mark = emulator.calls().len();

    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::ControlEmulatorInstance {
            instance_alias: ALIAS.to_owned(),
            action: RESTART,
        },
    );
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(507).expect("connection"))
        .expect("restart receipt");

    let code = HostCode::EmulatorRestartPreviousInstanceRunning.as_str();
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
    let launches = emulator.calls()[mark..]
        .iter()
        .filter(|call| **call == EmulatorCall::Control(START))
        .count();
    assert_eq!(launches, 0, "nothing is launched");
    let failure =
        failure_record(&host, request.request_id(), code).expect("the restart's failure record");
    assert_eq!(
        value(&failure, "timeout_ms"),
        Some(json!(BOUND_MS)),
        "{failure}"
    );
    let waited = value(&failure, "elapsed_ms")
        .and_then(|waited| waited.as_u64())
        .expect("the time waited");
    assert!(
        (BOUND_MS..60_000).contains(&waited),
        "the restart waited the configured bound: {waited} ms"
    );
    assert_eq!(
        value(&failure, "process_started"),
        Some(json!(true)),
        "{failure}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// ---- Item 4: the recovery preparation reads the emulator status first (A38 follow-up).

// T7: a fence waits for a person; meanwhile the emulator was restarted outside the Runtime, which
// holds the instance stopped (its binding pending, its opens refused). The fence's own recheck
// runs `prepare_recovery_connection`: it reads the emulator status first, finds it running on its
// port, recognizes the device again and binds it, and the recheck passes and lifts the fence.
#[test]
fn a50_t7_the_recovery_preparation_reads_the_status_and_recognizes_a_running_emulator() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.emulator_control.store(true, Ordering::Release);
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_config_manifest(ladder_manifest(json!({
                "fence_recheck": {"interval_ms": 60_000, "max_attempts": 3}
            }))),
        Arc::new(FakeProvider::one(ALIAS, instance, Arc::clone(&state))),
    )
    .expect("runtime host");
    // A person's direct run whose input ends with an unconfirmed close fences the instance,
    // which then waits for a person.
    *state.input_error_once.lock().expect("one-shot input error") = Some(
        DeviceError::fatal("synthetic input command left its child unconfirmed")
            .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1),
    );
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    );
    let failed = host
        .process_request_for_test(&run, ConnectionId::new(508).expect("connection"))
        .expect("run receipt");
    assert_eq!(
        failed.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str(),
        "{failed:?}"
    );
    // The Runtime holds the instance stopped while the emulator runs again on its port.
    host.replace_instance_endpoint_for_test(
        ALIAS,
        fake_discovered_instance(instance, None)
            .adb_endpoint()
            .cloned(),
    )
    .expect("hold the instance stopped");
    state.emulator.rebind(None);
    state.refuse_pending_opens.store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    let mark = state.emulator.calls().len();

    let next = status_fence(&host, ALIAS)
        .and_then(|fence| fence.person_wait)
        .and_then(|wait| wait.next_recheck_unix_ms)
        .expect("the fence's next recheck");
    clock.advance(next.saturating_sub(clock.sample().expect("clock").unix_ms));
    host.fence_recheck_tick_for_test().expect("sweep tick");
    let recheck_failed = HostCode::InstanceResourcesFenceRecheckFailed.as_str();
    wait_until("the recheck's end", || {
        let events = all_events(&host);
        phases(&events)
            .iter()
            .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::ResourceFenceLifted { .. }))
            || events.iter().any(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Runtime(RuntimePayload::Failed(record))
                        if record.outcome().code().as_str() == recheck_failed
                )
            })
    });

    let calls = state.emulator.calls()[mark..].to_vec();
    let read = calls
        .iter()
        .position(|call| *call == EmulatorCall::ReadState)
        .unwrap_or_else(|| panic!("the recheck reads the emulator status: {calls:?}"));
    let bound = calls
        .iter()
        .position(|call| *call == EmulatorCall::Rebind(Some(FAKE_EMULATOR_PORT)))
        .unwrap_or_else(|| panic!("the device is bound again: {calls:?}"));
    assert!(read < bound, "the status read comes first: {calls:?}");
    assert_eq!(state.emulator.bound_port(), Some(FAKE_EMULATOR_PORT));
    assert_eq!(
        status_fence(&host, ALIAS),
        None,
        "the passing recheck lifts the fence"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
