// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #369 S5 (model-369-queue v3.1 §7 row 5, W-3): operator requests wait for their turn
//! in the instance's one queue. Every wait returns as soon as what it waits for is recorded or
//! answered; `GATE_WAIT` only bounds a broken run. The caps themselves (1 s for the direct run
//! below, 3 s for a self-check) are the product's own waits.

use super::*;
use actingcommand_contract::{
    DiagnosticCode, EventFamily, RequestId, RuntimeLifecyclePhase, RuntimePayload,
    RuntimeSubscriptionRequest, SchedulerPayload, SchedulingPauseScope, SubscriptionCursor,
};

const GATE_WAIT: Duration = Duration::from_secs(30);
const ALIAS: &str = "neutral.instance";
const PAUSE_FACT_KEY: &str = "host.scheduling_pause";

/// A Runtime on a frozen clock, so no lease lapses while a request waits.
fn frozen_host(root: &TempDir, instance: InstanceId, state: Arc<FakeState>) -> RuntimeHost {
    RuntimeHost::start(
        config(root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0))),
        Arc::new(FakeProvider::one(ALIAS, instance, state)),
    )
    .expect("runtime host")
}

/// A neutral package for a direct run, with its digest computed as the existing tests do.
pub(super) fn direct_run_package(root: &TempDir, state: &FakeState) -> (String, String) {
    let package = root.path().join("neutral-task.zip");
    let bytes = neutral_contained_task_package(true);
    fs::write(&package, &bytes).expect("write package");
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    (
        package.display().to_string(),
        actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string(),
    )
}

/// Returns once an event of `event_type` is recorded under `request_id` (a ledger
/// subscription that answers as soon as it is).
fn await_recorded(host: &RuntimeHost, event_type: EventType, request_id: RequestId) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let subscription = RuntimeSubscriptionRequest::new(
        EventQuery {
            event_type: Some(event_type),
            request_id: Some(request_id),
            ..EventQuery::default()
        },
        ProjectionProfile::Forensic,
        SubscriptionCursor::default(),
        u64::try_from(GATE_WAIT.as_millis()).expect("gate wait"),
        1,
    )
    .expect("subscription request");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::SubscribeEvents {
                    request: subscription,
                },
            ),
            ConnectionId::new(169).expect("connection"),
        )
        .expect("subscription receipt");
    let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
        panic!("expected an event batch: {receipt:?}");
    };
    assert!(!batch.timed_out(), "{event_type:?} of the waiting request");
}

fn events(host: &RuntimeHost, query: EventQuery) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(query)
        .expect("ledger events")
}

fn of_request(host: &RuntimeHost, request_id: RequestId) -> Vec<PersistedEvent> {
    events(
        host,
        EventQuery {
            request_id: Some(request_id),
            ..EventQuery::default()
        },
    )
}

fn of_correlation(host: &RuntimeHost, request: &RuntimeRequest) -> Vec<PersistedEvent> {
    events(
        host,
        EventQuery {
            correlation_id: Some(request.correlation_id()),
            ..EventQuery::default()
        },
    )
}

fn only(events: &[PersistedEvent], event_type: EventType) -> &PersistedEvent {
    let matching = events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .collect::<Vec<_>>();
    let [event] = matching.as_slice() else {
        panic!("one {event_type:?}: {matching:#?}");
    };
    event
}

fn phase(event: &PersistedEvent) -> Option<RuntimeLifecyclePhase> {
    match event.payload() {
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => Some(value.phase()),
        _ => None,
    }
}

/// The receipt is the refusal W-3 names: Denied `lease_queue_expired`.
fn assert_queue_expired(receipt: &RuntimeReceipt) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let error = receipt.error_projection().expect("refusal projection");
    assert_eq!(error.code, RuntimeErrorCode::QueueExpired);
    assert_eq!(error.host_code(), Some("lease_queue_expired"));
}

/// The one record of the expiry: `scheduler.denied` `lease.queue_expired` under the request's
/// links, naming the instance. Ledger revision 3 (#378 A3): its severity is its outcome's
/// category, `lease_queue_expired`'s `error`.
fn assert_one_expiry_record(host: &RuntimeHost, request: &RuntimeRequest, instance: InstanceId) {
    let recorded = of_request(host, request.request_id());
    let denied = only(&recorded, EventType::SchedulerDenied);
    assert_eq!(denied.severity(), EventSeverity::Error);
    assert!(matches!(
        denied.payload(),
        EventPayload::Scheduler(SchedulerPayload::Denied(payload))
            if payload.diagnostic_code() == DiagnosticCode::LeaseQueueExpired
    ));
    assert_eq!(
        denied.links().correlation_id(),
        Some(&request.correlation_id())
    );
    assert_eq!(denied.links().instance_id(), Some(&instance));
}

#[test]
fn gate_a_direct_run_behind_a_hold_runs_after_the_release() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let (package, expected) = direct_run_package(&root, &state);
    let host = frozen_host(&root, instance_id(), Arc::clone(&state));
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(ALIAS);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package, expected).expect("task request"),
        ),
    );
    let release = holder.request(RuntimeOperation::ReleaseLease { token });

    let (receipt, released) = thread::scope(|scope| {
        let waiting = scope.spawn(|| {
            host.process_request_for_test(&run, ConnectionId::new(171).expect("connection"))
        });
        await_recorded(&host, EventType::SchedulerQueued, run.request_id());
        let released = holder.send(&release);
        let receipt = waiting
            .join()
            .expect("waiting run thread")
            .expect("run receipt");
        (receipt, released)
    });

    assert_eq!(released.state(), RuntimeReceiptState::Completed);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            ..
        })
    ));
    let run_events = of_request(&host, run.request_id());
    let queued = only(&run_events, EventType::SchedulerQueued);
    let release_events = of_request(&host, release.request_id());
    let holder_release = only(&release_events, EventType::LeaseReleased);
    assert!(queued.sequence() < holder_release.sequence());
    let transferred = only(&run_events, EventType::LeaseTransferred);
    assert!(
        !run_events
            .iter()
            .any(|event| event.event_type() == EventType::LeaseGranted),
        "the run is granted by the transfer"
    );
    let tasks = of_correlation(&host, &run)
        .into_iter()
        .filter(|event| event.event_type().family() == EventFamily::Task)
        .collect::<Vec<_>>();
    assert!(!tasks.is_empty());
    assert!(
        tasks
            .iter()
            .all(|event| event.sequence() > transferred.sequence()),
        "the run's task chain follows its grant"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(holder);
    host.close().expect("close host");
}

#[test]
fn gate_a_direct_run_whose_wait_expires_is_denied_without_task_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let (package, expected) = direct_run_package(&root, &state);
    let instance = instance_id();
    let host = frozen_host(&root, instance, Arc::clone(&state));
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(ALIAS);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    // W-3: the cap is min(60 s, 31 s − the 30 s run budget) = 1 s.
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package, expected)
                .expect("task request")
                .with_response_deadline_ms(31_000)
                .expect("response deadline"),
        ),
    );

    let receipt = host
        .process_request_for_test(&run, ConnectionId::new(172).expect("connection"))
        .expect("run receipt");

    assert_queue_expired(&receipt);
    assert_eq!(
        host.failure_detail_for_test(run.request_id())
            .expect("refusal detail")
            .as_deref(),
        Some("timeout_ms=1000 holder_kind=client_lease queue_position=1")
    );
    assert_one_expiry_record(&host, &run, instance);
    assert!(
        !of_correlation(&host, &run)
            .iter()
            .any(|event| event.event_type().family() == EventFamily::Task),
        "an expired wait writes no task event"
    );
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(holder);
    host.close().expect("close host");
}

#[test]
fn gate_a_self_check_behind_a_holder_expires_without_preparation_or_ladder() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let host = frozen_host(&root, instance, Arc::clone(&state));
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(ALIAS);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let check = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: ALIAS.to_owned(),
        },
    );

    let receipt = host
        .process_request_for_test(&check, ConnectionId::new(173).expect("connection"))
        .expect("self-check receipt");

    assert_queue_expired(&receipt);
    assert_eq!(
        host.failure_detail_for_test(check.request_id())
            .expect("refusal detail")
            .as_deref(),
        Some("timeout_ms=3000 holder_kind=client_lease queue_position=1")
    );
    assert_one_expiry_record(&host, &check, instance);
    let preparation = of_correlation(&host, &check)
        .into_iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Runtime(RuntimePayload::DeviceSelfCheck(_))
            ) || matches!(
                phase(event),
                Some(
                    RuntimeLifecyclePhase::BackendOpenObserved
                        | RuntimeLifecyclePhase::InstancePreparationFinished { .. }
                )
            )
        })
        .collect::<Vec<_>>();
    assert!(
        preparation.is_empty(),
        "no preparation ran: {preparation:#?}"
    );
    let ladder = events(&host, EventQuery::default())
        .into_iter()
        .filter(|event| {
            matches!(
                phase(event),
                Some(
                    RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                        | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
                        | RuntimeLifecyclePhase::RecoveryRungFinished { .. }
                        | RuntimeLifecyclePhase::RecoveryLadderFinished { .. }
                )
            )
        })
        .collect::<Vec<_>>();
    assert!(ladder.is_empty(), "no ladder: {ladder:#?}");
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(holder);
    host.close().expect("close host");
}

#[test]
fn gate_a_resume_reconnects_on_its_claim_granted_before_the_lift() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let scope = SchedulingPauseScope::Instance {
        instance_alias: ALIAS.to_owned(),
    };
    let pause = client.request(RuntimeOperation::PauseScheduling {
        scope: scope.clone(),
        pause_reason: actingcommand_contract::SchedulingPauseReason::new("gate_check")
            .expect("pause reason"),
        drain_timeout_ms: 1_000,
    });
    assert_eq!(client.send(&pause).state(), RuntimeReceiptState::Completed);
    let resume = client.request(RuntimeOperation::ResumeScheduling {
        scope,
        expected: None,
    });

    let receipt = client.send(&resume);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(receipt.error_projection().is_none());
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::SchedulingResumed {
            selfcheck: Some(_),
            ..
        })
    ));
    let resumed = of_correlation(&host, &resume);
    let granted = only(&resumed, EventType::LeaseGranted);
    let claim = granted.links().lease_id().expect("the claim's lease");
    let all = events(&host, EventQuery::default());
    let lift = all
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Runtime(RuntimePayload::FactRecorded(payload))
                    if payload.record().key == PAUSE_FACT_KEY
            )
        })
        .max_by_key(|event| event.sequence())
        .expect("the record that lifts the gate");
    assert!(
        granted.sequence() < lift.sequence(),
        "the reconnect claim is granted before the lift"
    );
    let prepared = resumed
        .iter()
        .find(|event| {
            matches!(
                phase(event),
                Some(RuntimeLifecyclePhase::InstancePreparationFinished { .. })
            )
        })
        .expect("the reconnect's preparation");
    assert!(lift.sequence() < prepared.sequence());
    let released = only(&resumed, EventType::LeaseReleased);
    assert_eq!(released.links().lease_id(), Some(claim));
    assert!(
        prepared.sequence() < released.sequence(),
        "the claim's release follows the preparation"
    );
    // No dedicated preparation lease: the claim's is the one lease the resume's records name,
    // and the only lease granted or released on the instance from the claim on.
    assert!(
        resumed
            .iter()
            .filter_map(|event| event.links().lease_id())
            .all(|lease_id| lease_id == claim)
    );
    let from_claim = |event_type: EventType| {
        all.iter()
            .filter(|event| {
                event.event_type() == event_type && event.sequence() >= granted.sequence()
            })
            .map(|event| *event.event_id())
            .collect::<Vec<_>>()
    };
    assert_eq!(from_claim(EventType::LeaseGranted), [*granted.event_id()]);
    assert_eq!(from_claim(EventType::LeaseReleased), [*released.event_id()]);
    assert!(
        !resumed
            .iter()
            .any(|event| event.event_type() == EventType::SchedulerDenied),
        "nothing carries lease_queue_expired"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}
