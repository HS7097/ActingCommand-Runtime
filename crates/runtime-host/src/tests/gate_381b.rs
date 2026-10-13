// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 B, test plan H-2 (Host side, the restart composed in the Host): the operator
//! restart is a Stop phase and then a Start phase through the existing drive path (F1′), a Stop
//! whose readiness wait expired still starts (F3), every failure applies the instance's current
//! state and records it (F4, review M3), and the ladder's emulator-restart rung follows the same
//! Stop rule (RB-H3). The emulator is the scripted controller (HOST-I1); every wait returns as
//! soon as the awaited call arrives, `GATE_WAIT` only bounds a broken run.

use super::emulator_controller::{EmulatorCall, ScriptedControl, running_state, stopped_state};
use super::instance_workers::{
    LadderRun, closed_ledger_events, emulator_ladder_fixture, phases, rung_outcomes,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{
    DiagnosticCode, EmulatorInstanceAction, EventAction, RecoveryRung, RecoveryRungOutcome,
    RuntimeErrorProjection, RuntimeFactInvalidationReason, RuntimeFactRecord, RuntimeFactScope,
    RuntimeLifecyclePhase, RuntimePayload, SchedulerPayload,
};
use actingcommand_device::DeviceProcessPresence;

const GATE_WAIT: Duration = Duration::from_secs(120);
const ALIAS: &str = "emulator.a";
const DEVICE_CONNECTED: &str = "device.connected";
const SELFCHECK_PREFIX: &str = "backend.selfcheck.";
const STOP: EmulatorInstanceAction = EmulatorInstanceAction::Stop;
const START: EmulatorInstanceAction = EmulatorInstanceAction::Start;
const RESTART: EmulatorInstanceAction = EmulatorInstanceAction::Restart;

/// A started host whose one physical instance is discovery-bound to the scripted emulator,
/// running on its default port.
struct EmulatorHost {
    _root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    instance_id: InstanceId,
}

fn emulator_host() -> EmulatorHost {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id, Arc::clone(&state))),
    )
    .expect("runtime host");
    bind_fake_emulator(&host, ALIAS, instance_id, &state);
    EmulatorHost {
        _root: root,
        host,
        state,
        instance_id,
    }
}

fn cli_request(operation: RuntimeOperation) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        operation,
    )
    .expect("cli request")
}

fn send(host: &RuntimeHost, operation: RuntimeOperation) -> RuntimeReceipt {
    host.process_request_for_test(
        &cli_request(operation),
        ConnectionId::new(91).expect("connection"),
    )
    .expect("receipt")
}

/// `actingctl emulator <action>`.
fn control(host: &RuntimeHost, action: EmulatorInstanceAction) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::ControlEmulatorInstance {
            instance_alias: ALIAS.to_owned(),
            action,
        },
    )
}

fn host_code(receipt: &RuntimeReceipt) -> Option<&str> {
    receipt
        .error_projection()
        .and_then(RuntimeErrorProjection::host_code)
}

/// The port the Runtime reports the instance bound to (`actingctl emulator status`); `None`
/// while the binding is pending.
fn status_port(host: &RuntimeHost) -> Option<u16> {
    let receipt = send(host, RuntimeOperation::Status);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == ALIAS)
        .expect("the instance's status")
        .adb_port()
}

fn instance_facts(host: &RuntimeHost, instance_id: InstanceId) -> Vec<RuntimeFactRecord> {
    let receipt = send(host, RuntimeOperation::RuntimeFactSnapshot);
    let Some(RuntimeResult::RuntimeFactSnapshot { snapshot }) = receipt.result() else {
        panic!("a fact snapshot: {receipt:?}");
    };
    snapshot
        .records
        .iter()
        .filter(|record| record.scope == RuntimeFactScope::Instance { instance_id })
        .cloned()
        .collect()
}

fn connected(facts: &[RuntimeFactRecord]) -> bool {
    facts.iter().any(|record| {
        record.key == DEVICE_CONNECTED && record.value == ContractFactValue::Boolean(true)
    })
}

fn self_checked(facts: &[RuntimeFactRecord]) -> bool {
    facts
        .iter()
        .any(|record| record.key.starts_with(SELFCHECK_PREFIX))
}

fn all_events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

/// The events recorded under the receipt's request.
fn request_events(host: &RuntimeHost, receipt: &RuntimeReceipt) -> Vec<PersistedEvent> {
    let request_id = receipt.request_id();
    all_events(host)
        .into_iter()
        .filter(|event| event.links().request_id() == Some(&request_id))
        .collect()
}

fn of_type(events: &[PersistedEvent], event_type: EventType) -> Vec<&PersistedEvent> {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .collect()
}

fn actions(events: &[PersistedEvent], event_type: EventType) -> Vec<EventAction> {
    of_type(events, event_type)
        .into_iter()
        .map(|event| event.payload().action())
        .collect()
}

/// The `runtime.failed` records among `events`: each one's code and its outcome as the
/// `key=value` tokens its primary detail message carried before ledger revision 3 (#378 A3):
/// every scalar value of the outcome, and `cause=<code>` for each `caused_by` link.
fn failures(events: &[PersistedEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.lifecycle_failure().is_some() =>
            {
                let outcome = record.outcome();
                let mut tokens = outcome
                    .values()
                    .iter()
                    .filter_map(|(key, value)| match value {
                        serde_json::Value::String(text) => Some(format!("{key}={text}")),
                        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
                            Some(format!("{key}={value}"))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                tokens.extend(
                    outcome_links(
                        outcome,
                        actingcommand_contract::outcome::CauseRelation::CausedBy,
                    )
                    .into_iter()
                    .map(|cause| format!("cause={}", cause.code())),
                );
                Some((outcome.code().as_str().to_owned(), tokens.join(" ")))
            }
            _ => None,
        })
        .collect()
}

fn assert_tokens(message: &str, tokens: &[&str]) {
    for token in tokens {
        assert!(
            message.split_whitespace().any(|part| part == *token),
            "{token} in {message}"
        );
    }
}

fn control_calls(calls: &[EmulatorCall]) -> Vec<EmulatorInstanceAction> {
    calls
        .iter()
        .filter_map(|call| match call {
            EmulatorCall::Control(action) => Some(*action),
            _ => None,
        })
        .collect()
}

fn read_calls(calls: &[EmulatorCall]) -> usize {
    calls
        .iter()
        .filter(|call| **call == EmulatorCall::ReadState)
        .count()
}

/// The fact events of the instance's `key` after `after`.
fn fact_events_after<'a>(
    events: &'a [PersistedEvent],
    key: &str,
    after: u64,
) -> Vec<&'a PersistedEvent> {
    events
        .iter()
        .filter(|event| event.sequence() > after)
        .filter(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded)) => {
                recorded.record().key == key
            }
            EventPayload::Runtime(RuntimePayload::FactInvalidated(invalidated)) => {
                invalidated.invalidation().key.starts_with(key)
            }
            _ => false,
        })
        .collect()
}

fn recorded_false(event: &PersistedEvent) -> bool {
    matches!(
        event.payload(),
        EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
            if recorded.record().value == ContractFactValue::Boolean(false)
    )
}

fn invalidated(event: &PersistedEvent) -> bool {
    matches!(
        event.payload(),
        EventPayload::Runtime(RuntimePayload::FactInvalidated(_))
    )
}

/// The failed Stop of the 10-09 replay: its readiness wait expired with the instance stopped
/// while its device process stayed.
fn stop_wait_expired() -> ScriptedControl {
    ScriptedControl::Failed {
        stage: "mumu_manager.wait_timeout",
        exit_code: Some(0),
        last_state: Some(stopped_state()),
        device_process_present: Some(DeviceProcessPresence::Present),
    }
}

fn start_wait_expired() -> ScriptedControl {
    ScriptedControl::Failed {
        stage: "mumu_manager.wait_timeout",
        exit_code: Some(0),
        last_state: Some(stopped_state()),
        device_process_present: None,
    }
}

/// RB-D1 (F1′): an operator restart is a Stop phase and then a Start phase, each through the
/// existing drive path; the vendor's `restart` is never dispatched; each phase records its
/// `command.received` and `command.validated` under the restart's links, and the receipt ends
/// on the Start's `command.validated`.
#[test]
fn gate_an_operator_restart_is_a_stop_phase_then_a_start_phase_never_the_vendor_restart() {
    let run = emulator_host();
    let receipt = control(&run.host, RESTART);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::EmulatorInstanceControlled {
        action,
        running,
        adb_port,
        ..
    }) = receipt.result()
    else {
        panic!("an emulator control result: {receipt:?}");
    };
    assert_eq!(*action, RESTART);
    assert!(*running);
    assert_eq!(*adb_port, Some(16384));
    assert_eq!(control_calls(&run.state.emulator.calls()), [STOP, START]);
    let events = request_events(&run.host, &receipt);
    assert_eq!(
        actions(&events, EventType::CommandReceived),
        [
            EventAction::EmulatorInstanceRestart,
            EventAction::EmulatorInstanceStop,
            EventAction::EmulatorInstanceStart,
        ]
    );
    assert_eq!(
        actions(&events, EventType::CommandValidated),
        [
            EventAction::EmulatorInstanceStop,
            EventAction::EmulatorInstanceStart,
        ]
    );
    let start_validated = of_type(&events, EventType::CommandValidated)[1];
    assert_eq!(
        receipt.terminal().map(|terminal| terminal.sequence),
        Some(start_validated.sequence())
    );
    assert!(of_type(&events, EventType::CommandRejected).is_empty());
    run.host.close().expect("close host");
}

/// RB-D3 (F3, D2): a Stop whose readiness wait expired still dispatches the Start; the restart
/// fails `phase=stop` whatever its Start did, with the last state, in one failure record that is
/// the receipt's terminal and comes after the Start phase.
#[test]
fn gate_a_restart_whose_stop_wait_expired_still_starts_and_fails_in_the_stop_phase() {
    let run = emulator_host();
    let emulator = &run.state.emulator;
    emulator.script_control(STOP, stop_wait_expired());
    emulator.script_read(stopped_state());
    emulator.script_control(START, start_wait_expired());
    emulator.script_read(stopped_state());
    let receipt = control(&run.host, RESTART);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        host_code(&receipt),
        Some(HostCode::EmulatorRestartStopFailed.as_str())
    );
    assert_eq!(control_calls(&emulator.calls()), [STOP, START]);
    let events = request_events(&run.host, &receipt);
    let rejected = of_type(&events, EventType::CommandRejected);
    assert_eq!(rejected.len(), 1, "one failure record: {events:?}");
    assert_eq!(
        rejected[0].payload().action(),
        EventAction::EmulatorInstanceRestart
    );
    assert_eq!(
        receipt.terminal().map(|terminal| terminal.sequence),
        Some(rejected[0].sequence())
    );
    let start_received = of_type(&events, EventType::CommandReceived)
        .into_iter()
        .find(|event| event.payload().action() == EventAction::EmulatorInstanceStart)
        .expect("the Start phase ran");
    assert!(rejected[0].sequence() > start_received.sequence());
    let recorded = failures(&events);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].0, HostCode::EmulatorRestartStopFailed.as_str());
    assert_tokens(
        &recorded[0].1,
        &[
            "restart_phase=stop",
            "stop_result=wait_timeout",
            "start_dispatched=true",
            "process_started=false",
            "android_started=false",
            "adb_port_present=false",
            "device_process_present=true",
            "cause=emulator_control_wait_timeout",
            "cause=foreign_mumu_manager_error",
        ],
    );
    for key in ["stop_elapsed_ms=", "start_elapsed_ms="] {
        assert!(recorded[0].1.contains(key), "{key} in {}", recorded[0].1);
    }
    // #378 A3 acceptance H-M1: the failing phase's vendor output is MuMuManager's, with the
    // control command's exit status (0 when the wait expired) as `exit_code`; an exit status
    // is never an OS error, so no link of the record carries `foreign_os_error` or `os_error`.
    let outcome = events
        .iter()
        .find_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.lifecycle_failure().is_some() =>
            {
                Some(record.outcome())
            }
            _ => None,
        })
        .expect("the restart's runtime.failed record");
    let vendor = outcome_links(
        outcome,
        actingcommand_contract::outcome::CauseRelation::CausedBy,
    )
    .into_iter()
    .find(|link| link.code().as_str() == "foreign_mumu_manager_error")
    .expect("the vendor output link");
    assert_eq!(vendor.value("exit_code"), Some(&serde_json::json!(0)));
    assert_eq!(
        vendor.value("raw_source"),
        Some(&serde_json::json!("mumu_manager"))
    );
    assert_eq!(
        vendor.value("raw_text"),
        Some(&serde_json::json!("scripted vendor output"))
    );
    assert!(vendor.value("os_error").is_none() && vendor.value("io_kind").is_none());
    assert!(
        outcome
            .codes()
            .iter()
            .all(|code| code.as_str() != "foreign_os_error"),
        "{outcome}"
    );
    run.host.close().expect("close host");
}

/// RB-D4 (F3's branch table): after a Stop that ended in `mumu_manager.wait_timeout` the Start
/// is dispatched; after any other Stop failure it is not, and the restart fails `phase=stop`.
#[test]
fn gate_only_a_stop_whose_wait_expired_is_followed_by_a_start() {
    let launch_failed = InstanceState {
        launch_err_code: 3,
        ..stopped_state()
    };
    let rows: [(&'static str, Option<i32>, Option<InstanceState>, bool); 5] = [
        (
            "mumu_manager.wait_timeout",
            Some(0),
            Some(stopped_state()),
            true,
        ),
        ("mumu_manager.control_exit", Some(1), None, false),
        ("mumu_manager.control_errcode", Some(0), None, false),
        ("mumu_manager.run", None, None, false),
        (
            "mumu_manager.launch_error",
            Some(0),
            Some(launch_failed),
            false,
        ),
    ];
    for (stage, exit_code, last_state, starts) in rows {
        let run = emulator_host();
        let emulator = &run.state.emulator;
        emulator.script_control(
            STOP,
            ScriptedControl::Failed {
                stage,
                exit_code,
                last_state,
                device_process_present: Some(DeviceProcessPresence::Unknown),
            },
        );
        let receipt = control(&run.host, RESTART);
        assert_eq!(
            host_code(&receipt),
            Some(HostCode::EmulatorRestartStopFailed.as_str()),
            "{stage}: {receipt:?}"
        );
        let expected: &[EmulatorInstanceAction] = if starts { &[STOP, START] } else { &[STOP] };
        assert_eq!(control_calls(&emulator.calls()), expected, "{stage}");
        let recorded = failures(&request_events(&run.host, &receipt));
        assert_eq!(recorded.len(), 1, "{stage}: {recorded:?}");
        let (_, token) = stage.rsplit_once('.').expect("a device stage");
        let stop_result = format!("stop_result={token}");
        let start_dispatched = format!("start_dispatched={starts}");
        assert_tokens(
            &recorded[0].1,
            &[
                "restart_phase=stop",
                stop_result.as_str(),
                start_dispatched.as_str(),
            ],
        );
        run.host.close().expect("close host");
    }
}

/// RB-D5 (F1′): the Stop and the Start are two independent provider calls; a confirmed Stop
/// followed by a Start whose readiness wait expired fails `phase=start` with the last state
/// stopped.
#[test]
fn gate_a_confirmed_stop_then_a_start_whose_wait_expired_fails_in_the_start_phase() {
    let run = emulator_host();
    let emulator = &run.state.emulator;
    emulator.script_control(START, start_wait_expired());
    emulator.script_read(stopped_state());
    let receipt = control(&run.host, RESTART);
    assert_eq!(
        host_code(&receipt),
        Some(HostCode::EmulatorRestartStartFailed.as_str()),
        "{receipt:?}"
    );
    assert_eq!(control_calls(&emulator.calls()), [STOP, START]);
    let recorded = failures(&request_events(&run.host, &receipt));
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_tokens(
        &recorded[0].1,
        &[
            "restart_phase=start",
            "stop_result=confirmed",
            "start_dispatched=true",
            "process_started=false",
            "android_started=false",
            "adb_port_present=false",
            "device_process_present=false",
        ],
    );
    run.host.close().expect("close host");
}

/// RB-H1 (F4): a restart whose Start fails with the instance stopped writes exactly one
/// `command.rejected` and one `runtime.failed` `emulator_restart_start_failed` with its cause;
/// the binding is pending, `device.connected` is false and the self-check is invalidated.
#[test]
fn gate_a_restart_whose_start_fails_stopped_leaves_the_instance_pending_and_disconnected() {
    let run = emulator_host();
    let started = control(&run.host, START);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    let before = instance_facts(&run.host, run.instance_id);
    assert!(connected(&before) && self_checked(&before), "{before:?}");
    let emulator = &run.state.emulator;
    emulator.script_control(START, start_wait_expired());
    emulator.script_read(stopped_state());
    let receipt = control(&run.host, RESTART);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let events = request_events(&run.host, &receipt);
    assert_eq!(of_type(&events, EventType::CommandRejected).len(), 1);
    let recorded = failures(&events);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].0, HostCode::EmulatorRestartStartFailed.as_str());
    assert_tokens(&recorded[0].1, &["cause=emulator_control_wait_timeout"]);
    assert_eq!(status_port(&run.host), None, "the binding is pending");
    let start_received = of_type(&events, EventType::CommandReceived)
        .into_iter()
        .find(|event| event.payload().action() == EventAction::EmulatorInstanceStart)
        .expect("the Start phase ran")
        .sequence();
    let all = all_events(&run.host);
    assert!(
        fact_events_after(&all, DEVICE_CONNECTED, start_received)
            .into_iter()
            .any(recorded_false),
        "device.connected is recorded false after the failed Start"
    );
    let after = instance_facts(&run.host, run.instance_id);
    assert!(!connected(&after), "{after:?}");
    assert!(!self_checked(&after), "{after:?}");
    run.host.close().expect("close host");
}

/// RB-H2 (F4, D2): a Stop whose wait expired, followed by a successful Start, fails
/// `emulator_restart_stop_failed`; the Start ran its whole success path, the binding is on the
/// port it reported and the self-check facts of the old session were invalidated.
#[test]
fn gate_a_restart_whose_stop_wait_expired_and_whose_start_succeeds_is_bound_to_the_new_port() {
    let run = emulator_host();
    let started = control(&run.host, START);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    assert!(self_checked(&instance_facts(&run.host, run.instance_id)));
    let emulator = &run.state.emulator;
    emulator.script_control(STOP, stop_wait_expired());
    emulator.script_read(stopped_state());
    emulator.script_control(
        START,
        ScriptedControl::Running {
            adb_port: Some(16448),
        },
    );
    let receipt = control(&run.host, RESTART);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        host_code(&receipt),
        Some(HostCode::EmulatorRestartStopFailed.as_str())
    );
    assert_eq!(status_port(&run.host), Some(16448));
    let events = request_events(&run.host, &receipt);
    let rejected = of_type(&events, EventType::CommandRejected);
    assert_eq!(rejected.len(), 1);
    let start_validated = of_type(&events, EventType::CommandValidated)
        .into_iter()
        .find(|event| event.payload().action() == EventAction::EmulatorInstanceStart)
        .expect("the Start phase completed");
    assert!(rejected[0].sequence() > start_validated.sequence());
    assert_eq!(
        receipt.terminal().map(|terminal| terminal.sequence),
        Some(rejected[0].sequence())
    );
    let restart_received = of_type(&events, EventType::CommandReceived)[0].sequence();
    let all = all_events(&run.host);
    assert!(
        fact_events_after(&all, SELFCHECK_PREFIX, restart_received)
            .into_iter()
            .any(invalidated),
        "the self-check facts were invalidated"
    );
    run.host.close().expect("close host");
}

/// RB-H4 (F4, review M3): a plain `emulator start` that fails with the instance stopped gets
/// the same state application as a restart: pending, not connected, self-check invalidated.
#[test]
fn gate_a_start_failing_with_the_instance_stopped_applies_the_stopped_state() {
    let run = emulator_host();
    let started = control(&run.host, START);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    let emulator = &run.state.emulator;
    emulator.script_control(START, start_wait_expired());
    emulator.script_read(stopped_state());
    let receipt = control(&run.host, START);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(status_port(&run.host), None, "the binding is pending");
    let after = instance_facts(&run.host, run.instance_id);
    assert!(!connected(&after), "{after:?}");
    assert!(!self_checked(&after), "{after:?}");
    run.host.close().expect("close host");
}

/// RB-H5 (review M3): an errcode refusal on a running instance takes exactly one bounded state
/// read, and the binding and `device.connected` stay as they were.
#[test]
fn gate_an_errcode_refusal_on_a_running_instance_reads_its_state_once_and_keeps_it_connected() {
    let run = emulator_host();
    let started = control(&run.host, START);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    let emulator = &run.state.emulator;
    let mark = emulator.calls().len();
    emulator.script_control(
        STOP,
        ScriptedControl::Failed {
            stage: "mumu_manager.control_errcode",
            exit_code: Some(0),
            last_state: None,
            device_process_present: Some(DeviceProcessPresence::Unknown),
        },
    );
    emulator.script_read(running_state(16384));
    let receipt = control(&run.host, STOP);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let calls = emulator.calls()[mark..].to_vec();
    assert_eq!(control_calls(&calls), [STOP]);
    assert_eq!(read_calls(&calls), 1, "{calls:?}");
    assert_eq!(status_port(&run.host), Some(16384));
    assert!(connected(&instance_facts(&run.host, run.instance_id)));
    run.host.close().expect("close host");
}

/// G19 row 1, the busy row (Workflow #369 H-4, W-3, S6b): an emulator control behind a holder
/// waits at most 5 s for its turn in the instance's queue; at the cap it is refused Denied
/// `lease_queue_expired`, naming the holder and its queue position, with one `scheduler.denied`
/// `lease.queue_expired`, no `emulator_control_busy` pair and no provider call. Ledger revision
/// 3 (#378 A3): the record's severity is its outcome's category, `lease_queue_expired`'s
/// `error` (was Warning).
#[test]
fn gate_an_emulator_control_behind_a_holder_expires_after_its_wait_without_a_provider_call() {
    // A frozen clock, so the holder's lease does not lapse while the control waits.
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0))),
        Arc::new(FakeProvider::one(ALIAS, instance_id, Arc::clone(&state))),
    )
    .expect("runtime host");
    bind_fake_emulator(&host, ALIAS, instance_id, &state);
    let run = EmulatorHost {
        _root: root,
        host,
        state,
        instance_id,
    };
    let mut holder = TestClient::connect(&run.host);
    let (_, token) = holder.acquire(ALIAS);
    let receipt = control(&run.host, START);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let error = receipt.error_projection().expect("refusal projection");
    assert_eq!(error.code, RuntimeErrorCode::QueueExpired);
    assert_eq!(error.host_code(), Some("lease_queue_expired"));
    // Ledger revision 3 (acceptance H-L3): the refusal's facts are its outcome's values.
    let refusal = run
        .host
        .failure_outcome_for_test(receipt.request_id())
        .expect("refusal outcome")
        .expect("the refusal's outcome");
    assert_eq!(refusal.code().as_str(), "lease_queue_expired");
    assert_eq!(refusal.value("timeout_ms"), Some(&serde_json::json!(5000)));
    assert_eq!(
        refusal.value("holder_kind"),
        Some(&serde_json::json!("client_lease"))
    );
    assert_eq!(refusal.value("queue_position"), Some(&serde_json::json!(1)));
    assert!(
        control_calls(&run.state.emulator.calls()).is_empty(),
        "no provider call"
    );
    let events = request_events(&run.host, &receipt);
    let denied = of_type(&events, EventType::SchedulerDenied);
    assert_eq!(denied.len(), 1, "{events:?}");
    assert_eq!(denied[0].severity(), EventSeverity::Error);
    assert!(matches!(
        denied[0].payload(),
        EventPayload::Scheduler(SchedulerPayload::Denied(payload))
            if payload.diagnostic_code() == DiagnosticCode::LeaseQueueExpired
    ));
    assert!(of_type(&events, EventType::CommandRejected).is_empty());
    assert!(of_type(&events, EventType::CommandValidated).is_empty());
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(holder);
    run.host.close().expect("close host");
}

/// G19 row 2 (`contracts/emulator-control.md`, "Cold start"): a Start binds the port it
/// reported, records `runtime.instance_bound` with it, and records `device.connected`.
#[test]
fn gate_a_start_binds_the_reported_port_records_the_binding_and_device_connected() {
    let run = emulator_host();
    let stopped = control(&run.host, STOP);
    assert_eq!(
        stopped.state(),
        RuntimeReceiptState::Completed,
        "{stopped:?}"
    );
    run.state.emulator.script_control(
        START,
        ScriptedControl::Running {
            adb_port: Some(16448),
        },
    );
    let receipt = control(&run.host, START);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(status_port(&run.host), Some(16448));
    let events = request_events(&run.host, &receipt);
    let validated = of_type(&events, EventType::CommandValidated)[0].sequence();
    let bound = of_type(&events, EventType::RuntimeInstanceBound);
    assert_eq!(bound.len(), 1, "{events:?}");
    assert!(bound[0].sequence() > validated);
    let EventPayload::Runtime(RuntimePayload::InstanceBound(binding)) = bound[0].payload() else {
        panic!("an instance binding: {:?}", bound[0]);
    };
    assert_eq!(binding.adb_port(), Some(16448));
    assert!(connected(&instance_facts(&run.host, run.instance_id)));
    run.host.close().expect("close host");
}

/// G19 row 3: a Stop returns the binding to pending, records `device.connected` false and
/// invalidates it with `device_closed`.
#[test]
fn gate_a_stop_returns_the_binding_to_pending_and_invalidates_device_connected() {
    let run = emulator_host();
    let receipt = control(&run.host, STOP);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(status_port(&run.host), None);
    let events = request_events(&run.host, &receipt);
    let validated = of_type(&events, EventType::CommandValidated)[0].sequence();
    let all = all_events(&run.host);
    let facts = fact_events_after(&all, DEVICE_CONNECTED, validated);
    assert!(facts.iter().any(|event| recorded_false(event)), "{facts:?}");
    assert!(
        facts.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Runtime(RuntimePayload::FactInvalidated(invalidation))
                if invalidation.invalidation().reason == RuntimeFactInvalidationReason::DeviceClosed
        )),
        "{facts:?}"
    );
    assert!(!connected(&instance_facts(&run.host, run.instance_id)));
    run.host.close().expect("close host");
}

/// G19 row 4: an instance reported running without a port fails
/// `emulator_control_endpoint_unresolved`, and the binding stays as it was.
#[test]
fn gate_a_running_start_without_a_port_fails_unresolved_and_keeps_the_binding() {
    let run = emulator_host();
    let stopped = control(&run.host, STOP);
    assert_eq!(
        stopped.state(),
        RuntimeReceiptState::Completed,
        "{stopped:?}"
    );
    run.state
        .emulator
        .script_control(START, ScriptedControl::Running { adb_port: None });
    let receipt = control(&run.host, START);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        host_code(&receipt),
        Some("emulator_control_endpoint_unresolved")
    );
    assert_eq!(status_port(&run.host), None, "the binding stays pending");
    run.host.close().expect("close host");
}

/// Waits until the scripted emulator sees `expected`, passing over the calls before it.
fn wait_for(calls: &mpsc::Receiver<EmulatorCall>, expected: EmulatorCall) {
    loop {
        let call = calls
            .recv_timeout(GATE_WAIT)
            .unwrap_or_else(|error| panic!("waiting for {expected:?}: {error}"));
        if call == expected {
            return;
        }
    }
}

/// RB-H3 (F3, L6, D5): on the ladder's emulator-restart rung a Stop whose readiness wait
/// expired leads on to the Start and the readiness wait; no `recovery_instance_stopped` is
/// written, and the Stop's failure record carries `device_process_present=true`.
#[test]
fn gate_rung_three_starts_after_a_stop_whose_wait_expired_and_waits_for_readiness() {
    let run = emulator_ladder_fixture(0, true);
    let emulator = &run.state.emulator;
    emulator.script_control(STOP, stop_wait_expired());
    emulator.script_read(stopped_state());
    let calls = emulator.listen();
    run.fail_scheduled_run();
    wait_for(&calls, EmulatorCall::Control(START));
    // The readiness wait's boot check, after the Start.
    wait_for(&calls, EmulatorCall::ObserveForeground);
    let LadderRun {
        root, host, state, ..
    } = run;
    host.close().expect("close host during the readiness wait");
    let events = closed_ledger_events(root.path());
    assert_eq!(control_calls(&state.emulator.calls()), [STOP, START]);
    let stop_rejected = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::CommandRejected
                && event.payload().action() == EventAction::EmulatorInstanceStop
        })
        .expect("the Stop's rejection");
    let start_validated = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::CommandValidated
                && event.payload().action() == EventAction::EmulatorInstanceStart
        })
        .expect("the Start's validation");
    assert!(start_validated.sequence() > stop_rejected.sequence());
    let stop_failure = failures(&events)
        .into_iter()
        .find(|(code, _)| code == "emulator_control_wait_timeout")
        .expect("the Stop's failure record");
    assert_tokens(&stop_failure.1, &["device_process_present=true"]);
    assert!(
        !phases(&events).iter().any(|(_, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryInstanceStopped { .. }
        )),
        "no confirmed Stop is claimed"
    );
}

/// RB-H3 (F3): on the rung a Stop whose tool exited non-zero fails the rung with its code and
/// dispatches no Start.
#[test]
fn gate_rung_three_fails_without_a_start_after_a_stop_whose_tool_exited() {
    let run = emulator_ladder_fixture(0, true);
    let emulator = &run.state.emulator;
    emulator.script_control(
        STOP,
        ScriptedControl::Failed {
            stage: "mumu_manager.control_exit",
            exit_code: Some(1),
            last_state: None,
            device_process_present: Some(DeviceProcessPresence::Unknown),
        },
    );
    let calls = emulator.listen();
    run.fail_scheduled_run();
    wait_for(&calls, EmulatorCall::Control(STOP));
    let LadderRun {
        root, host, state, ..
    } = run;
    host.close().expect("close host after the rung");
    let events = closed_ledger_events(root.path());
    assert_eq!(control_calls(&state.emulator.calls()), [STOP]);
    let rungs = rung_outcomes(&events);
    assert_eq!(
        rungs.last(),
        Some(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Failed,
            Some("emulator_control_failed".to_owned())
        )),
        "{rungs:?}"
    );
}
