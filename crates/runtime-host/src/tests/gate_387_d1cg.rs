// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #387 D1c-G, the host rows (Alice 10-14: a code's class follows its real cause; Workflow
// #403: an unconfirmed close of one instance's own resources fences that instance instead of
// ending the whole Runtime).
//
// An ADB child that a ladder's own entry probe could not confirm gone fences the instance the
// ladder climbs on: the rung fails with `instance_resources_fenced`, the instance waits for a
// person, the Runtime is not fatal and the other instance runs. A task frame without an extent
// ends only its run, with `contained_task_geometry_frame_extent_invalid`; the Runtime is not fatal
// and the other instance runs. A scheduled run whose vision provider fails ends with the class-5
// `contained_task_vision_provider_failed`, and its lease end starts no ladder. The same fence
// rule holds for the other ADB checks of one instance's own: the startup package's probe, the
// foreground gate's query and the emulator control's ADB wait each fence their instance, the
// instance waits for a person, the Runtime is not fatal and the other instance runs. Every wait
// returns as soon as what it waits for is recorded; the bound only stops a broken run.

use super::gate_403::{assert_fence, fence_records, status_fence};
use super::instance_workers::{
    admit_ladder_run, ladder_setup, ladder_setup_with_package, phases, rung_outcomes,
    wait_for_ladder_end,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::outcome::LadderClass;
use actingcommand_contract::{
    EmulatorInstanceAction, LeasePayload, RecoveryRung, RecoveryRungOutcome, RecoveryTriggerStage,
    RuntimeLifecyclePhase, StartupPackageDisposition,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_execution_kernel::ExecutionKernelCode;

const WAIT: Duration = Duration::from_secs(120);
const OTHER_ALIAS: &str = "d1cg.instance.b";

// An ADB baseline probe whose adb child could not be confirmed gone.
fn unconfirmed_adb_probe() -> DeviceError {
    DeviceError::fatal("synthetic adb probe left its child unconfirmed")
        .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1)
}

// A direct run of the neutral package on `instance_alias`, as `actingctl` sends it.
fn direct_run(instance_alias: &str, package: &(String, String)) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            instance_alias,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.0.clone(), package.1.clone()).expect("task request"),
        ),
    )
}

// The other instance's direct run completes on its target page.
fn assert_completes(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success,
            ..
        })
    ));
}

// Bug 1. The policy instance's scheduled run fails on pages that stay unknown, so its ladder
// climbs: the return-home rung runs, then the application restart rung's entry probe meets an
// adb child it cannot confirm gone. That fences the instance (one `instance_resources_fenced`
// caused by the probe's failure), the rung fails with the fence's code before any stop, and the
// emulator rung is skipped (no emulator control). The instance waits for a person; the Runtime
// is not fatal and the other instance's run completes.
#[test]
fn d1cg_an_unconfirmed_adb_cleanup_inside_a_ladder_fences_its_instance_and_the_other_runs() {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state.adb_probe_error.lock().expect("adb probe error") = Some(unconfirmed_adb_probe());
    let other_state = Arc::new(FakeState::default());
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let fenced = instance_id();
    // The policy's configured identities are the registered aliases (both instances); only the
    // ladder's instance is available to the policy.
    let mut facts = policy_facts();
    let mut other_identity = facts.instances[0].clone();
    other_identity.instance_id = OTHER_ALIAS.to_owned();
    other_identity.available = false;
    facts.instances.push(other_identity);
    let host = RuntimeHost::start(
        host_config.with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources())),
        Arc::new(FakeProvider::from_entries([
            (POLICY_INSTANCE_ALIAS.to_owned(), fenced, Arc::clone(&state)),
            (
                OTHER_ALIAS.to_owned(),
                instance_id(),
                Arc::clone(&other_state),
            ),
        ])),
    )
    .expect("runtime host with two instances");
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
        .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .expect("the ladder instance's dispatch intent")
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
    state.unknown_capture.store(true, Ordering::Release);

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{}", error.code());
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    wait_until(WAIT, || {
        status_fence(&host, POLICY_INSTANCE_ALIAS).is_some_and(|fence| fence.awaiting_person)
    });

    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let fences = fence_records(&events);
    let [(record, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_eq!(record.links().instance_id(), Some(&fenced));
    assert_fence(
        outcome,
        fenced,
        ExecutionKernelCode::ApplicationAdbBaselineFailed.as_str(),
    );
    let rungs = rung_outcomes(&events);
    assert!(
        rungs
            .iter()
            .any(|(rung, result, _)| *rung == RecoveryRung::ReturnHome
                && *result == RecoveryRungOutcome::Failed),
        "{rungs:?}"
    );
    assert!(
        rungs.contains(&(
            RecoveryRung::ApplicationRestart,
            RecoveryRungOutcome::Failed,
            Some(HostCode::InstanceResourcesFenced.as_str().to_owned()),
        )),
        "{rungs:?}"
    );
    assert!(
        rungs.contains(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Skipped,
            Some(
                HostCode::RecoveryRungSkippedNoEmulatorControl
                    .as_str()
                    .to_owned()
            ),
        )),
        "{rungs:?}"
    );
    // The fence came before the stop: the game was not touched.
    assert_eq!(state.application_count.load(Ordering::Acquire), 0);
    assert_eq!(status_fence(&host, OTHER_ALIAS), None);
    assert!(host.fatal_error().expect("runtime health").is_none());

    let other = host
        .process_request_for_test(
            &direct_run(OTHER_ALIAS, &other_package),
            ConnectionId::new(201).expect("connection"),
        )
        .expect("the other instance's receipt");
    assert_completes(&other);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Bug 2. A frame whose geometry is retained without an extent (a zero width; the capture
// pipeline's frame copy refuses such a frame first, so the hook supplies it) ends that run with
// `contained_task_geometry_frame_extent_invalid`, a non-fatal failure, before any input. The
// Runtime is not fatal and the other instance's run completes.
#[test]
fn d1cg_a_frame_without_an_extent_ends_only_its_run_and_the_other_runs() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_id(),
                Arc::clone(&state),
            ),
            (
                OTHER_ALIAS.to_owned(),
                instance_id(),
                Arc::clone(&other_state),
            ),
        ])),
    )
    .expect("runtime host with two instances");
    host.inject_frame_extent_fault_for_test()
        .expect("inject a frame without an extent");

    let failed = host
        .process_request_for_test(
            &direct_run(POLICY_INSTANCE_ALIAS, &package),
            ConnectionId::new(202).expect("connection"),
        )
        .expect("the run's receipt");

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert!(!failed.error_projection().expect("the run's failure").fatal);
    assert_eq!(
        failed.outcome().code().as_str(),
        HostCode::ContainedTaskGeometryFrameExtentInvalid.as_str()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());

    let other = host
        .process_request_for_test(
            &direct_run(OTHER_ALIAS, &other_package),
            ConnectionId::new(203).expect("connection"),
        )
        .expect("the other instance's receipt");
    assert_completes(&other);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Bug 3. A scheduled run of a package whose entry page is read by OCR, with an OCR provider that
// fails: the run ends with the class-5 `contained_task_vision_provider_failed`, not fatal. Its
// lease end starts no ladder at once (the default `pack_logic` item names class 4 only): the run's
// lease hands its key to no ladder claim and no ladder starts on the task stage. (Whether its
// records later add up to a burst is direction ①'s counting rule, not this row's.)
#[test]
fn d1cg_a_vision_provider_failure_in_a_scheduled_run_starts_no_ladder_at_once() {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) =
        ladder_setup_with_package(root.path(), &neutral_vision_contained_task_package(true));
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let vision = Arc::new(FakeVisionProvider {
        ocr_failure_detail: Some("synthetic OCR provider failure"),
        ..FakeVisionProvider::default()
    });
    let host = RuntimeHost::start(
        host_config,
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(vision.clone()),
        ),
    )
    .expect("runtime host");
    let context = admit_ladder_run(&host, &request);

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");

    assert!(!error.is_fatal(), "{}", error.code());
    assert_eq!(
        error.code(),
        ExecutionKernelCode::ContainedTaskVisionProviderFailed.as_str()
    );
    assert_eq!(
        ExecutionKernelCode::ContainedTaskVisionProviderFailed.ladder_class(),
        LadderClass::EnvironmentFault
    );
    assert!(vision.ocr_calls.load(Ordering::Acquire) > 0);
    // The decision is made inside the failed lease end, before the run returns: a ladder it
    // admitted would hold the run's key by a transfer from the run's lease.
    let run_lease = context.lease_token().lease_id();
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    assert!(
        !events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Lease(LeasePayload::Transferred(transfer))
                if transfer.from_lease_id() == run_lease
        )),
        "the run's lease end hands its key to no ladder"
    );
    assert!(
        !phases(&events).iter().any(|(_, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                if trigger.stage == RecoveryTriggerStage::Task
        )),
        "no ladder starts on the task stage"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The fenced instance (`POLICY_INSTANCE_ALIAS`) and the other one, on the fake provider.
fn two_instances(
    fenced: InstanceId,
    state: &Arc<FakeState>,
    other_state: &Arc<FakeState>,
) -> Arc<FakeProvider> {
    Arc::new(FakeProvider::from_entries([
        (POLICY_INSTANCE_ALIAS.to_owned(), fenced, Arc::clone(state)),
        (
            OTHER_ALIAS.to_owned(),
            instance_id(),
            Arc::clone(other_state),
        ),
    ]))
}

// The fenced instance waits for a person behind a fence caused by `cause`, the other instance
// has no fence and its direct run completes, and the Runtime is not fatal.
fn assert_fenced_and_the_other_runs(
    host: &RuntimeHost,
    fenced: InstanceId,
    cause: &str,
    other_package: &(String, String),
    connection: u64,
) {
    wait_until(WAIT, || {
        status_fence(host, POLICY_INSTANCE_ALIAS).is_some_and(|fence| fence.awaiting_person)
    });
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let fences = fence_records(&events);
    let Some((_, outcome)) = fences
        .iter()
        .find(|(record, _)| record.links().instance_id() == Some(&fenced))
    else {
        panic!("the fenced instance's fence record: {fences:#?}");
    };
    assert_fence(outcome, fenced, cause);
    assert_eq!(status_fence(host, OTHER_ALIAS), None);
    assert!(host.fatal_error().expect("runtime health").is_none());
    let other = host
        .process_request_for_test(
            &direct_run(OTHER_ALIAS, other_package),
            ConnectionId::new(connection).expect("connection"),
        )
        .expect("the other instance's receipt");
    assert_completes(&other);
    assert!(host.fatal_error().expect("runtime health").is_none());
}

// Follow-up site 1: the startup package's ADB probe. A startup run whose probe meets an adb child
// it cannot confirm gone fences its instance before the run, so nothing is captured there; the
// instance waits for a person (the fence's ladder meets the same probe at its application restart
// entry), the Runtime is not fatal and the other instance's run completes.
#[test]
fn d1cg_an_unconfirmed_startup_package_adb_probe_fences_its_instance_and_the_other_runs() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    *state.adb_probe_error.lock().expect("adb probe error") = Some(unconfirmed_adb_probe());
    let fenced = instance_id();
    let startup = ContainedTaskRequest::new(package.0, package.1).expect("startup package");
    let host = RuntimeHost::start(
        config(&root).with_startup_packages(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            startup,
        )])),
        two_instances(fenced, &state, &other_state),
    )
    .expect("runtime host with two instances");

    assert_eq!(
        host.schedule_startup_package_for_test(POLICY_INSTANCE_ALIAS)
            .expect("schedule the startup package"),
        StartupPackageDisposition::Scheduled
    );

    assert_fenced_and_the_other_runs(
        &host,
        fenced,
        ExecutionKernelCode::ApplicationAdbBaselineFailed.as_str(),
        &other_package,
        204,
    );
    assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
    host.close().expect("close host");
}

// Follow-up site 2: the foreground gate's ADB query. A direct run's first tap meets a foreground
// query whose adb child cannot be confirmed gone: the input is refused before it reaches the
// device and the run fails, non-fatally, with `instance_resources_fenced`; the instance waits for
// a person (manual work), the Runtime is not fatal and the other instance's run completes.
#[test]
fn d1cg_an_unconfirmed_foreground_gate_adb_query_fences_its_instance_and_the_other_runs() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    *state
        .foreground_error_once
        .lock()
        .expect("one-shot foreground error") = Some(
        DeviceError::fatal("synthetic foreground query left its child unconfirmed")
            .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1),
    );
    let fenced = instance_id();
    let host = RuntimeHost::start(config(&root), two_instances(fenced, &state, &other_state))
        .expect("runtime host with two instances");

    let failed = host
        .process_request_for_test(
            &direct_run(POLICY_INSTANCE_ALIAS, &package),
            ConnectionId::new(205).expect("connection"),
        )
        .expect("the run's receipt");

    assert!(
        !failed.error_projection().expect("the run's failure").fatal,
        "{failed:?}"
    );
    assert_eq!(
        failed.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert_fenced_and_the_other_runs(
        &host,
        fenced,
        HostCode::ApplicationForegroundUnknown.as_str(),
        &other_package,
        206,
    );
    host.close().expect("close host");
}

// Follow-up site 3: the emulator control's ADB wait. An `actingctl emulator start` whose ADB wait
// meets an adb child it cannot confirm gone fails, non-fatally, with `instance_resources_fenced`;
// the instance waits for a person (the fence's ladder meets the same wait after its own Start),
// the Runtime is not fatal and the other instance's run completes.
#[test]
fn d1cg_an_unconfirmed_emulator_control_adb_wait_fences_its_instance_and_the_other_runs() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    *state.adb_probe_error.lock().expect("adb probe error") = Some(unconfirmed_adb_probe());
    let fenced = instance_id();
    let host = RuntimeHost::start(config(&root), two_instances(fenced, &state, &other_state))
        .expect("runtime host with two instances");
    bind_fake_emulator(&host, POLICY_INSTANCE_ALIAS, fenced, &state);
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let failed = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::ControlEmulatorInstance {
                    instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
                    action: EmulatorInstanceAction::Start,
                },
            ),
            ConnectionId::new(207).expect("connection"),
        )
        .expect("the emulator control's receipt");

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert!(
        !failed
            .error_projection()
            .expect("the control's failure")
            .fatal
    );
    assert_eq!(
        failed.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str()
    );
    assert_fenced_and_the_other_runs(
        &host,
        fenced,
        ExecutionKernelCode::ApplicationAdbBaselineFailed.as_str(),
        &other_package,
        208,
    );
    host.close().expect("close host");
}
