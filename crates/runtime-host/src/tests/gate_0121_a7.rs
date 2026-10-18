// SPDX-License-Identifier: AGPL-3.0-only

// #411 A7 part 2 (v0.12.1; host gate 105; register §1.98 A7-2): in a preparation's recoverable
// check, a failed capture-backend open (`capture_backend_open_failed`) whose temporary-resource
// disposal is confirmed and that has no failed input-parameter observation is recoverable
// whatever its diagnostic category; every other rule of the check stays, and which codes start a
// ladder stays the configuration's (the shipped trigger item `preparation_backend`).
//
// The fixture is the 10-18 failure's shape: the input open passes and the capture open fails
// fatally with no diagnostic category (a Nemu IPC open that gets no positive connect handle),
// after it closed the one resource it acquired (confirmed disposal). The instance is physical,
// its stuck recovery is on and the ladder configuration is the shipped default.
//
// The self-check's preparation runs on its claim's key, so an admitted ladder takes that key by
// one `lease.transferred` before the receipt (H-1, P1): both tests read that hand-off, or its
// absence, from what the request recorded before its receipt. The ladder's own start record is
// written by the instance's worker and is awaited (bounded).

use super::instance_workers::{all_events, phases, wait_until};
use super::*;
use actingcommand_contract::{
    BackendInputGeometryObservation, BackendInputParameterCheck, BackendObservationStatus,
    BackendOpenAttempt, BackendOpenEntry, BackendOpenReport, BackendOpenSource, BackendOpenStage,
    InstanceStuckRecovery, LeasePayload, RecoveryTriggerStage, RequestId, RuntimeLifecyclePhase,
    SchedulingResumeSelfCheck,
};
use actingcommand_device::{BackendOpenObservation, DeviceResourceQuiescence};

const ALIAS: &str = "a7.instance";
const CAPTURE_OPEN_FAILED: &str = "capture_backend_open_failed";

// A started host whose one physical instance has stuck recovery on and opens that report a
// passed native open, so only the capture open set below fails.
fn host_with_stuck_recovery(root: &TempDir, state: &Arc<FakeState>) -> RuntimeHost {
    state.observed_open.store(true, Ordering::Release);
    RuntimeHost::start(
        config(root).with_stuck_recovery(BTreeMap::from([(
            ALIAS.to_owned(),
            InstanceStuckRecovery {
                enabled: true,
                cooldown_secs: 600,
            },
        )])),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host")
}

// The capture open fails fatally without a diagnostic category, its one acquired resource
// closed (confirmed disposal).
fn uncategorised_capture_open_failure() -> DeviceError {
    DeviceError::fatal("fixture capture open got no positive connection handle")
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
}

// The same failure, whose open observations also hold the input open's, in the shape the
// device's `auto` input selection reports it: the first touch backend's handshake failed its
// input-parameter check transiently, so the selection fell back; the second failed without a
// check; `adb_shell_input` passed its check with the geometry it read, which the report carries.
fn capture_open_failure_with_failed_input_parameters() -> DeviceError {
    let geometry = BackendInputGeometryObservation {
        natural_max_x: 1280,
        natural_max_y: 720,
        rotation_degrees: 0,
    };
    let passed = BackendInputParameterCheck {
        status: BackendObservationStatus::Passed,
        input_geometry: Some(geometry.clone()),
        ..BackendInputParameterCheck::default()
    };
    let attempt = |backend: &str, status, elapsed_ms: u64, input_parameters| BackendOpenAttempt {
        backend: backend.to_owned(),
        stage: BackendOpenStage::Connect,
        status,
        elapsed_ms: Some(elapsed_ms),
        detail: None,
        input_parameters,
    };
    let mut input = BackendOpenReport::unobserved(BackendOpenEntry::Input);
    input.source = BackendOpenSource::Native;
    input.requested = "auto".to_owned();
    input.selected = Some("adb_shell_input".to_owned());
    input.status = BackendObservationStatus::Passed;
    input.connection = BackendObservationStatus::Passed;
    input.input_check = BackendObservationStatus::Passed;
    input.input_geometry = Some(geometry);
    input.attempts = vec![
        attempt(
            "maatouch",
            BackendObservationStatus::Failed,
            40,
            Some(BackendInputParameterCheck::failed()),
        ),
        attempt("minitouch", BackendObservationStatus::Failed, 30, None),
        attempt(
            "adb_shell_input",
            BackendObservationStatus::Passed,
            20,
            Some(passed),
        ),
    ];
    let mut capture = BackendOpenReport::unobserved(BackendOpenEntry::Capture);
    capture.source = BackendOpenSource::Native;
    capture.requested = "nemu_ipc".to_owned();
    capture.status = BackendObservationStatus::Failed;
    uncategorised_capture_open_failure()
        .with_backend_open_observation(BackendOpenObservation::new(input))
        .with_backend_open_observation(BackendOpenObservation::new(capture))
}

fn head(host: &RuntimeHost) -> u64 {
    all_events(host).last().map_or(0, PersistedEvent::sequence)
}

// `SelfCheckInstance` of the instance: its request id and its self-check, which names the
// failed capture open.
fn self_check_fails_its_capture_open(host: &RuntimeHost) -> RequestId {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: ALIAS.to_owned(),
        },
    );
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(105).expect("connection"))
        .expect("self-check receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "a failed preparation does not fail the self-check: {receipt:?}"
    );
    let Some(RuntimeResult::InstanceSelfChecked {
        selfcheck: SchedulingResumeSelfCheck { failure_code, .. },
        ..
    }) = receipt.result()
    else {
        panic!("a self-check result: {receipt:?}");
    };
    assert_eq!(failure_code.as_deref(), Some(CAPTURE_OPEN_FAILED));
    request.request_id()
}

// The one `instance_preparation_finished` recorded after `after`: stage connection_preparation,
// outcome the failed capture open.
fn preparation_finished(events: &[PersistedEvent], after: u64) -> &PersistedEvent {
    let finished = phases(events)
        .into_iter()
        .filter(|(event, _)| event.sequence() > after)
        .filter_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::InstancePreparationFinished { stage, outcome, .. } => {
                Some((event, stage, outcome))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(event, stage, Some(outcome))] = finished.as_slice() else {
        panic!("one failed instance_preparation_finished: {finished:#?}");
    };
    assert_eq!(*stage, RecoveryTriggerStage::ConnectionPreparation);
    assert_eq!(outcome.code().as_str(), CAPTURE_OPEN_FAILED);
    event
}

// The lease the self-check's claim was granted.
fn self_check_lease(events: &[PersistedEvent], request: RequestId) -> LeaseId {
    let granted = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseGranted
                && event.links().request_id() == Some(&request)
        })
        .collect::<Vec<_>>();
    let [granted] = granted.as_slice() else {
        panic!("one lease.granted of the self-check: {granted:#?}");
    };
    *granted.links().lease_id().expect("the self-check's lease")
}

// The request of the claim the lease `from` was transferred to, if it was.
fn transferred_to(events: &[PersistedEvent], from: LeaseId) -> Option<RequestId> {
    events.iter().find_map(|event| match event.payload() {
        EventPayload::Lease(LeasePayload::Transferred(transfer))
            if transfer.from_lease_id() == from =>
        {
            Some(transfer.queued_request_id())
        }
        _ => None,
    })
}

// The 10-18 self-check: its capture open fails with no diagnostic category after a confirmed
// close. The failure is recoverable, so the shipped `preparation_backend` item starts a ladder
// at stage connection_preparation, naming the failed capture open and the self-check's
// preparation; the self-check's key goes to the ladder's claim by one transfer.
#[test]
fn a7_a_self_check_whose_capture_open_fails_starts_the_preparation_backend_ladder() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_stuck_recovery(&root, &state);
    *state.capture_open_error.lock().expect("capture open error") =
        Some(uncategorised_capture_open_failure());
    let before = head(&host);

    let check = self_check_fails_its_capture_open(&host);
    let events = all_events(&host);
    let finished = preparation_finished(&events, before);
    let preparation = TerminalEvent {
        sequence: finished.sequence(),
        event_id: *finished.event_id(),
    };
    let ladder = transferred_to(&events, self_check_lease(&events, check))
        .expect("the self-check's key goes to the ladder its failure admitted");

    wait_until("the ladder's start", || {
        phases(&all_events(&host)).iter().any(|(event, phase)| {
            matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. })
                && event.links().request_id() == Some(&ladder)
        })
    });
    let events = all_events(&host);
    let trigger = phases(&events)
        .into_iter()
        .find_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                if event.links().request_id() == Some(&ladder) =>
            {
                Some(trigger)
            }
            _ => None,
        })
        .expect("recovery_ladder_started");
    assert_eq!(trigger.stage, RecoveryTriggerStage::ConnectionPreparation);
    assert_eq!(trigger.failure_code.as_str(), CAPTURE_OPEN_FAILED);
    assert_eq!(trigger.rule.as_deref(), Some("preparation_backend"));
    assert_eq!(trigger.preparation, Some(preparation));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host during the climb");
}

// The guard: the same failed capture open, whose open observations hold an input attempt that
// failed its input parameters, is a configuration failure and stays outside the recoverable
// check: the preparation admits no ladder, so no ladder claim is made, the self-check's key is
// released and not transferred, and nothing holds or waits for the instance.
#[test]
fn a7_an_input_parameter_failure_still_starts_no_ladder() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_stuck_recovery(&root, &state);
    *state.capture_open_error.lock().expect("capture open error") =
        Some(capture_open_failure_with_failed_input_parameters());
    let before = head(&host);

    let check = self_check_fails_its_capture_open(&host);
    let events = all_events(&host);
    preparation_finished(&events, before);
    let lease = self_check_lease(&events, check);
    assert_eq!(
        transferred_to(&events, lease),
        None,
        "no ladder takes the key"
    );
    let requested = events
        .iter()
        .filter(|event| {
            event.sequence() > before && event.event_type() == EventType::LeaseRequested
        })
        .filter_map(|event| event.links().request_id().copied())
        .collect::<Vec<_>>();
    assert_eq!(requested, vec![check], "only the self-check's own claim");
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseReleased
                    && event.links().lease_id() == Some(&lease)
            })
            .count(),
        1,
        "the self-check's key is released"
    );
    assert_eq!(
        host.instance_claims_for_test(ALIAS).expect("claims"),
        (None, vec![]),
        "nothing holds or waits for the instance"
    );
    assert!(
        phases(&events).iter().all(|(_, phase)| !matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
        )),
        "no ladder"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
