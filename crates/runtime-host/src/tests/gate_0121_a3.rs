// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A3 (R-1; host gate 102): a routine entry that a scheduling pause holds back in the
// queue, such as an autostart claim under a restored pause, does no work until a resume, so it no
// longer refuses a shutdown. An accepted shutdown disconnects it (`lease.queue_disconnected`) and
// the next start queues it again under the restored pause. A holder, an eligible entry and a
// routine entry that only a #403 resource fence holds still refuse the shutdown. On the scripted
// emulator (HOST-I1) and a scripted connectivity status, with the S6c helpers; every wait returns
// as soon as what it waits for is read or recorded, `GATE_WAIT` only bounds a broken run.

use super::gate_369_s6c::{
    FIRST, GATE_WAIT, START, autostart_host, claims_after, codes, connectivity, control, head,
    instance_scope, instance_status, of_request, pause, resume, send, starts, stop,
};
use super::*;
use crate::codes::HostCode;
use crate::host::NetworkConnectivityForTest;
use actingcommand_contract::{
    DiagnosticCode, RequestId, SchedulerPayload, SchedulingPauseScope, StartupPackageDisposition,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_scheduler::ClaimKind;

const FENCED: &str = "shutdown.fenced";

// The host started again on `root` after `FIRST`'s emulator was stopped and `scope` paused: the
// pause is restored and the emulator stays off.
fn paused_restart(
    root: &TempDir,
    state: &Arc<FakeState>,
    instance: InstanceId,
    read: &NetworkConnectivityForTest,
    scope: SchedulingPauseScope,
) -> RuntimeHost {
    let host = autostart_host(root, &[(FIRST, instance, state)], read.clone(), |config| {
        config
    });
    stop(&host, FIRST, 201);
    assert_eq!(
        pause(&host, scope, 202).state(),
        RuntimeReceiptState::Completed
    );
    host.close().expect("close the paused host");
    autostart_host(root, &[(FIRST, instance, state)], read.clone(), |config| {
        config
    })
}

// actingd's enqueue at the start: one autostart claim, waiting gated from the first moment.
fn enqueue_gated(host: &RuntimeHost, instance: InstanceId) -> RequestId {
    let before = head(host);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![ClaimKind::Autostart]),
        "the claim waits gated"
    );
    let [claim] = claims_after(host, instance, before)[..] else {
        panic!("one autostart claim");
    };
    claim
}

// `actingctl request-shutdown` of the running host.
fn request_shutdown(host: &RuntimeHost, connection: u64) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::RequestShutdown {
            target: host.runtime_info().shutdown_target(),
        },
        connection,
    )
}

fn assert_accepted(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Admitted,
        "{receipt:?}"
    );
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ShutdownAccepted { .. })
        ),
        "{receipt:?}"
    );
}

fn assert_refused(receipt: &RuntimeReceipt, code: RuntimeErrorCode) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(
        receipt.error_projection().expect("the refusal").code,
        code,
        "{receipt:?}"
    );
}

// Requirement 1 and 3: under its instance's restored pause the autostart claim waits gated and
// does no work, so the shutdown is accepted at once; the Runtime then answers no resume, nothing
// is started, the dropped claim's request ends with `scheduler.denied lease.queue_disconnected`
// and no report line, and the next start restores the pause and queues the claim again, gated.
#[test]
fn a3_a_shutdown_is_accepted_while_a_paused_instance_holds_a_gated_autostart_claim() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = paused_restart(&root, &state, instance, &read, instance_scope(FIRST));
    let calls = state.emulator.listen();
    let reports = host.emulator_autostart_reports();
    let claim = enqueue_gated(&host, instance);
    assert_eq!(instance_status(&host, FIRST).queued_request_count(), 1);

    assert_accepted(&request_shutdown(&host, 203));
    assert_refused(
        &resume(&host, instance_scope(FIRST), 204),
        RuntimeErrorCode::RuntimeUnavailable,
    );
    host.close().expect("close the stopping host");
    assert_eq!(starts(&calls), 0, "no Start");
    assert_eq!(
        codes(&reports.take().expect("report lines")),
        ["emulator_autostart_queued"],
        "the disconnected claim reports nothing"
    );

    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| config);
    let (_, instance_pauses) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(
        instance_pauses.contains_key(FIRST),
        "the instance's pause is restored"
    );
    let dropped = of_request(&host, claim);
    let disconnected = dropped
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Scheduler(SchedulerPayload::Denied(payload))
                    if payload.diagnostic_code() == DiagnosticCode::LeaseQueueDisconnected
            )
        })
        .count();
    assert_eq!(disconnected, 1, "{dropped:#?}");
    assert_eq!(count_granted(&dropped), 0, "{dropped:#?}");
    let again = enqueue_gated(&host, instance);
    assert_ne!(again, claim, "a new claim of the new start");
    assert_eq!(starts(&calls), 0, "no Start under the restored pause");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

fn count_granted(events: &[PersistedEvent]) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == EventType::LeaseGranted)
        .count()
}

// Requirement 4: a resume done before the shutdown decision wins. While another admission on the
// instance keeps the pump from granting it, the resumed claim waits eligible and refuses the
// shutdown; once granted it holds its key (no internet access yet) and still refuses it.
#[test]
fn a3_a_resume_before_the_shutdown_grants_the_claim_and_the_shutdown_stays_busy() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, reads) = connectivity(Vec::new(), NetworkConnectivity::NoInternet);
    let host = paused_restart(&root, &state, instance, &read, SchedulingPauseScope::Global);
    enqueue_gated(&host, instance);

    let admission = host
        .instance_admission_for_test(FIRST)
        .expect("the instance's admission guard");
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        let _guard = admission.lock().expect("hold the instance's admission");
        held_tx.send(()).expect("report the held guard");
        let _ = release_rx.recv();
    });
    held_rx.recv_timeout(GATE_WAIT).expect("the guard is held");
    assert_eq!(
        resume(&host, SchedulingPauseScope::Global, 205).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![ClaimKind::Autostart]),
        "the resumed claim waits eligible"
    );
    assert_refused(&request_shutdown(&host, 206), RuntimeErrorCode::RuntimeBusy);

    release_tx.send(()).expect("release the guard");
    holder.join().expect("the guard's holder");
    assert_eq!(
        reads
            .recv_timeout(GATE_WAIT)
            .expect("the granted claim reads"),
        NetworkConnectivity::NoInternet
    );
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (Some(ClaimKind::Autostart), vec![]),
        "the claim holds its key"
    );
    assert_refused(&request_shutdown(&host, 207), RuntimeErrorCode::RuntimeBusy);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Requirement 1 under the global pause: an operator start passes the gated claim and completes
// with one Start; the claim stays queued and gated, and the shutdown is accepted.
#[test]
fn a3_an_operator_start_passes_the_gated_claim_and_the_shutdown_is_accepted() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = paused_restart(&root, &state, instance, &read, SchedulingPauseScope::Global);
    let calls = state.emulator.listen();
    enqueue_gated(&host, instance);

    let started = control(&host, FIRST, START, 208);
    assert_eq!(
        started.state(),
        RuntimeReceiptState::Completed,
        "{started:?}"
    );
    assert_eq!(starts(&calls), 1, "the operator's Start");
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![ClaimKind::Autostart]),
        "the gated claim waits for a resume"
    );
    assert_accepted(&request_shutdown(&host, 209));
    host.close().expect("close the stopping host");
    assert_eq!(starts(&calls), 0, "no other Start");
}

// Requirement 2: a routine entry that only a #403 resource fence holds still waits (the fence
// lifts without a resume), so it refuses the shutdown. A person's direct run whose input ends with
// an unconfirmed close fences the instance; the startup claim of a start that hands it no key then
// waits behind the fence's routine gate, with no pause held.
#[test]
fn a3_a_fence_gated_routine_entry_still_refuses_a_shutdown() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let startup = ContainedTaskRequest::new(package.clone()).expect("startup package");
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0)))
            .with_startup_packages(BTreeMap::from([(FENCED.to_owned(), startup)])),
        Arc::new(FakeProvider::one(FENCED, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host with a startup package");
    *state.input_error_once.lock().expect("one-shot input error") = Some(
        DeviceError::fatal("synthetic input command left its child unconfirmed")
            .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1),
    );
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            FENCED,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    );
    let failed = host
        .process_request_for_test(&run, ConnectionId::new(210).expect("connection"))
        .expect("run receipt");
    assert_eq!(
        failed.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str(),
        "{failed:?}"
    );
    assert!(
        super::gate_403::status_fence(&host, FENCED).is_some_and(|fence| fence.awaiting_person),
        "the instance is fenced"
    );

    assert_eq!(
        host.schedule_startup_package_for_test(FENCED)
            .expect("schedule the startup package"),
        StartupPackageDisposition::Scheduled
    );
    assert_eq!(
        host.instance_claims_for_test(FENCED).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits behind the fence"
    );
    let (global, instances) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(global.is_none() && instances.is_empty(), "no pause holds");
    assert_refused(&request_shutdown(&host, 211), RuntimeErrorCode::RuntimeBusy);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
