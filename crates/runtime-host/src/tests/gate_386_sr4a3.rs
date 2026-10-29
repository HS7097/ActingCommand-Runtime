// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-4a3 (SCH-M frozen model v1.0 §3.2 D-3, §6 row SR-4a3; frozen scheduling model
//! v1.4.2 §5.2a, §5.3 rule 3, C-3, rows T17, T17b, T17c, T46; `contracts/scheduling-blackout.md`,
//! "Complete silence"). Alice 10-09: 「黑名单就是完全静默，连接断开了也不管」.
//!
//! Every expected value is the model's:
//! - inside a blackout the ladders of a task failure, a startup preparation and a connection
//!   preparation are dropped, never queued: one `recovery_ladder_suppressed` `blackout` with
//!   `until_unix_ms` the blackout's end, nothing raised after it; a ladder claim that waits for
//!   the key when the blackout starts is dropped the same way (T17);
//! - the startup package stays queued with a wake at the end and runs then; the monitor probe
//!   takes no frame and records nothing (T17);
//! - a climbing ladder starts no rung whose time limit plus `quiet_margin_ms` reaches the next
//!   blackout's start, and ends with the suppression `blackout`, releasing its key; a startup
//!   package is not taken out of its queue under the same rule with its own deadline (T17b);
//! - a person's self-check runs inside a blackout; the ladder its failure would raise is dropped
//!   (T17c);
//! - an autostart claim inside a blackout gets no key and starts nothing, its wake the end; the
//!   first round after the end starts the emulator once; the same configuration outside a
//!   blackout starts at once (T46);
//! - a direct run and a Lab operation pass a blackout.
//!
//! Blackouts are `once` entries of the host list or of an instance's own list, all neutral data.
//! The ladder rows run on the Runtime's system clock with entries around the time the test reads;
//! the others on a manual Runtime clock, whose Unix reading alone is moved across a blackout's
//! end. Every wait returns as soon as what it waits for has happened; its bound only stops a
//! broken run.

use super::emulator_controller::EmulatorCall;
use super::gate_369_s6c::{FIRST, GATE_WAIT, autostart_host, codes, connectivity, starts, stop};
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_setup, phases, rung_outcomes, wait_until,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigSettingChange, ContainedLabOperationRequest, EmulatorInstanceAction,
    InstanceStuckRecovery, LabOperationSelection, LabProjectionHint, LeasePayload,
    ObservationFramePlan, RecoveryLadderSuppression, RecoveryRung, RecoveryRungOutcome,
    RecoveryTriggerStage, RuntimeConfigManifest, RuntimeLifecyclePhase, SchedulingResumeSelfCheck,
    StartupPackageDisposition,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

/// The manual Runtime clock's Unix reading at the start of a manual-clock row.
const NOW_UNIX_MS: u64 = POLICY_NOW_UNIX_MS;
/// A blackout's length in the ladder rows (system clock): far longer than a test runs.
const HOUR_MS: u64 = 3_600_000;
const CAPTURE_OPEN_FAILED: &str = "capture_backend_open_failed";
const SECOND: &str = "sr4a3.autostart.second";
const SILENT: &str = "sr4a3.instance";
/// Pixel 0 of the Lab package's `home` page.
const HOME: [u8; 3] = [255, 0, 0];

/// A neutral `once` blackout entry `[from_unix_ms, until_unix_ms)`.
fn once(id: &str, from_unix_ms: u64, until_unix_ms: u64) -> Value {
    json!({"kind": "once", "id": id, "from_unix_ms": from_unix_ms, "until_unix_ms": until_unix_ms})
}

fn instance_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// The configuration actingd assembles over the defaults file: the `recovery_ladder` section's
/// members `ladder`, the host list `host`, and for each instance of `own` its own list.
fn silence_manifest(
    ladder: Value,
    host: Value,
    own: &[(InstanceId, Value)],
) -> RuntimeConfigManifest {
    let ids = own
        .iter()
        .map(|(instance, _)| instance_text(*instance))
        .collect::<Vec<_>>();
    let instances = own
        .iter()
        .zip(&ids)
        .map(|((_, list), id)| json!({"instance_id": id, "scheduling": {"blackout": list}}))
        .collect::<Vec<_>>();
    let file = json!({
        "recovery_ladder": ladder,
        "scheduling": {"blackout": {"host": host}},
        "instances": instances,
    });
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &ids, &defaults, &file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// `ConfigureSettings` of the host list: the blackout table changes while the Runtime runs.
fn set_host_blackouts(host: &RuntimeHost, entries: Value, connection: u64) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::ConfigureSettings {
                    expected_version: None,
                    changes: vec![ConfigSettingChange {
                        key: "scheduling.blackout.host".to_owned(),
                        value: Some(entries),
                    }],
                },
            ),
            ConnectionId::new(connection).expect("connection"),
        )
        .expect("configure receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// Every `recovery_ladder_suppressed` as (reason, until).
fn suppressions(events: &[PersistedEvent]) -> Vec<(RecoveryLadderSuppression, u64)> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                reason,
                until_unix_ms,
                ..
            } => Some((reason, until_unix_ms)),
            _ => None,
        })
        .collect()
}

/// Whether a ladder started or finished.
fn ladder_climbed(events: &[PersistedEvent]) -> bool {
    phases(events).iter().any(|(_, phase)| {
        matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                | RuntimeLifecyclePhase::RecoveryLadderFinished { .. }
        )
    })
}

fn claims(host: &RuntimeHost, alias: &str) -> (Option<ClaimKind>, Vec<ClaimKind>) {
    host.instance_claims_for_test(alias)
        .expect("the instance's claims")
}

/// The ladder fixture of `instance_workers` (a scheduled physical run that binds a return-home
/// package, on an instance with a startup package and no emulator control) on a host whose
/// configuration is `manifest`, its run admitted.
fn admitted_routine_run(
    manifest: RuntimeConfigManifest,
) -> (
    TempDir,
    RuntimeHost,
    Arc<FakeState>,
    Box<PolicyRunContext>,
    ContainedTaskRequest,
) {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        host_config.with_config_manifest(manifest),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("physical runtime host");
    let context = admit_ladder_run(&host, &request);
    (root, host, state, context, request)
}

/// The admitted run's captures turn unknown and it fails, at the task stage.
fn fail_run(
    host: &RuntimeHost,
    state: &FakeState,
    context: &PolicyRunContext,
    request: &ContainedTaskRequest,
) {
    state.unknown_capture.store(true, Ordering::Release);
    let error = host
        .run_scheduled_contained_task(context, request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{}", error.code());
}

// T17, the task stage: a policy run admitted before the blackout table changed fails inside a
// blackout. Its ladder is dropped at its admission with `blackout` until the blackout's end:
// nothing climbs, no claim waits and the run's key goes nowhere, so nothing can be raised after
// the end either.
#[test]
fn sr4a3_t17_a_a_failed_runs_ladder_inside_a_blackout_is_dropped_never_queued() {
    let (_root, host, state, context, request) =
        admitted_routine_run(silence_manifest(json!({}), json!([]), &[]));
    let now = unix_ms_now().expect("wall clock");
    let until = now + HOUR_MS;
    set_host_blackouts(&host, json!([once("t17.task", now - 60_000, until)]), 1701);

    fail_run(&host, &state, &context, &request);

    let events = all_events(&host);
    assert_eq!(
        suppressions(&events),
        vec![(RecoveryLadderSuppression::Blackout, until)],
        "one drop, until the blackout's end"
    );
    assert!(!ladder_climbed(&events), "no ladder started");
    assert!(
        !events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Lease(LeasePayload::Transferred(_))
        )),
        "the run's key went to no ladder"
    );
    assert_eq!(claims(&host, POLICY_INSTANCE_ALIAS), (None, vec![]));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

// The capture open fails without a diagnostic category after a confirmed close (the A7 shape the
// shipped trigger item `preparation_backend` matches at both preparation stages).
fn uncategorised_capture_open_failure() -> DeviceError {
    DeviceError::fatal("fixture capture open got no positive connection handle")
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
}

/// A host whose one physical instance `SILENT` has stuck recovery on, a bound device endpoint (so
/// the daemon start prepares and self-checks it, as a configured instance) and opens that report a
/// passed native open, configured with `manifest`; `capture_open` is set before the start.
fn preparation_host(
    root: &TempDir,
    state: &Arc<FakeState>,
    manifest: RuntimeConfigManifest,
    capture_open: Option<DeviceError>,
) -> RuntimeHost {
    state.observed_open.store(true, Ordering::Release);
    *state.capture_open_error.lock().expect("capture open error") = capture_open;
    RuntimeHost::start(
        config(root)
            .with_stuck_recovery(BTreeMap::from([(
                SILENT.to_owned(),
                InstanceStuckRecovery {
                    enabled: true,
                    cooldown_secs: 600,
                },
            )]))
            .with_config_manifest(manifest),
        Arc::new(
            FakeProvider::one(SILENT, instance_id(), Arc::clone(state)).with_device_endpoints(),
        ),
    )
    .expect("runtime host")
}

/// The stage and outcome code of every failed `instance_preparation_finished`.
fn failed_preparations(events: &[PersistedEvent]) -> Vec<(RecoveryTriggerStage, String)> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::InstancePreparationFinished {
                stage,
                outcome: Some(outcome),
                ..
            } => Some((stage, outcome.code().as_str().to_owned())),
            _ => None,
        })
        .collect()
}

// T17, the startup preparation: the daemon starts inside a blackout and its preparation of the
// instance fails. The failure is observed and recorded as usual; the ladder it would raise is
// dropped with `blackout` until the end, and no claim waits.
#[test]
fn sr4a3_t17_b_a_failed_daemon_start_preparation_inside_a_blackout_raises_no_ladder() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let now = unix_ms_now().expect("wall clock");
    let until = now + HOUR_MS;
    let host = preparation_host(
        &root,
        &state,
        silence_manifest(
            json!({}),
            json!([once("t17.start", now - 60_000, until)]),
            &[],
        ),
        Some(uncategorised_capture_open_failure()),
    );

    let events = all_events(&host);
    assert_eq!(
        failed_preparations(&events),
        vec![(
            RecoveryTriggerStage::StartupPreparation,
            CAPTURE_OPEN_FAILED.to_owned()
        )],
        "the preparation ran and its failure is recorded"
    );
    assert_eq!(
        suppressions(&events),
        vec![(RecoveryLadderSuppression::Blackout, until)]
    );
    assert!(!ladder_climbed(&events), "no ladder started");
    assert_eq!(claims(&host, SILENT), (None, vec![]));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

// T17c, and T17's connection preparation: a person's reconnect self-check inside a blackout is
// carried out (its preparation runs and is recorded); its failure raises no ladder: the ladder is
// dropped with `blackout` until the end, and the self-check's key goes to none.
#[test]
fn sr4a3_t17c_a_persons_self_check_runs_inside_a_blackout_and_raises_no_ladder() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let now = unix_ms_now().expect("wall clock");
    let until = now + HOUR_MS;
    let host = preparation_host(
        &root,
        &state,
        silence_manifest(
            json!({}),
            json!([once("t17c.check", now - 60_000, until)]),
            &[],
        ),
        None,
    );
    *state.capture_open_error.lock().expect("capture open error") =
        Some(uncategorised_capture_open_failure());
    let before = all_events(&host).last().map_or(0, PersistedEvent::sequence);
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::SelfCheckInstance {
                    instance_alias: SILENT.to_owned(),
                },
            ),
            ConnectionId::new(1703).expect("connection"),
        )
        .expect("self-check receipt");

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::InstanceSelfChecked {
        selfcheck: SchedulingResumeSelfCheck { failure_code, .. },
        ..
    }) = receipt.result()
    else {
        panic!("a self-check result: {receipt:?}");
    };
    assert_eq!(failure_code.as_deref(), Some(CAPTURE_OPEN_FAILED));
    let events = all_events(&host)
        .into_iter()
        .filter(|event| event.sequence() > before)
        .collect::<Vec<_>>();
    assert_eq!(
        failed_preparations(&events),
        vec![(
            RecoveryTriggerStage::ConnectionPreparation,
            CAPTURE_OPEN_FAILED.to_owned()
        )],
        "the self-check's preparation ran"
    );
    assert_eq!(
        suppressions(&events),
        vec![(RecoveryLadderSuppression::Blackout, until)]
    );
    assert!(!ladder_climbed(&events), "no ladder started");
    assert!(
        !events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Lease(LeasePayload::Transferred(_))
        )),
        "the self-check's key went to no ladder"
    );
    assert_eq!(claims(&host, SILENT), (None, vec![]));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

// T17 (v1.3 rule 16): a ladder claim that waits for the key when a blackout starts (one that
// waits for performance release) is dropped with `blackout` until the end; released afterwards,
// the instance climbs no ladder.
#[test]
fn sr4a3_t17_d_a_ladder_claim_waiting_when_a_blackout_starts_is_dropped() {
    let (_root, host, state, context, request) = admitted_routine_run(silence_manifest(
        json!({"triggers": [{
            "id": "after_release",
            "stages": ["task"],
            "classes": ["pack_logic_mismatch"],
            "start": "after_performance_release",
        }]}),
        json!([]),
        &[],
    ));
    host.hold_performance_for_test(true)
        .expect("hold the instance back from release");
    fail_run(&host, &state, &context, &request);
    let (holder, queued) = claims(&host, POLICY_INSTANCE_ALIAS);
    assert_eq!(holder, None);
    assert_eq!(queued, vec![ClaimKind::Ladder], "the ladder's claim waits");
    let now = unix_ms_now().expect("wall clock");
    let until = now + HOUR_MS;

    set_host_blackouts(&host, json!([once("t17.wait", now - 60_000, until)]), 1704);

    wait_until("the waiting ladder claim's drop", || {
        claims(&host, POLICY_INSTANCE_ALIAS) == (None, vec![])
    });
    let events = all_events(&host);
    assert_eq!(
        suppressions(&events),
        vec![(RecoveryLadderSuppression::Blackout, until)]
    );
    host.hold_performance_for_test(false)
        .expect("release the instance");
    assert_eq!(claims(&host, POLICY_INSTANCE_ALIAS), (None, vec![]));
    assert!(!ladder_climbed(&all_events(&host)), "no ladder climbs");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

// T17b, the ladder: the next blackout starts 180 s ahead. Rung 1 (return home, a bound package:
// its 60 s deadline plus the 60 s margin) still fits and runs; rung 2 (application restart: its
// 90 s entry hold and the 60 s startup package plus the margin) would reach the start, so it does
// not start: the ladder ends with `blackout` until the blackout's end and releases its key.
#[test]
fn sr4a3_t17b_a_a_climbing_ladder_starts_no_rung_whose_limit_reaches_the_next_blackout() {
    let (_root, host, state, context, request) =
        admitted_routine_run(silence_manifest(json!({}), json!([]), &[]));
    let now = unix_ms_now().expect("wall clock");
    let (from, until) = (now + 180_000, now + HOUR_MS);
    set_host_blackouts(&host, json!([once("t17b.ladder", from, until)]), 1705);

    fail_run(&host, &state, &context, &request);

    wait_until("the ladder's end and its release", || {
        !suppressions(&all_events(&host)).is_empty()
            && claims(&host, POLICY_INSTANCE_ALIAS).0.is_none()
    });
    let events = all_events(&host);
    assert!(
        phases(&events)
            .iter()
            .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. })),
        "the ladder started"
    );
    let rungs = rung_outcomes(&events)
        .into_iter()
        .map(|(rung, result, _)| (rung, result))
        .collect::<Vec<_>>();
    assert!(
        !rungs.is_empty()
            && rungs
                .iter()
                .all(|(rung, result)| *rung == RecoveryRung::ReturnHome
                    && *result == RecoveryRungOutcome::Failed),
        "only rung 1 ran: {rungs:?}"
    );
    assert_eq!(
        suppressions(&events),
        vec![(RecoveryLadderSuppression::Blackout, until)],
        "the ladder ends with the suppression"
    );
    assert!(
        !phases(&events).iter().any(|(_, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryLadderFinished { .. }
        )),
        "it is not continued"
    );
    assert_eq!(claims(&host, POLICY_INSTANCE_ALIAS), (None, vec![]));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

/// A host on the manual clock `clock` whose one physical instance `SILENT` has the neutral
/// startup package (deadline 60 s), configured with `manifest`.
fn startup_host(
    root: &TempDir,
    state: &Arc<FakeState>,
    clock: &Arc<ManualRuntimeClock>,
    instance: InstanceId,
    manifest: RuntimeConfigManifest,
) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let package = root.path().join("startup-package.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_startup_packages(BTreeMap::from([(SILENT.to_owned(), startup)]))
            .with_config_manifest(manifest),
        Arc::new(FakeProvider::one(SILENT, instance, Arc::clone(state))),
    )
    .expect("runtime host with a startup package")
}

/// The completed contained runs.
fn completed_runs(host: &RuntimeHost) -> usize {
    all_events(host)
        .iter()
        .filter(|event| event.event_type() == EventType::TaskCompleted)
        .count()
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

// T17, the startup package, and a direct run: inside a blackout of the instance's own list the
// startup package stays queued (no key); a person's direct run passes and completes meanwhile.
// At the blackout's end (the clock's wake) the startup package is granted and runs, and nothing
// waits any more.
#[test]
fn sr4a3_t17_e_inside_a_blackout_the_startup_package_waits_for_the_end_and_a_direct_run_passes() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let instance = instance_id();
    let until = NOW_UNIX_MS + 100_000;
    let host = startup_host(
        &root,
        &state,
        &clock,
        instance,
        silence_manifest(
            json!({}),
            json!([]),
            &[(
                instance,
                json!([once("t17.own", NOW_UNIX_MS - 1_000, until)]),
            )],
        ),
    );

    assert_eq!(
        host.schedule_startup_package_for_test(SILENT)
            .expect("schedule the startup package"),
        StartupPackageDisposition::Scheduled
    );
    assert_eq!(
        claims(&host, SILENT),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits, no key"
    );
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let receipt = direct_run(&host, SILENT, &package, 1706);
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
    assert_eq!(completed_runs(&host), 1, "only the direct run ran");
    assert_eq!(
        claims(&host, SILENT),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim still waits"
    );

    clock.set_unix_ms(until);

    wait_until("the startup run at the blackout's end", || {
        completed_runs(&host) == 2 && claims(&host, SILENT) == (None, vec![])
    });
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

// T17b, the startup package: the next blackout starts 100 s ahead, before the package's 60 s
// deadline plus the 60 s margin could end: the claim is not taken out of its queue. Past the
// blackout's end it runs.
#[test]
fn sr4a3_t17b_b_a_startup_package_waits_when_its_deadline_reaches_the_next_blackout() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let until = NOW_UNIX_MS + 200_000;
    let host = startup_host(
        &root,
        &state,
        &clock,
        instance_id(),
        silence_manifest(
            json!({}),
            json!([once("t17b.startup", NOW_UNIX_MS + 100_000, until)]),
            &[],
        ),
    );

    assert_eq!(
        host.schedule_startup_package_for_test(SILENT)
            .expect("schedule the startup package"),
        StartupPackageDisposition::Scheduled
    );
    assert_eq!(
        claims(&host, SILENT),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim is put back to wait"
    );
    assert_eq!(completed_runs(&host), 0);

    clock.set_unix_ms(until);

    wait_until("the startup run after the blackout", || {
        completed_runs(&host) == 1 && claims(&host, SILENT) == (None, vec![])
    });
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

// T17, the monitor probe: inside a blackout a due probe takes no frame and records nothing, not
// even its request; it stays due and is taken once the blackout has ended.
#[test]
fn sr4a3_t17_f_the_monitor_probe_takes_no_frame_inside_a_blackout() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let until = NOW_UNIX_MS + 100_000;
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_config_manifest(silence_manifest(
                json!({}),
                json!([once("t17.monitor", NOW_UNIX_MS - 1_000, until)]),
                &[],
            )),
        Arc::new(FakeProvider::one(SILENT, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: SILENT.to_owned(),
        policy: RuntimeMonitorPolicy::new(200, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);

    wait_until("three probes passed over", || {
        host.monitor_probes_silenced_for_test()
            .expect("silenced probes")
            >= 3
    });
    assert_eq!(
        state.monitor_observation_count.load(Ordering::Acquire),
        0,
        "no frame inside the blackout"
    );
    let monitor_records = |host: &RuntimeHost| {
        all_events(host)
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type(),
                    EventType::MonitorProbeRequested
                        | EventType::MonitorProbeStarted
                        | EventType::MonitorProbeCompleted
                        | EventType::MonitorProbeFailed
                )
            })
            .count()
    };
    assert_eq!(monitor_records(&host), 0, "nothing recorded");

    clock.set_unix_ms(until);

    wait_until("the probe after the blackout", || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 1
    });
    assert!(monitor_records(&host) > 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

// A Lab operation passes a blackout: inside one of the host list, a Lab `do` (a tap by
// coordinates, no arrival) is carried out on the instance.
#[test]
fn sr4a3_g_a_lab_operation_passes_a_blackout() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        |_capture: usize, _inputs: usize| -> Vec<(usize, [u8; 3])> { vec![(0, HOME)] },
    ));
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock)
            .with_config_manifest(silence_manifest(
                json!({}),
                json!([once("g.lab", NOW_UNIX_MS - 1_000, NOW_UNIX_MS + 100_000)]),
                &[],
            )),
        Arc::new(FakeProvider::one(SILENT, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let package = super::gate_411_bt_l18::write_package(&root);
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Lab client");

    let verified = client
        .begin_debug_session()
        .expect("debug session")
        .run_contained_lab_operation(
            SILENT,
            ContainedLabOperationRequest {
                package_path: package.to_str().expect("path").to_owned(),
                selection: LabOperationSelection::Coordinates {
                    action: InputAction::Tap { x: 1, y: 0 },
                },
                projection_hint: LabProjectionHint {
                    sequence: None,
                    content_sha256: None,
                },
                after: None,
            },
            ObservationFramePlan::SINGLE,
        )
        .expect("the client verifies the operation");

    let record = &verified.operation().record;
    assert!(record.failure.is_none(), "{:?}", record.failure);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1, "the tap");
    drop(client);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

/// The report lines once `done` holds for them.
fn await_reports(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

fn started(lines: &[Outcome]) -> Vec<String> {
    lines
        .iter()
        .filter(|line| line.is(HostCode::EmulatorAutostartStarted))
        .filter_map(|line| {
            line.value("alias")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

// T46: the daemon starts inside a blackout of FIRST's own list with both configured emulators
// stopped. FIRST's autostart claim gets no key and its emulator is not started (zero Start
// calls); it waits queued. SECOND, the same configuration outside any blackout, starts at once.
// At FIRST's blackout end (its wake) the first round starts FIRST's emulator once.
#[test]
fn sr4a3_t46_an_autostart_inside_a_blackout_takes_no_key_and_starts_once_after_it() {
    let root = TempDir::new().expect("tempdir");
    let first_state = Arc::new(FakeState::default());
    let second_state = Arc::new(FakeState::default());
    let (first, second) = (instance_id(), instance_id());
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let until = NOW_UNIX_MS + 100_000;
    let manifest = silence_manifest(
        json!({}),
        json!([]),
        &[(
            first,
            json!([once("t46.first", NOW_UNIX_MS - 1_000, until)]),
        )],
    );
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(
        &root,
        &[
            (FIRST, first, &first_state),
            (SECOND, second, &second_state),
        ],
        read,
        |config| {
            config
                .with_runtime_clock(clock.clone())
                .with_config_manifest(manifest)
        },
    );
    stop(&host, FIRST, 1746);
    stop(&host, SECOND, 1747);
    let first_calls = first_state.emulator.listen();
    let second_calls = second_state.emulator.listen();

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 2);

    let lines = await_reports(&host, |lines| {
        lines
            .iter()
            .any(|line| line.is(HostCode::EmulatorAutostartStarted))
    });
    assert_eq!(
        started(&lines),
        vec![SECOND.to_owned()],
        "{:?}",
        codes(&lines)
    );
    assert_eq!(starts(&second_calls), 1, "outside the blackout: at once");
    assert_eq!(
        claims(&host, FIRST),
        (None, vec![ClaimKind::Autostart]),
        "no key: the claim waits queued"
    );
    assert_eq!(starts(&first_calls), 0, "no Start inside the blackout");

    clock.set_unix_ms(until);

    let lines = await_reports(&host, |lines| started(lines).len() == 2);
    assert_eq!(
        started(&lines),
        vec![SECOND.to_owned(), FIRST.to_owned()],
        "{:?}",
        codes(&lines)
    );
    assert_eq!(starts(&first_calls), 1, "one Start after the blackout");
    let calls = first_state.emulator.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| **call == EmulatorCall::Control(EmulatorInstanceAction::Start))
            .count(),
        1
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
