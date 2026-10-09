// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #369 S2+S3b (model-369-queue v3.1 §7 row 4): the per-instance workers, a failed
//! policy run's hand-off to its ladder, the ladder's one continuous hold, and the startup
//! claim. The ladder's emulator-restart rung runs only on a fake given emulator control and a
//! discovery binding (`bind_fake_emulator`, review C-2); elsewhere it is skipped. An emulator
//! start's startup claim is driven through the control's own tail
//! (`schedule_startup_package_for_test`).

use super::*;
use actingcommand_contract::{
    InstallTransitionAction, InstallTransitionPhase, LeasePayload, RecoveryLadderOutcome,
    RecoveryRung, RecoveryRungOutcome, RuntimeLifecyclePhase, RuntimePayload, SchedulerPayload,
    SchedulingPauseScope, StartupPackageDisposition,
};
use actingcommand_scheduler::ClaimKind;

const WAIT: Duration = Duration::from_secs(120);
const STARTUP_ALIAS: &str = "startup.instance";

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn all_events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

fn phases(events: &[PersistedEvent]) -> Vec<(&PersistedEvent, RuntimeLifecyclePhase)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => {
                Some((event, value.phase()))
            }
            _ => None,
        })
        .collect()
}

fn ladder_finished(
    events: &[PersistedEvent],
) -> Option<(&PersistedEvent, RecoveryLadderOutcome, u8)> {
    phases(events)
        .into_iter()
        .find_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderFinished {
                outcome,
                rungs_tried,
            } => Some((event, outcome, rungs_tried)),
            _ => None,
        })
}

fn rung_outcomes(
    events: &[PersistedEvent],
) -> Vec<(RecoveryRung, RecoveryRungOutcome, Option<String>)> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryRungFinished {
                rung,
                outcome,
                reason,
                ..
            } => Some((rung, outcome, reason)),
            _ => None,
        })
        .collect()
}

/// The diagnostic message of a `runtime.failed` record.
fn failure_message(event: &PersistedEvent) -> Option<&str> {
    match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(record)) => {
            record.detail().map(|detail| detail.message())
        }
        _ => None,
    }
}

fn count(events: &[PersistedEvent], event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .count()
}

/// The ladder finished and released its key: nothing holds the instance any more.
fn wait_for_ladder_end(host: &RuntimeHost, instance_alias: &str) {
    wait_until("the ladder's end and its release", || {
        ladder_finished(&all_events(host)).is_some()
            && host
                .instance_claims_for_test(instance_alias)
                .expect("instance claims")
                .0
                .is_none()
    });
}

/// A rung run is in flight: the ladder queued its continuation just before it.
fn wait_for_rung_run(host: &RuntimeHost, instance_alias: &str) {
    wait_until("a rung run on the ladder's key", || {
        let (holder, queued) = host
            .instance_claims_for_test(instance_alias)
            .expect("instance claims");
        holder == Some(ClaimKind::Ladder) && queued.contains(&ClaimKind::LadderContinuation)
    });
}

struct LadderRun {
    root: TempDir,
    config: RuntimeHostConfig,
    instance_id: InstanceId,
    host: RuntimeHost,
    state: Arc<FakeState>,
    context: Box<PolicyRunContext>,
    request: ContainedTaskRequest,
}

/// A physical policy run that binds a return-home package, on an instance whose startup package
/// is configured, admitted and then left with captures that stay unknown: the run fails with a
/// stuck-recovery trigger, and so does every rung run, each `capture_delay_ms` per capture. The
/// ladder runs rung 1 and rung 2 on its key; rung 3 is skipped (no emulator control; see
/// `emulator_ladder_fixture`).
fn ladder_fixture(capture_delay_ms: u64) -> LadderRun {
    ladder_fixture_with(capture_delay_ms, |_, _, _| {})
}

/// `ladder_fixture` with `prepare` applied to the started host before the run's admission.
fn ladder_fixture_with(
    capture_delay_ms: u64,
    prepare: impl FnOnce(&RuntimeHost, InstanceId, &FakeState),
) -> LadderRun {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let registered = instance_id();
    let host = RuntimeHost::start(
        host_config.clone(),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::clone(&state),
        )),
    )
    .expect("physical runtime host");
    prepare(&host, registered, state.as_ref());
    let context = admit_ladder_run(&host, &request);
    state.unknown_capture.store(true, Ordering::Release);
    state
        .capture_delay_ms
        .store(capture_delay_ms, Ordering::Release);
    LadderRun {
        root,
        config: host_config,
        instance_id: registered,
        host,
        state,
        context,
        request,
    }
}

/// The ladder fixture's packages under `root`, its host configuration (the test `config`, with
/// the scheduled package as the procedure's primary and as the instance's startup package) and
/// the scheduled run's request, which binds the return-home package.
fn ladder_setup(root: &Path) -> (RuntimeHostConfig, ContainedTaskRequest) {
    let package = neutral_contained_task_package(true);
    let package_path = root.join("physical-scheduled-task.zip");
    fs::write(&package_path, &package).expect("write scheduled package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let return_home = neutral_non_home_start_contained_task_package(true);
    let return_home_path = root.join("return-home.zip");
    fs::write(&return_home_path, &return_home).expect("write return-home package");
    let return_home_sha256 = format!("{:x}", Sha256::digest(&return_home));
    let startup = ContainedTaskRequest::new(
        package_path.to_string_lossy().into_owned(),
        package_sha256.clone(),
    )
    .expect("startup package");
    let host_config = RuntimeHostConfig::new(root, b"runtime-host-test-salt")
        .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
        .with_procedure_manifest(procedure_manifest_with_primary(
            &package,
            vec!["after_observation".to_owned()],
        ))
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
        .with_startup_packages(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            startup,
        )]));
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .and_then(|request| {
                request.with_recovery(ContainedTaskRecoveryBinding::new(
                    return_home_path.to_string_lossy().into_owned(),
                    return_home_sha256,
                )?)
            })
            .expect("scheduled package request");
    (host_config, request)
}

/// Activates the policy catalog and admits the scheduled run.
fn admit_ladder_run(host: &RuntimeHost, request: &ContainedTaskRequest) -> Box<PolicyRunContext> {
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(host, PolicyTrigger::FactsChanged);
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(&intent, &reasons, &policy_context(host, &intent), request)
        .expect("policy admission")
    else {
        panic!("expected a policy run context")
    };
    context
}

impl LadderRun {
    /// Runs the scheduled task; its failure hands the key to the ladder.
    fn fail_scheduled_run(&self) {
        let error = self
            .host
            .run_scheduled_contained_task(&self.context, &self.request)
            .expect_err("the scheduled run fails");
        assert!(
            !error.is_fatal() && actingcommand_contract::is_stuck_recovery_trigger(error.code()),
            "a stuck-recovery trigger: {}",
            error.code()
        );
    }
}

/// Workflow #369 §6, H-1, H-3, E6 (and C12): the incident replay. The failed run's lease end
/// hands the key to its ladder (the claim's own records first, then the transfer and the run's
/// release); the rungs run on the ladder's key with no lease of their own, each release handing
/// the key back to the ladder's continuation; nothing else is granted the instance until the
/// ladder's own release; no rung meets `lease_busy`, and no ladder is suppressed.
#[test]
fn a_failed_policy_run_hands_its_key_to_its_ladder_which_holds_it_through_every_rung() {
    let run = ladder_fixture(0);
    run.fail_scheduled_run();
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&run.host);
    let run_id = run.context.run_id();
    let correlation_id = run.context.correlation_id();

    let run_releases = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().run_id() == Some(&run_id)
        })
        .collect::<Vec<_>>();
    let [run_release] = run_releases.as_slice() else {
        panic!(
            "one run-linked release of the failed run: {}",
            run_releases.len()
        );
    };
    let hand_offs = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseTransferred
                && event.links().correlation_id() == Some(&correlation_id)
                && event.links().run_id().is_none()
                && event.sequence() < run_release.sequence()
        })
        .collect::<Vec<_>>();
    let [hand_off] = hand_offs.as_slice() else {
        panic!(
            "one hand-off inside the run's lease end: {}",
            hand_offs.len()
        );
    };
    let ladder_request = *hand_off.links().request_id().expect("ladder claim request");
    let EventPayload::Lease(LeasePayload::Transferred(transfer)) = hand_off.payload() else {
        panic!("hand-off payload");
    };
    assert_eq!(transfer.queued_request_id(), ladder_request);
    assert_eq!(transfer.priority(), LeasePriority::High);
    assert_eq!(
        transfer.from_lease_id(),
        run.context.lease_token().lease_id()
    );
    // Review L-2: the ladder claim's own records, then the receiver's intent, before the transfer.
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.links().request_id() == Some(&ladder_request)
                    && event.sequence() < hand_off.sequence()
            })
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
        ]
    );
    let queued = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::SchedulerQueued
                && event.links().request_id() == Some(&ladder_request)
        })
        .expect("the ladder claim's scheduler.queued");
    let EventPayload::Scheduler(SchedulerPayload::Queued(queued)) = queued.payload() else {
        panic!("queued payload");
    };
    assert_eq!(queued.deadline_monotonic_ms(), u64::MAX);
    assert_eq!(queued.priority(), LeasePriority::High);
    assert!(!queued.preempt_requested());
    let task_failed = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::TaskFailed && event.links().run_id() == Some(&run_id)
        })
        .expect("the run's task.failed");
    assert!(task_failed.sequence() < hand_off.sequence());

    // The ladder's key: the hand-off's lease, then every continuation's.
    let mut chain = vec![transfer.to_lease_id()];
    let mut continuations = Vec::new();
    for event in &events {
        if let EventPayload::Lease(LeasePayload::Transferred(next)) = event.payload()
            && chain.contains(&next.from_lease_id())
        {
            chain.push(next.to_lease_id());
            continuations.push(next.queued_request_id());
        }
    }
    assert_eq!(continuations.len(), 2, "one continuation per rung run");
    let rung_runs = events
        .iter()
        .filter(|event| {
            event
                .links()
                .lease_id()
                .is_some_and(|lease| chain.contains(lease))
        })
        .filter_map(|event| event.links().run_id().copied())
        .filter(|rung_run| *rung_run != run_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(rung_runs.len(), 2, "rung 1 and rung 2 run on the key");
    for rung_run in &rung_runs {
        let releases = events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseReleased
                    && event.links().run_id() == Some(rung_run)
            })
            .collect::<Vec<_>>();
        let [release] = releases.as_slice() else {
            panic!("one run-linked release per rung run: {}", releases.len());
        };
        assert!(chain.contains(release.links().lease_id().expect("rung release lease")));
        assert_eq!(
            release.payload().effect_disposition(),
            Some(EffectDisposition::Performed)
        );
    }
    // C12: only the policy dispatch was ever granted a lease; the ladder claim and the two
    // continuations were requested and handed the key, and no rung requested one.
    assert_eq!(count(&events, EventType::LeaseGranted), 1);
    assert_eq!(count(&events, EventType::LeaseRequested), 4);
    // E6: no run-less artifact is written under the ladder's key.
    assert!(events.iter().all(|event| {
        event.artifacts().is_empty()
            || !event
                .links()
                .lease_id()
                .is_some_and(|lease| chain.contains(lease))
            || event.links().run_id().is_some()
    }));

    let (finished, outcome, rungs_tried) = ladder_finished(&events).expect("ladder finished");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(rungs_tried, 2);
    assert_eq!(finished.severity(), EventSeverity::Error);
    assert_eq!(
        rung_outcomes(&events)
            .into_iter()
            .map(|(rung, outcome, _)| (rung, outcome))
            .collect::<Vec<_>>(),
        vec![
            (RecoveryRung::ReturnHome, RecoveryRungOutcome::Failed),
            (
                RecoveryRung::ApplicationRestart,
                RecoveryRungOutcome::Failed
            ),
            (RecoveryRung::EmulatorRestart, RecoveryRungOutcome::Skipped),
        ]
    );
    let final_releases = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().request_id() == Some(&ladder_request)
        })
        .collect::<Vec<_>>();
    let [final_release] = final_releases.as_slice() else {
        panic!("the ladder releases once: {}", final_releases.len());
    };
    assert!(final_release.links().run_id().is_none());
    assert!(final_release.sequence() > finished.sequence());
    assert!(
        chain.contains(
            final_release
                .links()
                .lease_id()
                .expect("ladder release lease")
        )
    );
    // M-1: a failed rung run's end is an ordinary one: no ladder is suppressed or nested.
    assert!(phases(&events).iter().all(|(_, phase)| !matches!(
        phase,
        RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
    )));
    assert_eq!(
        phases(&events)
            .iter()
            .filter(|(_, phase)| matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
            ))
            .count(),
        1
    );
    // No rung met `lease_busy`, and rung 2's stop ran on the key.
    assert_eq!(count(&events, EventType::SchedulerDenied), 0);
    assert!(events.iter().all(|event| {
        !failure_message(event).is_some_and(|message| message.contains("lease_busy"))
    }));
    assert_eq!(run.state.application_count.load(Ordering::Acquire), 1);

    assert!(run.host.fatal_error().expect("runtime health").is_none());
    let mut client = TestClient::connect(&run.host);
    let (_, token) = client.acquire(POLICY_INSTANCE_ALIAS);
    let release = client.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    run.host.close().expect("close host");
    drop(run.root);
}

/// Workflow #369 H-3 (ruling Q8), §5.1: a pause during a rung run drains the run; its release
/// hands the key back to the ladder's continuation (the gate never holds it back), the ladder
/// sees the pause, fails its next rung with `recovery_admission_denied`, finishes exhausted at
/// Warning and releases once, and the pause completes. (A pause during the R4 readiness wait:
/// `a_pause_during_the_readiness_wait_ends_the_climb_at_warning_with_one_release`.)
#[test]
fn a_pause_during_a_rung_run_ends_the_climb_at_warning_and_completes() {
    let run = ladder_fixture(400);
    run.fail_scheduled_run();
    wait_for_rung_run(&run.host, POLICY_INSTANCE_ALIAS);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let pause = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
            },
            reason_code: "s2_pause_during_climb".to_owned(),
            drain_timeout_ms: 1_000,
        },
    )
    .expect("pause request");
    let receipt = run
        .host
        .process_request_for_test(&pause, ConnectionId::new(71).expect("connection"))
        .expect("pause receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&run.host);
    let (finished, outcome, _) = ladder_finished(&events).expect("ladder finished");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(finished.severity(), EventSeverity::Warning);
    let rungs = rung_outcomes(&events);
    assert!(
        rungs.iter().any(
            |(_, outcome, reason)| *outcome == RecoveryRungOutcome::Failed
                && reason.as_deref() == Some("recovery_admission_denied")
        ),
        "{rungs:?}"
    );
    assert!(phases(&events).iter().all(|(_, phase)| !matches!(
        phase,
        RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
    )));
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

/// Review C-2 (#666): the ladder fixture on an instance with emulator control and a discovery
/// binding, so rung 3 runs its Stop and Start on the ladder's key and then the readiness wait
/// (R4). With `hold_android_boot`, Android reports no foreground activity after the Start, so
/// R4 keeps polling.
fn emulator_ladder_fixture(capture_delay_ms: u64, hold_android_boot: bool) -> LadderRun {
    ladder_fixture_with(capture_delay_ms, |host, registered, state| {
        state
            .hold_android_boot
            .store(hold_android_boot, Ordering::Release);
        bind_fake_emulator(host, POLICY_INSTANCE_ALIAS, registered, state);
    })
}

fn emulator_actions(state: &FakeState) -> Vec<actingcommand_contract::EmulatorInstanceAction> {
    state
        .emulator_actions
        .lock()
        .expect("fake emulator actions lock")
        .clone()
}

/// Rung 3's Stop and Start ran on the fake; the ladder is in its readiness wait or later.
fn wait_for_rung_three_start(state: &FakeState) {
    use actingcommand_contract::EmulatorInstanceAction::{Start, Stop};
    wait_until("rung 3's Stop and Start", || {
        emulator_actions(state) == [Stop, Start]
    });
}

/// An instance pause with a 1 s drain, as the operator's `actingctl pause` sends it.
fn pause_instance(host: &RuntimeHost, instance_alias: &str, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let pause = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: instance_alias.to_owned(),
            },
            reason_code: "s2_pause_on_the_ladder".to_owned(),
            drain_timeout_ms: 1_000,
        },
    )
    .expect("pause request");
    host.process_request_for_test(&pause, ConnectionId::new(connection).expect("connection"))
        .expect("pause receipt")
}

/// The ladder claim's own release records (request links only), found through the hand-off
/// that took the failed run's lease.
fn ladder_releases(events: &[PersistedEvent], run_lease: LeaseId) -> Vec<&PersistedEvent> {
    let ladder_request = events
        .iter()
        .find_map(|event| match event.payload() {
            EventPayload::Lease(LeasePayload::Transferred(transfer))
                if transfer.from_lease_id() == run_lease =>
            {
                Some(transfer.queued_request_id())
            }
            _ => None,
        })
        .expect("the hand-off to the ladder");
    events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().request_id() == Some(&ladder_request)
        })
        .collect()
}

/// §7 row 4, H-3, review2 L7 (review C-2): a pause during rung 3's readiness wait (R4). The
/// Stop and Start ran on the ladder's key; while Android has not booted the wait polls, sees
/// the pause, fails rung 3 with `recovery_admission_denied` and ends the climb exhausted at
/// Warning; the ladder releases its key once, and the pause completes within its grace.
#[test]
fn a_pause_during_the_readiness_wait_ends_the_climb_at_warning_with_one_release() {
    let run = emulator_ladder_fixture(0, true);
    run.fail_scheduled_run();
    wait_for_rung_three_start(&run.state);
    let receipt = pause_instance(&run.host, POLICY_INSTANCE_ALIAS, 74);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the pause completes within its grace: {receipt:?}"
    );
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&run.host);
    let (finished, outcome, _) = ladder_finished(&events).expect("ladder finished");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(finished.severity(), EventSeverity::Warning);
    let rungs = rung_outcomes(&events);
    assert_eq!(
        rungs.last(),
        Some(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Failed,
            Some("recovery_admission_denied".to_owned())
        )),
        "{rungs:?}"
    );
    let releases = ladder_releases(&events, run.context.lease_token().lease_id());
    assert_eq!(releases.len(), 1, "the ladder releases its key once");
    assert!(releases[0].sequence() > finished.sequence());
    {
        use actingcommand_contract::EmulatorInstanceAction::{Start, Stop};
        assert_eq!(emulator_actions(&run.state), [Stop, Start]);
    }
    let (_, paused) = run.host.scheduling_pauses_for_test().expect("pauses");
    assert!(
        paused.contains_key(POLICY_INSTANCE_ALIAS),
        "the pause stays"
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

/// §7 row 4, W-2 (review C-2): instance A's worker blocked in rung 3's readiness wait (R4)
/// holds only A. Instance B's startup claim is granted meanwhile and its startup run completes
/// on B's own worker, while A's ladder still holds A and is still waiting.
#[test]
fn a_ladder_blocked_in_the_readiness_wait_does_not_hold_up_another_instances_startup_claim() {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let package = fs::read(root.path().join("physical-scheduled-task.zip")).expect("package");
    let startup = ContainedTaskRequest::new(
        root.path()
            .join("physical-scheduled-task.zip")
            .to_string_lossy()
            .into_owned(),
        format!("{:x}", Sha256::digest(&package)),
    )
    .expect("startup package");
    let ladder_state = Arc::new(FakeState::default());
    ladder_state
        .physical_task_geometry
        .store(true, Ordering::Release);
    let startup_state = Arc::new(FakeState::default());
    startup_state
        .physical_task_geometry
        .store(true, Ordering::Release);
    startup_state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let ladder_instance = instance_id();
    let startup_instance = instance_id();
    // The policy's configured identities are the registered aliases (both instances); only A
    // is available to the policy.
    let mut facts = policy_facts();
    let mut startup_identity = facts.instances[0].clone();
    startup_identity.instance_id = STARTUP_ALIAS.to_owned();
    startup_identity.available = false;
    facts.instances.push(startup_identity);
    let host = RuntimeHost::start(
        host_config
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources()))
            .with_startup_packages(BTreeMap::from([
                (POLICY_INSTANCE_ALIAS.to_owned(), startup.clone()),
                (STARTUP_ALIAS.to_owned(), startup),
            ])),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                ladder_instance,
                Arc::clone(&ladder_state),
            ),
            (
                STARTUP_ALIAS.to_owned(),
                startup_instance,
                Arc::clone(&startup_state),
            ),
        ])),
    )
    .expect("runtime host with two instances");
    ladder_state
        .hold_android_boot
        .store(true, Ordering::Release);
    bind_fake_emulator(&host, POLICY_INSTANCE_ALIAS, ladder_instance, &ladder_state);
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
        .expect("A's dispatch intent")
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
    ladder_state.unknown_capture.store(true, Ordering::Release);
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(actingcommand_contract::is_stuck_recovery_trigger(
        error.code()
    ));
    wait_for_rung_three_start(&ladder_state);
    assert_eq!(
        host.schedule_startup_package_for_test(STARTUP_ALIAS)
            .expect("schedule B's startup package"),
        StartupPackageDisposition::Scheduled
    );
    wait_until("B's startup run", || {
        host.instance_claims_for_test(STARTUP_ALIAS)
            .expect("B's claims")
            .0
            .is_none()
            && all_events(&host).iter().any(|event| {
                event.event_type() == EventType::TaskCompleted
                    && event.links().instance_id() == Some(&startup_instance)
            })
    });
    assert_eq!(
        host.instance_claims_for_test(POLICY_INSTANCE_ALIAS)
            .expect("A's claims")
            .0,
        Some(ClaimKind::Ladder),
        "A's ladder still holds A while B's startup run completed"
    );
    assert!(
        ladder_finished(&all_events(&host)).is_none(),
        "A's ladder is still waiting for readiness"
    );
    {
        use actingcommand_contract::EmulatorInstanceAction::{Start, Stop};
        assert_eq!(emulator_actions(&ladder_state), [Stop, Start]);
    }
    assert!(startup_state.capture_count.load(Ordering::Acquire) > 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Review L6 (#666), §5.10: when every rung ran and failed on its own, a holding interruption at
/// the climb's end does not soften the outcome. Rung 3's Start ends with the vendor's readiness
/// wait timing out while a pause has just closed the instance's gate: the climb is exhausted at
/// Error, the ladder releases its key once, and the pause completes.
#[test]
fn a_climb_whose_rungs_all_ran_and_failed_stays_at_error_under_a_pause_at_its_end() {
    let run = emulator_ladder_fixture(0, false);
    run.state
        .block_emulator_start
        .store(true, Ordering::Release);
    run.state.fail_emulator_start.store(true, Ordering::Release);
    run.fail_scheduled_run();
    wait_until("rung 3's Start", || {
        run.state.emulator_start_entered.load(Ordering::Acquire)
    });
    let receipt = thread::scope(|scope| {
        let host = &run.host;
        let pause = scope.spawn(move || pause_instance(host, POLICY_INSTANCE_ALIAS, 75));
        wait_until("the pause's gate", || {
            host.scheduling_pauses_for_test()
                .expect("pauses")
                .1
                .contains_key(POLICY_INSTANCE_ALIAS)
        });
        run.state
            .block_emulator_start
            .store(false, Ordering::Release);
        pause.join().expect("the pause's thread")
    });
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&run.host);
    let (finished, outcome, _) = ladder_finished(&events).expect("ladder finished");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(finished.severity(), EventSeverity::Error);
    let rungs = rung_outcomes(&events);
    assert_eq!(
        rungs.last(),
        Some(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Failed,
            Some("emulator_control_wait_timeout".to_owned())
        )),
        "rung 3 ran and failed on its own: {rungs:?}"
    );
    assert_eq!(
        ladder_releases(&events, run.context.lease_token().lease_id()).len(),
        1
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

/// Review L6 (verify-666): a pause that cancels rung 3's own package run ends the climb at
/// Warning, as a pause during rung 1's or rung 2's run does: the run's `contained_task_paused`
/// is a holding interruption, not the rung's own failure. Rung 3's readiness passes here (the
/// fake's opens report a passed self-check once Android has booted), so rung 3 runs the startup
/// package on the key; the pause's drain cancels that run.
#[test]
fn a_pause_that_cancels_rung_three_s_package_run_ends_the_climb_at_warning() {
    let run = emulator_ladder_fixture(0, true);
    run.fail_scheduled_run();
    wait_for_rung_three_start(&run.state);
    // R4 polls an Android that has not booted. Its next attempt sees Android up, passes its
    // preparation, and rung 3 runs the startup package, each capture taking 3 s.
    run.state.observed_open.store(true, Ordering::Release);
    run.state.capture_delay_ms.store(3_000, Ordering::Release);
    run.state.android_down.store(false, Ordering::Release);
    wait_for_rung_run(&run.host, POLICY_INSTANCE_ALIAS);
    let receipt = pause_instance(&run.host, POLICY_INSTANCE_ALIAS, 76);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&run.host);
    assert!(
        phases(&events).iter().any(|(_, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryEnvironmentReady { .. }
        )),
        "rung 3's readiness passed"
    );
    let (finished, outcome, _) = ladder_finished(&events).expect("ladder finished");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(finished.severity(), EventSeverity::Warning);
    let rungs = rung_outcomes(&events);
    assert_eq!(
        rungs.last(),
        Some(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Failed,
            Some("contained_task_paused".to_owned())
        )),
        "{rungs:?}"
    );
    assert_eq!(
        ladder_releases(&events, run.context.lease_token().lease_id()).len(),
        1
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

/// Workflow #369 §5.2: an install drain during a climb cancels nothing the ladder holds; the
/// ladder ends at its next holding check (`recovery_ladder_drain_requested`, exhausted at
/// Warning), releases once, and the install drain completes.
#[test]
fn an_install_drain_during_a_climb_ends_the_ladder_and_drains() {
    let run = ladder_fixture(400);
    run.fail_scheduled_run();
    wait_for_rung_run(&run.host, POLICY_INSTANCE_ALIAS);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let connection = ConnectionId::new(72).expect("connection");
    let governance = |operation| {
        RuntimeRequest::new(
            ids.mint_request_id().expect("request id"),
            ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::User,
            EventSource::Ui,
            unix_ms_now().expect("wall clock"),
            operation,
        )
        .expect("governance request")
    };
    let declared = run
        .host
        .process_request_for_test(
            &governance(RuntimeOperation::DeclareGovernanceIdentity {
                card: test_governance_card(),
            }),
            connection,
        )
        .expect("declaration");
    assert_eq!(declared.state(), RuntimeReceiptState::Completed);
    let target = run.host.runtime_info().shutdown_target();
    let begin = run
        .host
        .process_request_for_test(
            &governance(RuntimeOperation::InstallTransition {
                target,
                action: InstallTransitionAction::BeginDrain {
                    transition_id: "s2-drain-during-climb".to_owned(),
                    timeout_ms: 120_000,
                },
            }),
            connection,
        )
        .expect("begin drain");
    assert!(
        !matches!(
            begin.state(),
            RuntimeReceiptState::Denied | RuntimeReceiptState::Failed
        ),
        "{begin:?}"
    );
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    wait_until("the install drain", || {
        let status = run
            .host
            .process_request_for_test(
                &governance(RuntimeOperation::InstallTransition {
                    target,
                    action: InstallTransitionAction::Query {
                        transition_id: "s2-drain-during-climb".to_owned(),
                    },
                }),
                connection,
            )
            .expect("install status");
        matches!(
            status.result(),
            Some(RuntimeResult::InstallTransition { status })
                if status.phase == InstallTransitionPhase::Drained
        )
    });
    let events = all_events(&run.host);
    let (finished, outcome, _) = ladder_finished(&events).expect("ladder finished");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(finished.severity(), EventSeverity::Warning);
    let rungs = rung_outcomes(&events);
    assert!(
        rungs
            .iter()
            .any(|(_, _, reason)| reason.as_deref() == Some("recovery_ladder_drain_requested")),
        "{rungs:?}"
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

/// Workflow #369 W-2, H-3 (review L3): a shutdown during a climb cancels the queues
/// (`lease.queue_disconnected`), the worker ends the ladder with
/// `recovery_ladder_shutdown_requested` (exhausted at Warning), the host closes without a
/// fatal error and the next start finds the instance free.
#[test]
fn a_shutdown_during_a_climb_ends_the_ladder_without_a_leak_or_a_fatal_error() {
    let run = ladder_fixture(400);
    run.fail_scheduled_run();
    wait_for_rung_run(&run.host, POLICY_INSTANCE_ALIAS);
    let LadderRun {
        root,
        config: host_config,
        instance_id: registered,
        host,
        state,
        ..
    } = run;
    host.close().expect("close host during the climb");
    let events = closed_ledger_events(root.path());
    let (finished, outcome, _) = ladder_finished(&events).expect("ladder finished");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(finished.severity(), EventSeverity::Warning);
    let rungs = rung_outcomes(&events);
    assert!(
        rungs.iter().any(|(_, _, reason)| {
            reason.as_deref() == Some("recovery_ladder_shutdown_requested")
        }),
        "{rungs:?}"
    );
    assert!(
        events
            .iter()
            .all(|event| event.severity() != EventSeverity::Fatal),
        "no fatal record"
    );
    state.unknown_capture.store(false, Ordering::Release);
    state.capture_delay_ms.store(0, Ordering::Release);
    let reopened = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, state)),
    )
    .expect("restart after the climb");
    let (holder, queued) = reopened
        .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
        .expect("claims after restart");
    assert!(holder.is_none() && queued.is_empty(), "nothing leaked");
    assert!(reopened.fatal_error().expect("runtime health").is_none());
    reopened.close().expect("close reopened host");
}

fn startup_host(
    root: &TempDir,
    aliases: &[(&str, Arc<FakeState>)],
    deadline_ms: u64,
) -> RuntimeHost {
    startup_host_with(root, aliases, deadline_ms, |config| config)
}

/// `startup_host` with `configure` applied to its configuration.
fn startup_host_with(
    root: &TempDir,
    aliases: &[(&str, Arc<FakeState>)],
    deadline_ms: u64,
    configure: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> RuntimeHost {
    let package = neutral_contained_task_package(true);
    let package_path = root.path().join("startup-package.zip");
    fs::write(&package_path, &package).expect("write startup package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let startup =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .and_then(|request| request.with_response_deadline_ms(deadline_ms))
            .expect("startup package request");
    let mut startup_packages = BTreeMap::new();
    let mut entries = Vec::new();
    for (alias, state) in aliases {
        state.physical_task_geometry.store(true, Ordering::Release);
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        startup_packages.insert((*alias).to_owned(), startup.clone());
        entries.push(((*alias).to_owned(), instance_id(), Arc::clone(state)));
    }
    RuntimeHost::start(
        configure(config(root).with_startup_packages(startup_packages)),
        Arc::new(FakeProvider::from_entries(entries)),
    )
    .expect("runtime host with startup packages")
}

/// The run ids of every completed contained run.
fn completed_runs(events: &[PersistedEvent]) -> Vec<RunId> {
    events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskCompleted)
        .filter_map(|event| event.links().run_id().copied())
        .collect()
}

/// Workflow #369 H-6: an emulator start's startup claim is queued under the control's admission
/// guard and granted by the pump when the guard is let go, so no dispatch can see the instance
/// free in between; the startup run runs on the claim's key, with no lease of its own, and its
/// release ends the hold.
#[test]
fn a_start_queues_its_startup_claim_under_the_guard_and_runs_it_on_the_claims_key() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = startup_host(&root, &[(STARTUP_ALIAS, Arc::clone(&state))], 60_000);
    assert_eq!(
        host.schedule_startup_package_for_test(STARTUP_ALIAS)
            .expect("schedule startup package"),
        StartupPackageDisposition::Scheduled
    );
    wait_until("the startup run", || {
        completed_runs(&all_events(&host)).len() == 1
            && host
                .instance_claims_for_test(STARTUP_ALIAS)
                .expect("claims")
                .0
                .is_none()
    });
    let events = all_events(&host);
    let scheduled = phases(&events)
        .into_iter()
        .find(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. }))
        .map(|(event, _)| event)
        .expect("startup_package_scheduled");
    let causation = *scheduled
        .links()
        .causation_id()
        .expect("scheduling causation");
    let claim_request = *events
        .iter()
        .find(|event| {
            event.event_type() == EventType::LeaseRequested
                && event.links().causation_id() == Some(&causation)
        })
        .and_then(|event| event.links().request_id())
        .expect("the startup claim's request");
    let claim = events
        .iter()
        .filter(|event| event.links().request_id() == Some(&claim_request))
        .collect::<Vec<_>>();
    assert_eq!(
        claim
            .iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
        ]
    );
    assert!(
        claim
            .iter()
            .all(|event| event.sequence() > scheduled.sequence())
    );
    let EventPayload::Scheduler(SchedulerPayload::Queued(queued)) = claim[1].payload() else {
        panic!("queued payload");
    };
    assert_eq!(queued.priority(), LeasePriority::High);
    assert_eq!(queued.deadline_monotonic_ms(), u64::MAX);
    // Nothing else was granted the instance, and the run took no lease of its own.
    assert_eq!(count(&events, EventType::LeaseGranted), 1);
    assert_eq!(count(&events, EventType::LeaseRequested), 1);
    let [startup_run] = completed_runs(&events)[..] else {
        panic!("one startup run");
    };
    let releases = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().run_id() == Some(&startup_run)
        })
        .count();
    assert_eq!(releases, 1);
    assert_eq!(
        count(&events, EventType::LeaseReleased),
        1,
        "the run's release ends the claim's hold"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Workflow #369 H-6: until S6b the startup claim is not gated, so a start under a scheduling
/// pause still runs its startup package (`scheduling-pause.md`, "not gated"); the pause stays.
#[test]
fn a_start_under_a_pause_still_runs_its_startup_package() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = startup_host(&root, &[(STARTUP_ALIAS, Arc::clone(&state))], 60_000);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let pause = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: STARTUP_ALIAS.to_owned(),
            },
            reason_code: "s2_start_under_pause".to_owned(),
            drain_timeout_ms: 1_000,
        },
    )
    .expect("pause request");
    assert_eq!(
        host.process_request_for_test(&pause, ConnectionId::new(73).expect("connection"))
            .expect("pause receipt")
            .state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(
        host.schedule_startup_package_for_test(STARTUP_ALIAS)
            .expect("schedule startup package"),
        StartupPackageDisposition::Scheduled
    );
    wait_until("the startup run under the pause", || {
        completed_runs(&all_events(&host)).len() == 1
            && host
                .instance_claims_for_test(STARTUP_ALIAS)
                .expect("claims")
                .0
                .is_none()
    });
    let (_, paused) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(paused.contains_key(STARTUP_ALIAS), "the pause stays");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Workflow #369 H-1 (review M-1): a failed startup run is an ordinary lease end: it starts no
/// ladder and suppresses none.
#[test]
fn a_failed_startup_run_starts_no_ladder() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = startup_host(&root, &[(STARTUP_ALIAS, Arc::clone(&state))], 60_000);
    state.unknown_capture.store(true, Ordering::Release);
    host.schedule_startup_package_for_test(STARTUP_ALIAS)
        .expect("schedule startup package");
    wait_until("the failed startup run", || {
        count(&all_events(&host), EventType::TaskFailed) == 1
            && host
                .instance_claims_for_test(STARTUP_ALIAS)
                .expect("claims")
                .0
                .is_none()
    });
    let events = all_events(&host);
    assert!(phases(&events).iter().all(|(_, phase)| !matches!(
        phase,
        RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
            | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
    )));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Review C-1 (#666), model H-6: a startup claim never waits for business capacity. A start
/// while capacity refuses has its claim granted; the startup run's own capacity check refuses
/// the run and records the refusal, and the claim's key is released: nothing stays queued and
/// nothing holds the instance.
#[test]
fn a_start_while_capacity_refuses_records_the_refusal_and_leaves_no_queued_entry() {
    use actingcommand_contract::CapacityThresholds;
    use actingcommand_host_metrics::{
        CapacitySample, CapacityTarget, HostSample, HostSampler, ProcessLoadThresholds,
    };

    struct NoFreeBytes;

    impl HostSampler for NoFreeBytes {
        fn sample_capacity(&mut self, targets: &[CapacityTarget]) -> Vec<CapacitySample> {
            actingcommand_host_metrics::sample_capacity(targets)
                .into_iter()
                .map(|mut sample| {
                    if sample.available_bytes.is_ok() {
                        sample.available_bytes = Ok(0);
                    }
                    sample
                })
                .collect()
        }

        fn sample(
            &mut self,
            _observed_at_unix_ms: u64,
            _owned_processes: &BTreeMap<u32, String>,
            _top_process_count: usize,
            _thresholds: ProcessLoadThresholds,
        ) -> Result<HostSample, &'static str> {
            panic!("capacity-only specification does not enable performance counters")
        }
    }

    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = startup_host_with(
        &root,
        &[(STARTUP_ALIAS, Arc::clone(&state))],
        60_000,
        |config| config.with_capacity_thresholds(CapacityThresholds::default()),
    );
    host.replace_capacity_sampler_for_test(Box::new(NoFreeBytes))
        .expect("hard capacity pressure");
    assert_eq!(
        host.schedule_startup_package_for_test(STARTUP_ALIAS)
            .expect("schedule startup package"),
        StartupPackageDisposition::Scheduled
    );
    wait_until("the refused startup run and its key's release", || {
        let events = all_events(&host);
        let claims = host
            .instance_claims_for_test(STARTUP_ALIAS)
            .expect("claims");
        count(&events, EventType::LeaseReleased) == 1 && claims.0.is_none() && claims.1.is_empty()
    });
    let events = all_events(&host);
    // The claim was granted, not held back for capacity, and its key released once.
    assert_eq!(count(&events, EventType::LeaseGranted), 1);
    assert_eq!(count(&events, EventType::LeaseReleased), 1);
    // The run's own capacity check refused it and recorded the refusal; no task ran.
    assert!(
        events.iter().any(|event| {
            event.event_type() == EventType::SchedulerDenied
                && event.severity() == EventSeverity::Warning
                && event.payload().action() == actingcommand_contract::EventAction::ScheduleAdmit
        }),
        "the capacity refusal is recorded"
    );
    assert_eq!(count(&events, EventType::TaskCompleted), 0);
    assert_eq!(count(&events, EventType::TaskFailed), 0);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Workflow #369 W-2 (review L-10): host work on one instance no longer waits for another's.
/// While instance A's worker is inside a long startup run, instance B's startup claim is granted
/// and completes on B's own worker.
#[test]
fn one_instances_worker_does_not_hold_up_another_instances_startup_claim() {
    let root = TempDir::new().expect("tempdir");
    let slow = Arc::new(FakeState::default());
    let quick = Arc::new(FakeState::default());
    let host = startup_host(
        &root,
        &[
            ("instance.a", Arc::clone(&slow)),
            ("instance.b", Arc::clone(&quick)),
        ],
        60_000,
    );
    slow.capture_delay_ms.store(3_000, Ordering::Release);
    host.schedule_startup_package_for_test("instance.a")
        .expect("schedule A");
    wait_until("A's startup run", || {
        slow.capture_count.load(Ordering::Acquire) > 0
            || host
                .instance_claims_for_test("instance.a")
                .expect("claims")
                .0
                == Some(ClaimKind::StartupPackage)
    });
    host.schedule_startup_package_for_test("instance.b")
        .expect("schedule B");
    wait_until("B's startup run", || {
        host.instance_claims_for_test("instance.b")
            .expect("claims")
            .0
            .is_none()
            && completed_runs(&all_events(&host)).len() == 1
    });
    assert_eq!(
        host.instance_claims_for_test("instance.a")
            .expect("claims")
            .0,
        Some(ClaimKind::StartupPackage),
        "A's worker still holds A while B's startup run completed"
    );
    assert!(quick.capture_count.load(Ordering::Acquire) > 0);
    wait_until("A's startup run", || {
        host.instance_claims_for_test("instance.a")
            .expect("claims")
            .0
            .is_none()
    });
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Workflow #369 W-2 (spec §7; review L-8): a worker panic is caught at the thread root and
/// recorded as `panic_caught` (warning) under the payload's failure category; the payload is
/// dropped and its key lapses at its TTL; the rebuilt worker runs the next payload.
#[test]
fn a_worker_panic_is_recorded_and_the_rebuilt_worker_runs_the_next_payload() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = startup_host(&root, &[(STARTUP_ALIAS, Arc::clone(&state))], 2_000);
    host.inject_instance_worker_panic_for_test()
        .expect("inject worker panic");
    host.schedule_startup_package_for_test(STARTUP_ALIAS)
        .expect("schedule the panicking payload");
    wait_until("panic_caught", || {
        all_events(&host).iter().any(|event| {
            event.severity() == EventSeverity::Warning
                && failure_message(event)
                    .is_some_and(|message| message.contains("code=panic_caught"))
        })
    });
    // The second claim waits behind the panicked payload's key until that key lapses.
    host.schedule_startup_package_for_test(STARTUP_ALIAS)
        .expect("schedule the next payload");
    wait_until("the next payload's run", || {
        completed_runs(&all_events(&host)).len() == 1
            && host
                .instance_claims_for_test(STARTUP_ALIAS)
                .expect("claims")
                .0
                .is_none()
    });
    let events = all_events(&host);
    let text = events
        .iter()
        .filter_map(failure_message)
        .find(|message| message.contains("code=panic_caught"))
        .expect("panic record");
    assert!(
        events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.detail().is_some_and(|detail| detail.category() == "startup_package"
                    && detail.message().contains("code=panic_caught"))
        )),
        "{text}"
    );
    assert!(text.contains("boundary=instance_worker"), "{text}");
    assert!(
        text.contains("raw_text=injected instance worker panic"),
        "{text}"
    );
    assert!(
        count(&events, EventType::LeaseExpired) + count(&events, EventType::LeaseTransferred) >= 1,
        "the panicked payload's key lapsed and was handed on"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// The events of a closed or crashed host's ledger, its artifacts verified.
fn closed_ledger_events(root: &Path) -> Vec<PersistedEvent> {
    let artifacts = ArtifactStore::open(root).expect("open the artifact store");
    GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root),
        |reference| artifacts.verify_recovery_reference(reference).ok(),
    )
    .expect("read the closed ledger")
    .query(&EventQuery::default())
}

/// Workflow #369 E3: the child process of the crash points around a hand-off. It starts the
/// ladder fixture's host on `ACTINGCOMMAND_LADDER_CRASH_ROOT`, admits the scheduled run, notes
/// its identity, arms the crash point and runs it; the crash point ends the process.
#[test]
fn ladder_hand_off_crash_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_LADDER_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let registered: InstanceId =
        serde_json::from_slice(&fs::read(root.join("instance.json")).expect("instance bytes"))
            .expect("instance identifier");
    let (host_config, request) = ladder_setup(&root);
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::clone(&state),
        )),
    )
    .expect("child runtime host");
    // A later crash on the same root admits at its own evaluation time; the catalog is
    // already active there.
    let context = match std::env::var("ACTINGCOMMAND_LADDER_CRASH_EVAL_MS") {
        Ok(at) => admit_ladder_run_at(&host, &request, at.parse().expect("evaluation time")),
        Err(_) => admit_ladder_run(&host, &request),
    };
    fs::write(
        root.join("crash-run.json"),
        serde_json::to_vec(&(context.run_id(), context.lease_token().lease_id()))
            .expect("run identity bytes"),
    )
    .expect("run identity file");
    if std::env::var("ACTINGCOMMAND_POLICY_CRASH_POINT").as_deref()
        == Ok("after_lease_release_before_policy_execution")
    {
        let marker = std::env::var_os("ACTINGCOMMAND_POLICY_CRASH_MARKER").expect("marker path");
        host.exit_at_scheduled_policy_checkpoint_for_test(&context, PathBuf::from(marker))
            .expect("arm the checkpoint");
    }
    if std::env::var_os("ACTINGCOMMAND_LADDER_CRASH_SUCCEED").is_some() {
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
    } else {
        state.unknown_capture.store(true, Ordering::Release);
    }
    let outcome = host.run_scheduled_contained_task(&context, &request);
    // A completed run's outcome is recorded by its caller, as the policy driver does; the
    // checkpoint ends the process there.
    if let Ok(receipt) = &outcome {
        let completed = host.complete_scheduled_policy_run(&context, receipt);
        panic!(
            "the crash point did not stop the child: {:?}",
            completed.err()
        );
    }
    if std::env::var("ACTINGCOMMAND_POLICY_CRASH_POINT").as_deref() == Ok("mid_rung_run") {
        // E6: the run failed and handed its key to the ladder. Every capture from here on
        // takes ten minutes, so the next rung run stays in flight; once it has started, the
        // marker is written and the parent kills the process.
        state.capture_delay_ms.store(600_000, Ordering::Release);
        let scheduled = context.run_id();
        loop {
            let events = all_events(&host);
            let rung_run_in_flight = events.iter().any(|event| {
                event.event_type() == EventType::TaskStarted
                    && event
                        .links()
                        .run_id()
                        .is_some_and(|run| *run != scheduled && !run_ended(&events, run))
            });
            if rung_run_in_flight {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let marker = std::env::var_os("ACTINGCOMMAND_POLICY_CRASH_MARKER").expect("marker path");
        fs::write(marker, b"mid_rung_run").expect("crash marker");
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    panic!(
        "the crash point did not stop the child: {:?}",
        outcome.err()
    );
}

/// Whether `run` has a task terminal among `events`.
fn run_ended(events: &[PersistedEvent], run: &RunId) -> bool {
    events.iter().any(|event| {
        matches!(
            event.event_type(),
            EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
        ) && event.links().run_id() == Some(run)
    })
}

/// `admit_ladder_run` on a root whose catalog is already active, evaluated at `unix_ms`.
fn admit_ladder_run_at(
    host: &RuntimeHost,
    request: &ContainedTaskRequest,
    unix_ms: u64,
) -> Box<PolicyRunContext> {
    let (_, intent, reasons) =
        evaluated_policy_dispatch_at(host, PolicyTrigger::FactsChanged, unix_ms, 8);
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(&intent, &reasons, &policy_context(host, &intent), request)
        .expect("policy admission")
    else {
        panic!("expected a policy run context")
    };
    context
}

/// Starts the crash child at `point` on `root`, with its marker path.
fn spawn_ladder_crash_child(root: &Path, point: &str) -> (std::process::Child, PathBuf) {
    spawn_ladder_crash_child_with(root, point, &[])
}

/// `spawn_ladder_crash_child` with extra environment for the child.
fn spawn_ladder_crash_child_with(
    root: &Path,
    point: &str,
    environment: &[(&str, String)],
) -> (std::process::Child, PathBuf) {
    // A fresh marker for every child: a root can see more than one crash.
    static CHILDREN: AtomicUsize = AtomicUsize::new(0);
    let marker = root.join(format!(
        "ladder-crash-marker-{}",
        CHILDREN.fetch_add(1, Ordering::Relaxed)
    ));
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "tests::instance_workers::ladder_hand_off_crash_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_LADDER_CRASH_ROOT", root)
        .env("ACTINGCOMMAND_POLICY_CRASH_POINT", point)
        .env("ACTINGCOMMAND_POLICY_CRASH_MARKER", &marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in environment {
        command.env(key, value);
    }
    (command.spawn().expect("spawn the crash child"), marker)
}

/// Waits for the crash child's marker (its barrier was reached), then kills it.
fn kill_at_marker(mut child: std::process::Child, marker: &Path) {
    let deadline = Instant::now() + WAIT;
    while !marker.is_file() {
        assert!(
            child.try_wait().expect("poll the crash child").is_none(),
            "the crash child exited before its crash point"
        );
        assert!(Instant::now() < deadline, "the crash barrier timed out");
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().expect("kill the crash child");
    let _ = child.wait();
}

fn crash_run_identity(root: &Path) -> (RunId, LeaseId) {
    serde_json::from_slice(&fs::read(root.join("crash-run.json")).expect("run identity bytes"))
        .expect("run identity")
}

fn restart_ladder_host(root: &Path, registered: InstanceId) -> RuntimeHost {
    let (host_config, _) = ladder_setup(root);
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, state)),
    )
    .expect("restart after the crash")
}

fn run_count(events: &[PersistedEvent], run_id: &RunId, event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type && event.links().run_id() == Some(run_id))
        .count()
}

fn crash_root() -> (TempDir, InstanceId) {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    fs::write(
        root.path().join("instance.json"),
        serde_json::to_vec(&registered).expect("instance bytes"),
    )
    .expect("instance file");
    (root, registered)
}

/// Workflow #369 E3, first crash point: after the hand-off's `lease.released` and before
/// `policy.execution_recorded`. The restart settles the failed run exactly once.
#[test]
fn a_crash_after_the_hand_off_release_settles_the_run_exactly_once() {
    let (root, registered) = crash_root();
    let (mut child, marker) =
        spawn_ladder_crash_child(root.path(), "after_lease_release_before_policy_execution");
    let deadline = Instant::now() + WAIT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the crash child") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill the timed-out child");
            let _ = child.wait();
            panic!("the crash child timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(87), "the checkpoint ended the child");
    assert!(marker.is_file());
    let (run_id, lease_id): (RunId, LeaseId) = serde_json::from_slice(
        &fs::read(root.path().join("crash-run.json")).expect("run identity bytes"),
    )
    .expect("run identity");
    let prefix = closed_ledger_events(root.path());
    assert_eq!(run_count(&prefix, &run_id, EventType::LeaseReleased), 1);
    assert_eq!(
        run_count(&prefix, &run_id, EventType::PolicyExecutionRecorded),
        0
    );
    assert!(
        prefix.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Lease(LeasePayload::Transferred(transfer))
                if transfer.from_lease_id() == lease_id
        )),
        "the run's lease end was a hand-off"
    );
    let host = restart_ladder_host(root.path(), registered);
    let events = all_events(&host);
    for event_type in [
        EventType::LeaseReleased,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            run_count(&events, &run_id, event_type),
            1,
            "settled once: {event_type:?}"
        );
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");
}

/// The scheduled policy evaluation time of a later dispatch: a day and two hours on, past the
/// task's hourly interval and its day.
const LATER_DISPATCH_MS: u64 = POLICY_NOW_UNIX_MS + 26 * 3_600_000;

/// A crash at E3's second crash point (between the ladder claim's `scheduler.queued` and the
/// run's `lease.released`) on `root`, admitted at `eval_ms` when given: the run has its
/// `task.failed` and no release, and the hand-off's transfer was not written.
fn crash_inside_the_hand_off(root: &Path, eval_ms: Option<u64>) -> (RunId, LeaseId) {
    let environment = eval_ms
        .map(|at| vec![("ACTINGCOMMAND_LADDER_CRASH_EVAL_MS", at.to_string())])
        .unwrap_or_default();
    let (child, marker) = spawn_ladder_crash_child_with(
        root,
        "after_ladder_claim_queued_before_hand_off",
        &environment,
    );
    kill_at_marker(child, &marker);
    let (run_id, lease_id) = crash_run_identity(root);
    let prefix = closed_ledger_events(root);
    assert_eq!(run_count(&prefix, &run_id, EventType::TaskFailed), 1);
    assert_eq!(run_count(&prefix, &run_id, EventType::LeaseReleased), 0);
    assert!(!prefix.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Lease(LeasePayload::Transferred(transfer))
            if transfer.from_lease_id() == lease_id
    )));
    (run_id, lease_id)
}

/// The failure code of the run's one `policy.execution_recorded`; `None` when it succeeded.
fn recorded_failure_code(events: &[PersistedEvent], run_id: &RunId) -> Option<String> {
    let executions = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::PolicyExecutionRecorded
                && event.links().run_id() == Some(run_id)
        })
        .collect::<Vec<_>>();
    let [execution] = executions.as_slice() else {
        panic!("one execution of the run: {}", executions.len());
    };
    match execution.payload() {
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
            match payload.outcome() {
                PolicyExecutionOutcome::Failed { failure } => Some(failure.error_code.clone()),
                PolicyExecutionOutcome::Succeeded { .. } => None,
            }
        }
        _ => panic!("execution payload"),
    }
}

/// Coordinator ruling on #670 (review H-1): the restart recovered the run's missing release (one
/// run-linked `lease.released`, effect `not_performed`), recorded
/// `policy_settlement_release_recovered` once under the run's links, at Info, and settled the
/// run once as interrupted.
fn assert_settled_once_as_interrupted(events: &[PersistedEvent], run_id: &RunId) {
    let releases = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased && event.links().run_id() == Some(run_id)
        })
        .collect::<Vec<_>>();
    let [release] = releases.as_slice() else {
        panic!("one recovered release: {}", releases.len());
    };
    assert_eq!(
        release.payload().effect_disposition(),
        Some(EffectDisposition::NotPerformed)
    );
    assert_eq!(
        recorded_failure_code(events, run_id).as_deref(),
        Some("policy_settlement_interrupted")
    );
    assert_eq!(
        run_count(events, run_id, EventType::PolicyDispatchCompleted),
        1
    );
    let records = events
        .iter()
        .filter(|event| {
            event.links().run_id() == Some(run_id)
                && failure_message(event).is_some_and(|message| {
                    message.contains("code=policy_settlement_release_recovered")
                })
        })
        .collect::<Vec<_>>();
    let [record] = records.as_slice() else {
        panic!("one recovery record: {}", records.len());
    };
    assert_eq!(record.severity(), EventSeverity::Info);
}

/// Workflow #369 E3, second crash point (model C1; coordinator ruling on #670 review H-1). The
/// failed run has its `task.failed` and no release. The restart writes the missing run-linked
/// release, records `policy_settlement_release_recovered` and settles the run once as
/// interrupted, so its dispatch is closed: (a) a new dispatch on the same instance proceeds,
/// and the evaluation after it is not fatal.
#[test]
fn a_crash_inside_the_hand_off_is_settled_as_interrupted_and_a_new_dispatch_proceeds() {
    let (root, registered) = crash_root();
    let (run_id, _) = crash_inside_the_hand_off(root.path(), None);
    let host = restart_ladder_host(root.path(), registered);
    assert_settled_once_as_interrupted(&all_events(&host), &run_id);
    let (_, request) = ladder_setup(root.path());
    let context = admit_ladder_run_at(&host, &request, LATER_DISPATCH_MS);
    assert_ne!(context.run_id(), run_id);
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &policy_resources(),
        EvaluationTime {
            unix_ms: LATER_DISPATCH_MS + 60_000,
            monotonic_ms: LATER_DISPATCH_MS + 60_000,
        },
        9,
        PolicyTrigger::FactsChanged,
    )
    .expect("the evaluation after the new dispatch");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");
}

/// Coordinator ruling on #670 (review H-1), test (b): two runs on the same instance cut inside
/// their hand-offs, each followed by a restart. Each run is settled once as interrupted (the
/// second in a row holds the pair, P5), and the start after the second crash evaluates the
/// policy without a fatal error.
#[test]
fn two_cut_hand_offs_on_one_instance_are_each_settled_and_the_next_start_evaluates() {
    let (root, registered) = crash_root();
    let (first, _) = crash_inside_the_hand_off(root.path(), None);
    let host = restart_ladder_host(root.path(), registered);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close after the first crash");
    let (second, _) = crash_inside_the_hand_off(root.path(), Some(LATER_DISPATCH_MS));
    assert_ne!(first, second);
    let host = restart_ladder_host(root.path(), registered);
    let at = LATER_DISPATCH_MS + 26 * 3_600_000;
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &policy_resources(),
        EvaluationTime {
            unix_ms: at,
            monotonic_ms: at,
        },
        10,
        PolicyTrigger::Recovery,
    )
    .expect("the start's evaluation");
    let events = all_events(&host);
    assert_settled_once_as_interrupted(&events, &first);
    assert_settled_once_as_interrupted(&events, &second);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close after the second crash");
}

/// §7 row 4, E6 (review-670 M-1): a crash while a rung run is in flight. The next start settles
/// cleanly: the failed policy run, released by its hand-off, is settled once; the cut rung run
/// gets its recovered terminal; and retention closes the cut run at the epoch end (none of its
/// frames is still running, and a frame of it lies under its epoch-end point).
#[test]
fn a_crash_mid_rung_settles_cleanly_and_retention_closes_the_run_at_the_epoch_end() {
    let (root, registered) = crash_root();
    let (child, marker) = spawn_ladder_crash_child(root.path(), "mid_rung_run");
    kill_at_marker(child, &marker);
    let (run_id, _) = crash_run_identity(root.path());
    let prefix = closed_ledger_events(root.path());
    let cut = prefix
        .iter()
        .filter(|event| event.event_type() == EventType::TaskStarted)
        .filter_map(|event| event.links().run_id().copied())
        .filter(|run| *run != run_id && !run_ended(&prefix, run))
        .collect::<Vec<_>>();
    let [cut] = cut.as_slice() else {
        panic!("one rung run in flight at the crash: {cut:?}");
    };
    let host = restart_ladder_host(root.path(), registered);
    let events = all_events(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    for event_type in [
        EventType::LeaseReleased,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            run_count(&events, &run_id, event_type),
            1,
            "the failed run is settled once: {event_type:?}"
        );
    }
    assert!(run_ended(&events, cut), "the cut rung run has its terminal");
    host.close().expect("close the restarted host");
    let artifacts = ArtifactStore::open(root.path()).expect("open the artifact store");
    let view = GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root.path()),
        |reference| artifacts.verify_recovery_reference(reference).ok(),
    )
    .expect("read the closed ledger")
    .frame_retention_view(
        unix_ms_now().expect("wall clock"),
        actingcommand_ledger::FrameRetentionSwitches {
            dedup_error: true,
            dedup_lab: false,
        },
    )
    .expect("the frame retention view");
    let frames = view
        .frames
        .iter()
        .filter(|frame| frame.run_id.as_ref() == Some(cut))
        .collect::<Vec<_>>();
    assert!(
        frames
            .iter()
            .all(|frame| frame.class != actingcommand_ledger::FrameRetentionClass::Running),
        "{frames:?}"
    );
    if !frames.is_empty() {
        assert!(
            view.error_points
                .iter()
                .any(|point| point.epoch_end && point.run_id.as_ref() == Some(cut)),
            "the cut run's epoch-end point"
        );
    }
}

/// Review-670 L-3: the physical success branch. A physical scheduled run completes, and the
/// daemon ends after its release and before `policy.execution_recorded`; the restart measures
/// it from its scheduler `command.received` and settles it once as succeeded.
#[test]
fn a_physical_run_cut_after_its_success_is_settled_once_as_succeeded() {
    let (root, registered) = crash_root();
    let (mut child, marker) = spawn_ladder_crash_child_with(
        root.path(),
        "after_lease_release_before_policy_execution",
        &[("ACTINGCOMMAND_LADDER_CRASH_SUCCEED", "1".to_owned())],
    );
    let deadline = Instant::now() + WAIT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the crash child") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill the timed-out child");
            let _ = child.wait();
            panic!("the crash child timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(87), "the checkpoint ended the child");
    assert!(marker.is_file());
    let (run_id, _) = crash_run_identity(root.path());
    let prefix = closed_ledger_events(root.path());
    assert_eq!(run_count(&prefix, &run_id, EventType::TaskCompleted), 1);
    assert_eq!(run_count(&prefix, &run_id, EventType::LabRequest), 0);
    let host = restart_ladder_host(root.path(), registered);
    let events = all_events(&host);
    for event_type in [
        EventType::LeaseReleased,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            run_count(&events, &run_id, event_type),
            1,
            "settled once: {event_type:?}"
        );
    }
    assert_eq!(recorded_failure_code(&events, &run_id), None, "succeeded");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");
}
