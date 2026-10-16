// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #369 S6c review low (b) (wave 18 coordinator ruling 6; host gate 100): a resume's
// reconnect or a self-check whose preparation fails and admits no stuck-recovery ladder releases
// the instance's queued startup claim (`scheduler.denied lease.queue_cancelled`) before its own
// key's release, as an emulator control's failed preparation does (coordinator ruling on review
// #716, item 1), so no startup run follows on an instance that failed its preparation; the
// failure stays visible in the receipt's self-check and in `instance_preparation_finished`. The
// instance is discovery-bound to the fake emulator and has stuck recovery off, so the failed
// preparation, one the preparation rule calls recoverable, admits no ladder. Nothing here waits:
// every assertion reads what the request recorded before its receipt.

use super::instance_workers::phases;
use super::*;
use actingcommand_contract::{
    DiagnosticCode, EmulatorInstanceAction, InstanceStuckRecovery, RecoveryTriggerStage, RequestId,
    RuntimeLifecyclePhase, SchedulerPayload, SchedulingPauseScope, SchedulingResumeSelfCheck,
    StartupPackageDisposition,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_scheduler::ClaimKind;

const ALIAS: &str = "s6clow.instance";

// A started host whose one physical instance `ALIAS` is discovery-bound to the fake emulator,
// with opens that pass a preparation's self-check, the neutral startup package and stuck
// recovery off for the instance.
fn startup_host_without_ladder(
    root: &TempDir,
    state: &Arc<FakeState>,
    instance: InstanceId,
) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let package_path = root.path().join("startup-package.zip");
    fs::write(&package_path, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    let host = RuntimeHost::start(
        config(root)
            .with_startup_packages(BTreeMap::from([(ALIAS.to_owned(), startup)]))
            .with_stuck_recovery(BTreeMap::from([(
                ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )])),
        Arc::new(FakeProvider::one(ALIAS, instance, Arc::clone(state))),
    )
    .expect("runtime host with the fake emulator");
    bind_fake_emulator(&host, ALIAS, instance, state);
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

fn instance_scope() -> SchedulingPauseScope {
    SchedulingPauseScope::Instance {
        instance_alias: ALIAS.to_owned(),
    }
}

fn resume(host: &RuntimeHost, connection: u64) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::ResumeScheduling {
            scope: instance_scope(),
            expected: None,
        },
        connection,
    )
}

fn all_events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

fn head(host: &RuntimeHost) -> u64 {
    all_events(host).last().map_or(0, PersistedEvent::sequence)
}

fn count(events: &[PersistedEvent], event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .count()
}

// The instance is paused, then `actingctl emulator start` drives the emulator and its preparation
// passes: the startup claim waits queued as a gated entry. Returns the claim's request, found
// through the causation of its `startup_package_scheduled`.
fn queue_startup_claim(host: &RuntimeHost) -> RequestId {
    let paused = send(
        host,
        RuntimeOperation::PauseScheduling {
            scope: instance_scope(),
            pause_reason: actingcommand_contract::SchedulingPauseReason::new("s6clow_pause")
                .expect("pause reason"),
            drain_timeout_ms: 5_000,
        },
        201,
    );
    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
    let started = send(
        host,
        RuntimeOperation::ControlEmulatorInstance {
            instance_alias: ALIAS.to_owned(),
            action: EmulatorInstanceAction::Start,
        },
        202,
    );
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
        host.instance_claims_for_test(ALIAS).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits gated"
    );
    let events = all_events(host);
    let causation = phases(&events)
        .into_iter()
        .rev()
        .find(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. }))
        .and_then(|(event, _)| event.links().causation_id().copied())
        .expect("the scheduling's causation");
    events
        .iter()
        .rev()
        .find(|event| {
            event.event_type() == EventType::LeaseRequested
                && event.links().causation_id() == Some(&causation)
        })
        .and_then(|event| event.links().request_id().copied())
        .expect("the startup claim's request")
}

// From now on every input open fails in a way the preparation rule calls recoverable: with stuck
// recovery on, it would admit a ladder.
fn fail_input_opens(state: &FakeState) {
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
}

// The failure stays visible: the self-check names a failing step, and the preparation recorded
// after `after` is one `instance_preparation_finished` whose outcome is that code. No ladder
// started.
fn assert_failure_visible(host: &RuntimeHost, selfcheck: &SchedulingResumeSelfCheck, after: u64) {
    let code = selfcheck
        .failure_code
        .as_deref()
        .expect("the self-check names the failing step");
    let events = all_events(host);
    let recent = events
        .iter()
        .filter(|event| event.sequence() > after)
        .cloned()
        .collect::<Vec<_>>();
    let finished = phases(&recent)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::InstancePreparationFinished { stage, outcome, .. } => {
                Some((stage, outcome))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(stage, Some(outcome))] = finished.as_slice() else {
        panic!("one failed instance_preparation_finished: {finished:#?}");
    };
    assert_eq!(*stage, RecoveryTriggerStage::ConnectionPreparation);
    assert_eq!(outcome.code().as_str(), code);
    assert!(
        phases(&recent).iter().all(|(_, phase)| !matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
        )),
        "stuck recovery is off: no ladder"
    );
}

// The startup claim was released from the queue: one `scheduler.denied lease.queue_cancelled`
// under its request, and it never got the key.
fn assert_startup_claim_released(host: &RuntimeHost, startup_claim: RequestId) {
    let events = all_events(host);
    let claim = events
        .iter()
        .filter(|event| event.links().request_id() == Some(&startup_claim))
        .collect::<Vec<_>>();
    let cancelled = claim
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Scheduler(SchedulerPayload::Denied(payload))
                    if payload.diagnostic_code() == DiagnosticCode::LeaseQueueCancelled
            )
        })
        .count();
    assert_eq!(cancelled, 1, "one scheduler.denied lease.queue_cancelled");
    assert!(
        claim.iter().all(|event| !matches!(
            event.event_type(),
            EventType::LeaseGranted | EventType::LeaseTransferred
        )),
        "the startup claim never gets the key"
    );
}

// A resume whose reconnect preparation fails with no ladder: the reconnect cancels the startup
// claim the start under the pause left queued, then releases its key, so no startup run follows.
#[test]
fn s6clow_b_a_resume_whose_reconnect_fails_without_a_ladder_releases_the_queued_startup_claim() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = startup_host_without_ladder(&root, &state, instance_id());
    let startup_claim = queue_startup_claim(&host);
    fail_input_opens(&state);
    let before = head(&host);

    let resumed = resume(&host, 203);
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "a failed reconnect does not fail the resume: {resumed:?}"
    );
    let Some(RuntimeResult::SchedulingResumed {
        selfcheck: Some(selfcheck),
        ..
    }) = resumed.result()
    else {
        panic!("a resume with its self-check: {resumed:?}");
    };
    assert_failure_visible(&host, selfcheck, before);
    assert_startup_claim_released(&host, startup_claim);
    assert_eq!(
        host.instance_claims_for_test(ALIAS).expect("claims"),
        (None, vec![]),
        "nothing holds or waits for the instance"
    );
    host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        host.instance_claims_for_test(ALIAS).expect("claims"),
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

// A self-check whose preparation fails with no ladder, while the pause holds the startup claim
// queued: the self-check cancels it, then releases its key; the resume that follows, whose
// reconnect passes, runs no startup package.
#[test]
fn s6clow_b_a_self_check_whose_preparation_fails_without_a_ladder_releases_the_queued_startup_claim()
 {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = startup_host_without_ladder(&root, &state, instance_id());
    let startup_claim = queue_startup_claim(&host);
    fail_input_opens(&state);
    let before = head(&host);

    let checked = send(
        &host,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: ALIAS.to_owned(),
        },
        204,
    );
    assert_eq!(
        checked.state(),
        RuntimeReceiptState::Completed,
        "{checked:?}"
    );
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = checked.result() else {
        panic!("a self-check result: {checked:?}");
    };
    assert_failure_visible(&host, selfcheck, before);
    assert_startup_claim_released(&host, startup_claim);
    assert_eq!(
        host.instance_claims_for_test(ALIAS).expect("claims"),
        (None, vec![]),
        "nothing holds or waits for the instance"
    );

    *state.input_open_error.lock().expect("input open error") = None;
    let resumed = resume(&host, 205);
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    assert!(matches!(
        resumed.result(),
        Some(RuntimeResult::SchedulingResumed {
            selfcheck: Some(SchedulingResumeSelfCheck {
                failure_code: None,
                ..
            }),
            ..
        })
    ));
    host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        host.instance_claims_for_test(ALIAS).expect("claims"),
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
