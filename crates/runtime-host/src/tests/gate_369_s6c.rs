// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #369 S6c (model-369-queue v3.1 §7 row 8, H-5; host gate 86): the configured emulator
// start at daemon start as one claim per instance, tests (1) to (9) of the slice row, on the
// scripted emulator (HOST-I1) and a scripted connectivity status; and the two items the merge
// ledger left for S6c (wave 13 coordinator ruling 6): a routine claim under a restored pause waits
// queued from the first moment (ledger :303, test (5)), and an emulator control whose provider
// step fails cancels the queued startup claim, as a stop and a failed preparation do (ledger :529;
// coordinator ruling on review #716, item 1). Every wait returns as soon as what it waits for is
// read, reported or recorded; `GATE_WAIT` only bounds a broken run.

use super::emulator_controller::EmulatorCall;
use super::instance_workers::phases;
use super::*;
use crate::codes::HostCode;
use crate::host::NetworkConnectivityForTest;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    DiagnosticCode, EmulatorInstanceAction, LeasePayload, RequestId, RuntimeLifecyclePhase,
    RuntimePayload, RuntimeSubscriptionRequest, SchedulerPayload, SchedulingPauseScope,
    StartupPackageDisposition, SubscriptionCursor,
};
use actingcommand_host_metrics::{ConnectivityReason, NetworkConnectivity};
use actingcommand_scheduler::ClaimKind;

const GATE_WAIT: Duration = Duration::from_secs(30);
const FIRST: &str = "autostart.first";
const SECOND: &str = "autostart.second";
const START: EmulatorInstanceAction = EmulatorInstanceAction::Start;
const STOP: EmulatorInstanceAction = EmulatorInstanceAction::Stop;

// A scripted connectivity status: `script` in order, then `then` for every later read. Each read
// is also sent to the returned receiver.
fn connectivity(
    script: Vec<NetworkConnectivity>,
    then: NetworkConnectivity,
) -> (
    NetworkConnectivityForTest,
    mpsc::Receiver<NetworkConnectivity>,
) {
    let script = std::sync::Mutex::new(VecDeque::from(script));
    let (sender, receiver) = mpsc::channel();
    let sender = std::sync::Mutex::new(sender);
    let read: NetworkConnectivityForTest = Arc::new(move || {
        let status = script
            .lock()
            .expect("connectivity script")
            .pop_front()
            .unwrap_or(then);
        let _ = sender.lock().expect("connectivity reads").send(status);
        status
    });
    (read, receiver)
}

// A started host whose instances, in configuration order, all name `start_emulator`; each is
// discovery-bound to its scripted emulator, with opens that pass a preparation's self-check. The
// connectivity status is `read`; `configure` adds to the configuration.
fn autostart_host(
    root: &TempDir,
    instances: &[(&str, InstanceId, &Arc<FakeState>)],
    read: NetworkConnectivityForTest,
    configure: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> RuntimeHost {
    let mut entries = Vec::new();
    for (alias, instance, state) in instances {
        state.physical_task_geometry.store(true, Ordering::Release);
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        state.observed_open.store(true, Ordering::Release);
        entries.push(((*alias).to_owned(), *instance, Arc::clone(state)));
    }
    let configured = instances
        .iter()
        .map(|(alias, _, _)| (*alias).to_owned())
        .collect();
    let host = RuntimeHost::start(
        configure(
            config(root)
                .with_emulator_autostart(configured)
                .with_network_connectivity_for_test(read),
        ),
        Arc::new(FakeProvider::from_entries(entries)),
    )
    .expect("runtime host with configured emulator starts");
    for (alias, instance, state) in instances {
        // A second start on the same root finds the emulator as the first left it.
        if !state.emulator_control.load(Ordering::Acquire) {
            bind_fake_emulator(&host, alias, *instance, state);
        }
    }
    host
}

fn send(host: &RuntimeHost, operation: RuntimeOperation, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.process_request_for_test(
        &runtime_request(&ids, operation),
        ConnectionId::new(connection).expect("connection"),
    )
    .expect("receipt")
}

// `actingctl emulator <action>` of `alias`.
fn control(
    host: &RuntimeHost,
    alias: &str,
    action: EmulatorInstanceAction,
    connection: u64,
) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::ControlEmulatorInstance {
            instance_alias: alias.to_owned(),
            action,
        },
        connection,
    )
}

// The emulator of `alias` is stopped and its binding pending, as at a daemon start whose emulator
// does not run.
fn stop(host: &RuntimeHost, alias: &str, connection: u64) {
    let receipt = control(host, alias, STOP, connection);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

fn pause(host: &RuntimeHost, scope: SchedulingPauseScope, connection: u64) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::PauseScheduling {
            scope,
            pause_reason: actingcommand_contract::SchedulingPauseReason::new("s6c_pause")
                .expect("pause reason"),
            drain_timeout_ms: 5_000,
        },
        connection,
    )
}

fn resume(host: &RuntimeHost, scope: SchedulingPauseScope, connection: u64) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::ResumeScheduling {
            scope,
            expected: None,
        },
        connection,
    )
}

fn instance_scope(alias: &str) -> SchedulingPauseScope {
    SchedulingPauseScope::Instance {
        instance_alias: alias.to_owned(),
    }
}

fn all_events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

fn head(host: &RuntimeHost) -> u64 {
    all_events(host).last().map_or(0, PersistedEvent::sequence)
}

fn of_request(host: &RuntimeHost, request_id: RequestId) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        request_id: Some(request_id),
        ..EventQuery::default()
    })
    .expect("events of the request")
}

fn count(events: &[PersistedEvent], event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .count()
}

fn first_of(events: &[PersistedEvent], event_type: EventType) -> &PersistedEvent {
    events
        .iter()
        .find(|event| event.event_type() == event_type)
        .unwrap_or_else(|| panic!("a {event_type:?}: {events:#?}"))
}

// The requests of the claims requested on `instance` after `after`, in order.
fn claims_after(host: &RuntimeHost, instance: InstanceId, after: u64) -> Vec<RequestId> {
    all_events(host)
        .iter()
        .filter(|event| {
            event.sequence() > after
                && event.event_type() == EventType::LeaseRequested
                && event.links().instance_id() == Some(&instance)
        })
        .filter_map(|event| event.links().request_id().copied())
        .collect()
}

// Returns the sequence of the first `event_type` under `request_id` after `after`, once it is
// recorded (a ledger subscription that answers as soon as it is).
fn await_recorded(
    host: &RuntimeHost,
    event_type: EventType,
    request_id: RequestId,
    after: u64,
) -> u64 {
    let query = EventQuery {
        event_type: Some(event_type),
        request_id: Some(request_id),
        from_sequence: Some(after + 1),
        ..EventQuery::default()
    };
    let subscription = RuntimeSubscriptionRequest::new(
        query.clone(),
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
        169,
    );
    let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
        panic!("expected an event batch: {receipt:?}");
    };
    assert!(!batch.timed_out(), "{event_type:?} of {request_id:?}");
    host.query_persisted_events_for_test(query)
        .expect("the recorded event")
        .first()
        .map(PersistedEvent::sequence)
        .expect("the recorded event")
}

fn codes(lines: &[Outcome]) -> Vec<&str> {
    lines.iter().map(|line| line.code().as_str()).collect()
}

fn reported(lines: &[Outcome], code: HostCode) -> usize {
    lines.iter().filter(|line| line.is(code)).count()
}

// The report lines once `done` holds for them.
fn await_reports(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

fn instance_status(
    host: &RuntimeHost,
    alias: &str,
) -> actingcommand_contract::RuntimeInstanceStatus {
    let receipt = send(host, RuntimeOperation::Status, 168);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == alias)
        .cloned()
        .expect("the instance's status")
}

fn starts(calls: &mpsc::Receiver<EmulatorCall>) -> usize {
    calls
        .try_iter()
        .filter(|call| *call == EmulatorCall::Control(START))
        .count()
}

// (1) The claim holds its instance from the enqueue: status reads `lease_active` and nothing
// queued while the claim waits for the network; nothing is commanded before the Start.
#[test]
fn s6c_1_the_claim_holds_its_instance_from_the_enqueue() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::NoInternet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| config);
    stop(&host, FIRST, 71);
    let before = head(&host);

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (Some(ClaimKind::Autostart), vec![])
    );
    let status = instance_status(&host, FIRST);
    assert!(status.lease_active());
    assert_eq!(status.queued_request_count(), 0);
    let [claim] = claims_after(&host, instance, before)[..] else {
        panic!("one autostart claim");
    };
    let claim = of_request(&host, claim);
    assert_eq!(count(&claim, EventType::LeaseGranted), 1, "{claim:#?}");
    assert_eq!(count(&claim, EventType::CommandReceived), 0, "{claim:#?}");
    let lines = host
        .emulator_autostart_reports()
        .take()
        .expect("report lines");
    assert_eq!(codes(&lines), ["emulator_autostart_queued"]);
    assert_eq!(lines[0].value("count"), Some(&serde_json::json!(1)));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (2) No internet access, then internet access after two reads 5 s apart: the claim starts the
// emulator, reporting the wait it took.
#[test]
fn s6c_2_no_internet_then_internet_after_two_reads_starts_the_emulator() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, reads) = connectivity(
        vec![
            NetworkConnectivity::NoInternet,
            NetworkConnectivity::NoInternet,
        ],
        NetworkConnectivity::Internet,
    );
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    stop(&host, FIRST, 72);
    let calls = state.emulator.listen();
    let before = head(&host);

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    let read_at = || reads.recv_timeout(GATE_WAIT).expect("a connectivity read");
    assert_eq!(read_at(), NetworkConnectivity::NoInternet);
    assert_eq!(starts(&calls), 0, "no Start without internet access");
    clock.advance(5_000);
    assert_eq!(read_at(), NetworkConnectivity::NoInternet);
    clock.advance(5_000);
    assert_eq!(read_at(), NetworkConnectivity::Internet);

    let lines = await_reports(&host, |lines| {
        reported(lines, HostCode::EmulatorAutostartStarted) == 1
    });
    assert_eq!(
        codes(&lines),
        [
            "emulator_autostart_queued",
            "emulator_autostart_network_ready",
            "emulator_autostart_started",
        ]
    );
    assert_eq!(lines[1].value("alias"), Some(&serde_json::json!(FIRST)));
    assert_eq!(
        lines[1].value("elapsed_ms"),
        Some(&serde_json::json!(10_000))
    );
    assert!(reads.try_recv().is_err(), "three reads in all");
    assert_eq!(starts(&calls), 1);
    let claim = of_request(&host, claims_after(&host, instance, before)[0]);
    assert!(
        first_of(&claim, EventType::CommandReceived).sequence()
            < first_of(&claim, EventType::CommandValidated).sequence()
    );
    assert_eq!(count(&claim, EventType::CommandRejected), 0, "{claim:#?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (3) An unknown connectivity status: the claim starts at once and reports it in one line.
#[test]
fn s6c_3_an_unknown_status_starts_at_once_with_one_line() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let (read, reads) = connectivity(
        Vec::new(),
        NetworkConnectivity::Unknown {
            reason: ConnectivityReason::LevelUnknown,
            os_error: None,
        },
    );
    let host = autostart_host(&root, &[(FIRST, instance_id(), &state)], read, |config| {
        config
    });
    stop(&host, FIRST, 73);
    let calls = state.emulator.listen();

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    let lines = await_reports(&host, |lines| {
        reported(lines, HostCode::EmulatorAutostartStarted) == 1
    });
    assert_eq!(
        codes(&lines),
        [
            "emulator_autostart_queued",
            "emulator_autostart_network_status_unknown",
            "emulator_autostart_started",
        ]
    );
    assert_eq!(
        lines[1].value("connectivity_reason"),
        Some(&serde_json::json!("level_unknown"))
    );
    assert_eq!(lines[1].value("os_error"), None);
    assert_eq!(reads.try_iter().count(), 1, "one read, no wait");
    assert_eq!(starts(&calls), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (4) No internet access for 600 s: the start is refused with the existing failure pair,
// `emulator_autostart_network_unready`, and the claim releases its key without a Start. Its key
// never lapses while it waits.
#[test]
fn s6c_4_no_internet_for_600_s_refuses_the_start_with_the_failure_pair() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, reads) = connectivity(Vec::new(), NetworkConnectivity::NoInternet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    stop(&host, FIRST, 74);
    let calls = state.emulator.listen();
    let before = head(&host);

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    reads.recv_timeout(GATE_WAIT).expect("the first read");
    // 24 steps of 25 s, each answered by one read: 600 s in all.
    for _ in 0..24 {
        clock.advance(25_000);
        reads
            .recv_timeout(GATE_WAIT)
            .expect("a read after the step");
    }
    let lines = await_reports(&host, |lines| {
        reported(lines, HostCode::EmulatorAutostartNetworkUnready) == 1
    });
    let unready = lines
        .iter()
        .find(|line| line.is(HostCode::EmulatorAutostartNetworkUnready))
        .expect("the refusal line");
    assert_eq!(unready.value("alias"), Some(&serde_json::json!(FIRST)));
    assert_eq!(
        unready.value("elapsed_ms"),
        Some(&serde_json::json!(600_000))
    );
    let [claim] = claims_after(&host, instance, before)[..] else {
        panic!("one autostart claim");
    };
    let claim = of_request(&host, claim);
    assert_eq!(count(&claim, EventType::CommandReceived), 1, "{claim:#?}");
    let rejected = first_of(&claim, EventType::CommandRejected);
    assert_eq!(rejected.severity(), EventSeverity::Error);
    let failed = first_of(&claim, EventType::RuntimeFailed);
    assert!(matches!(
        failed.payload(),
        EventPayload::Runtime(RuntimePayload::Failed(record))
            if record.outcome().code().as_str() == "emulator_autostart_network_unready"
    ));
    assert_eq!(count(&claim, EventType::LeaseReleased), 1, "{claim:#?}");
    assert!(
        all_events(&host)
            .iter()
            .all(|event| event.event_type() != EventType::LeaseExpired),
        "the waiting claim's key never lapsed"
    );
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![])
    );
    assert_eq!(starts(&calls), 0, "no Start");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (5) Ledger :303: under a restored pause the claim waits queued as a gated entry from the first
// moment (never granted on the free instance) and nothing starts; the resume grants it and the
// emulator starts.
#[test]
fn s6c_5_under_a_restored_pause_the_claim_waits_queued_until_the_resume() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(
        &root,
        &[(FIRST, instance, &state)],
        read.clone(),
        |config| config,
    );
    stop(&host, FIRST, 75);
    assert_eq!(
        pause(&host, SchedulingPauseScope::Global, 76).state(),
        RuntimeReceiptState::Completed
    );
    host.close().expect("close the paused host");

    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| config);
    let (paused, _) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(paused.is_some(), "the global pause is restored");
    let calls = state.emulator.listen();
    let before = head(&host);

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![ClaimKind::Autostart]),
        "the claim waits gated from the first moment"
    );
    host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![ClaimKind::Autostart])
    );
    assert_eq!(instance_status(&host, FIRST).queued_request_count(), 1);
    let [claim] = claims_after(&host, instance, before)[..] else {
        panic!("one autostart claim");
    };
    let held = of_request(&host, claim);
    assert_eq!(count(&held, EventType::SchedulerQueued), 1, "{held:#?}");
    assert_eq!(count(&held, EventType::LeaseGranted), 0, "{held:#?}");
    assert_eq!(starts(&calls), 0, "no Start under the pause");

    assert_eq!(
        resume(&host, SchedulingPauseScope::Global, 77).state(),
        RuntimeReceiptState::Completed
    );
    let lines = await_reports(&host, |lines| {
        reported(lines, HostCode::EmulatorAutostartStarted) == 1
    });
    assert_eq!(reported(&lines, HostCode::EmulatorAutostartYielded), 0);
    assert_eq!(count(&of_request(&host, claim), EventType::LeaseGranted), 1);
    assert_eq!(starts(&calls), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (6) A pause that arrives while the claim waits: the claim yields, queues again as a gated entry
// and releases its key, so the pause completes; nothing starts.
#[test]
fn s6c_6_a_pause_during_the_wait_yields_and_the_pause_completes() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::NoInternet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| config);
    stop(&host, FIRST, 78);
    let calls = state.emulator.listen();
    let before = head(&host);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims").0,
        Some(ClaimKind::Autostart)
    );

    let [claim] = claims_after(&host, instance, before)[..] else {
        panic!("one autostart claim");
    };

    let paused = pause(&host, instance_scope(FIRST), 79);
    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
    let lines = await_reports(&host, |lines| {
        reported(lines, HostCode::EmulatorAutostartYielded) == 1
    });
    assert_eq!(reported(&lines, HostCode::EmulatorAutostartFailed), 0);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![ClaimKind::Autostart]),
        "the claim waits again as a gated entry"
    );
    let claim = of_request(&host, claim);
    assert_eq!(count(&claim, EventType::LeaseReleased), 1, "{claim:#?}");
    assert!(
        all_events(&host)
            .iter()
            .all(|event| event.sequence() <= before
                || event.event_type() != EventType::CommandReceived),
        "no command was received for the instance"
    );
    assert_eq!(starts(&calls), 0, "no Start under the pause");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (7) An operator start while the claim waits supersedes it: the claim releases its key with no
// failure pair, and the operator's start, granted next, drives the emulator.
#[test]
fn s6c_7_a_cli_start_during_the_wait_supersedes_the_claim() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::NoInternet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| config);
    stop(&host, FIRST, 80);
    let calls = state.emulator.listen();
    let before = head(&host);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    let [claim] = claims_after(&host, instance, before)[..] else {
        panic!("one autostart claim");
    };

    let started = control(&host, FIRST, START, 81);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "granted within its 5 s: {started:?}"
    );
    let lines = await_reports(&host, |lines| {
        reported(lines, HostCode::EmulatorAutostartSuperseded) == 1
    });
    assert_eq!(reported(&lines, HostCode::EmulatorAutostartFailed), 0);
    let claim = of_request(&host, claim);
    assert_eq!(count(&claim, EventType::LeaseReleased), 1, "{claim:#?}");
    for event_type in [
        EventType::CommandReceived,
        EventType::CommandRejected,
        EventType::RuntimeFailed,
    ] {
        assert_eq!(count(&claim, event_type), 0, "{event_type:?}: {claim:#?}");
    }
    assert_eq!(starts(&calls), 1, "the operator's Start only");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (8) A started emulator whose connection preparation fails: the claim's key goes to the ladder
// that failure admits, by one transfer, and the line reports the failed preparation.
#[test]
fn s6c_8_a_failed_preparation_hands_the_key_to_the_ladder() {
    use actingcommand_device::DeviceResourceQuiescence;
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| config);
    stop(&host, FIRST, 82);
    *state.input_open_error.lock().expect("input open error") = Some(
        DeviceError::transient("fixture input handshake failed")
            .with_diagnostic(
                DeviceErrorCategory::Handshake,
                crate::codes::HostLocation::DeviceRegistryInputOperation,
            )
            .with_diagnostic_context(
                "adb_shell_input",
                crate::codes::HostLocation::OpenInput,
                DeviceErrorSensitivity::Internal,
            )
            .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1),
    );
    let before = head(&host);

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    let lines = await_reports(&host, |lines| {
        reported(lines, HostCode::EmulatorAutostartPreparationFailed) == 1
    });
    assert_eq!(reported(&lines, HostCode::EmulatorAutostartStarted), 0);
    // The first claim requested is the autostart claim; the ladder's follows at the hand-off.
    let claim = of_request(&host, claims_after(&host, instance, before)[0]);
    let claim_lease = *first_of(&claim, EventType::LeaseGranted)
        .links()
        .lease_id()
        .expect("the claim's lease");
    let events = all_events(&host);
    let transferred = events
        .iter()
        .find_map(|event| match event.payload() {
            EventPayload::Lease(LeasePayload::Transferred(transfer))
                if transfer.from_lease_id() == claim_lease =>
            {
                Some((event.sequence(), transfer.queued_request_id()))
            }
            _ => None,
        })
        .expect("the claim's key is transferred");
    let (transfer_sequence, ladder) = transferred;
    let ladder_claim = of_request(&host, ladder);
    assert_eq!(count(&ladder_claim, EventType::LeaseGranted), 0);
    assert!(first_of(&ladder_claim, EventType::SchedulerQueued).sequence() < transfer_sequence);
    // The ladder records its start under its own claim's request.
    let mut after = 0;
    let trigger = loop {
        let at = await_recorded(&host, EventType::RuntimeLifecycleObserved, ladder, after);
        let recorded = all_events(&host);
        let started = phases(&recorded)
            .into_iter()
            .find_map(|(event, phase)| match phase {
                RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                    if event.sequence() == at =>
                {
                    Some(trigger)
                }
                _ => None,
            });
        if let Some(trigger) = started {
            break trigger;
        }
        after = at;
    };
    assert_eq!(
        trigger.stage,
        actingcommand_contract::RecoveryTriggerStage::ConnectionPreparation
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host during the climb");
}

// (9) Two instances start in configuration order. While the first one's Start takes 200 s, the
// second claim waits for its turn renewing its key, which never lapses; its Start follows.
#[test]
fn s6c_9_two_instances_start_in_configuration_order_and_the_waiting_claim_renews() {
    let root = TempDir::new().expect("tempdir");
    let first_state = Arc::new(FakeState::default());
    let second_state = Arc::new(FakeState::default());
    let (first, second) = (instance_id(), instance_id());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(
        &root,
        &[
            (FIRST, first, &first_state),
            (SECOND, second, &second_state),
        ],
        read,
        |config| config.with_runtime_clock(clock.clone()),
    );
    stop(&host, FIRST, 83);
    stop(&host, SECOND, 84);
    first_state
        .block_emulator_start
        .store(true, Ordering::Release);
    let before = head(&host);

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 2);
    let [first_claim] = claims_after(&host, first, before)[..] else {
        panic!("the first claim");
    };
    let [second_claim] = claims_after(&host, second, before)[..] else {
        panic!("the second claim");
    };
    // The first claim renews before its provider step, then its Start blocks.
    await_recorded(&host, EventType::LeaseRenewed, first_claim, before);
    clock.advance(95_000);
    let renewed = await_recorded(&host, EventType::LeaseRenewed, second_claim, before);
    clock.advance(95_000);
    await_recorded(&host, EventType::LeaseRenewed, second_claim, renewed);
    clock.advance(10_000);
    first_state
        .block_emulator_start
        .store(false, Ordering::Release);

    let lines = await_reports(&host, |lines| {
        reported(lines, HostCode::EmulatorAutostartStarted) == 2
    });
    let started = lines
        .iter()
        .filter(|line| line.is(HostCode::EmulatorAutostartStarted))
        .map(|line| line.value("alias").cloned())
        .collect::<Vec<_>>();
    assert_eq!(
        started,
        [
            Some(serde_json::json!(FIRST)),
            Some(serde_json::json!(SECOND))
        ]
    );
    let first_records = of_request(&host, first_claim);
    let second_records = of_request(&host, second_claim);
    assert!(
        first_of(&first_records, EventType::CommandValidated).sequence()
            < first_of(&second_records, EventType::CommandReceived).sequence(),
        "the second Start follows the first"
    );
    let second_received = first_of(&second_records, EventType::CommandReceived).sequence();
    assert!(
        second_records
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseRenewed
                && event.sequence() < second_received)
            .count()
            >= 2,
        "the second claim renewed while it waited"
    );
    assert!(
        all_events(&host)
            .iter()
            .all(|event| event.event_type() != EventType::LeaseExpired),
        "no key lapsed"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Ledger :529 (coordinator ruling on review #716, item 1): an emulator control whose provider step
// itself fails hands no startup claim over, so it cancels the instance's queued startup claim as a
// stop does; the resume then runs no startup package.
#[test]
fn s6c_a_start_whose_provider_step_fails_cancels_the_queued_startup_claim() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let package = neutral_contained_task_package(true);
    let package_path = root.path().join("startup-package.zip");
    fs::write(&package_path, &package).expect("write startup package");
    let startup = ContainedTaskRequest::new(
        package_path.to_string_lossy().into_owned(),
        format!("{:x}", Sha256::digest(&package)),
    )
    .and_then(|request| request.with_response_deadline_ms(60_000))
    .expect("startup package request");
    let host = RuntimeHost::start(
        config(&root).with_startup_packages(BTreeMap::from([(FIRST.to_owned(), startup)])),
        Arc::new(FakeProvider::one(FIRST, instance, Arc::clone(&state))),
    )
    .expect("runtime host with a startup package");
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    bind_fake_emulator(&host, FIRST, instance, &state);
    assert_eq!(
        pause(&host, instance_scope(FIRST), 85).state(),
        RuntimeReceiptState::Completed
    );
    let started = control(&host, FIRST, START, 86);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    assert!(matches!(
        started.result(),
        Some(RuntimeResult::EmulatorInstanceControlled {
            startup_package: StartupPackageDisposition::Scheduled,
            ..
        })
    ));
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits gated"
    );
    let before = head(&host);

    state.fail_emulator_start.store(true, Ordering::Release);
    let failed = control(&host, FIRST, START, 87);
    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(
        failed.outcome().code().as_str(),
        "emulator_control_wait_timeout"
    );
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![]),
        "the failed start cancelled the startup claim"
    );
    let cancelled = all_events(&host)
        .into_iter()
        .filter(|event| {
            event.sequence() > before
                && matches!(
                    event.payload(),
                    EventPayload::Scheduler(SchedulerPayload::Denied(payload))
                        if payload.diagnostic_code() == DiagnosticCode::LeaseQueueCancelled
                )
        })
        .count();
    assert_eq!(cancelled, 1, "one scheduler.denied lease.queue_cancelled");

    assert_eq!(
        resume(&host, instance_scope(FIRST), 88).state(),
        RuntimeReceiptState::Completed
    );
    host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![])
    );
    assert_eq!(
        count(&all_events(&host), EventType::TaskRequested),
        0,
        "no startup run"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
