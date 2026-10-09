// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #369 S2+S3b (model-369-queue v3.1 §7 row 4): the per-instance workers, a failed
//! policy run's hand-off to its ladder, the ladder's one continuous hold, and the startup
//! claim. The test fakes have no emulator control: the ladder's emulator-restart rung is
//! skipped, and an emulator start's startup claim is driven through the control's own tail
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
/// ladder runs rung 1 and rung 2 on its key; rung 3 is skipped (the fakes have no emulator
/// control).
fn ladder_fixture(capture_delay_ms: u64) -> LadderRun {
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
        event.event_type() != EventType::RuntimeFailed
            || !format!("{:?}", event.payload()).contains("lease_busy")
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
/// Warning and releases once, and the pause completes. (The R4 readiness wait checks the pause
/// at each poll too; the fakes have no emulator control to reach it.)
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
    let ledger = GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root.path()),
        |_| None,
    )
    .expect("read the closed ledger");
    let events = ledger.query(&EventQuery::default());
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
    drop(ledger);
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
        config(root).with_startup_packages(startup_packages),
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
            event.event_type() == EventType::RuntimeFailed
                && event.severity() == EventSeverity::Warning
                && format!("{:?}", event.payload()).contains("code=panic_caught")
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
    let panic = events
        .iter()
        .find(|event| format!("{:?}", event.payload()).contains("code=panic_caught"))
        .expect("panic record");
    let text = format!("{:?}", panic.payload());
    assert!(text.contains("startup_package"), "{text}");
    assert!(text.contains("boundary=instance_worker"), "{text}");
    assert!(text.contains("injected instance worker panic"), "{text}");
    assert!(
        count(&events, EventType::LeaseExpired) + count(&events, EventType::LeaseTransferred) >= 1,
        "the panicked payload's key lapsed and was handed on"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
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
    let context = admit_ladder_run(&host, &request);
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
    state.unknown_capture.store(true, Ordering::Release);
    let outcome = host.run_scheduled_contained_task(&context, &request);
    panic!(
        "the crash point did not stop the child: {:?}",
        outcome.err()
    );
}

/// Starts the crash child at `point` on `root`, with its marker path.
fn spawn_ladder_crash_child(root: &Path, point: &str) -> (std::process::Child, PathBuf) {
    let marker = root.join("ladder-crash-marker");
    let child = Command::new(std::env::current_exe().expect("test executable"))
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
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the crash child");
    (child, marker)
}

fn closed_ledger_events(root: &Path) -> Vec<PersistedEvent> {
    GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root),
        |_| None,
    )
    .expect("read the crashed ledger")
    .query(&EventQuery::default())
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

/// Workflow #369 E3, second crash point: between the ladder claim's `scheduler.queued` and the
/// run's `lease.released`. The failed run then has no release (C1). The outcome: the restart
/// recovers no release for it, settles it once from its committed `task.failed`, and starts
/// cleanly with the instance free (the ladder, memory only, is gone).
#[test]
fn a_crash_inside_the_hand_off_leaves_the_run_without_a_release_and_settles_it_once() {
    let (root, registered) = crash_root();
    let (mut child, marker) =
        spawn_ladder_crash_child(root.path(), "after_ladder_claim_queued_before_hand_off");
    let deadline = Instant::now() + WAIT;
    while !marker.is_file() {
        assert!(
            child.try_wait().expect("poll the crash child").is_none(),
            "the crash child exited before the hand-off"
        );
        assert!(Instant::now() < deadline, "the crash barrier timed out");
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().expect("kill the crash child");
    let _ = child.wait();
    let (run_id, lease_id): (RunId, LeaseId) = serde_json::from_slice(
        &fs::read(root.path().join("crash-run.json")).expect("run identity bytes"),
    )
    .expect("run identity");
    let prefix = closed_ledger_events(root.path());
    assert_eq!(run_count(&prefix, &run_id, EventType::TaskFailed), 1);
    assert_eq!(run_count(&prefix, &run_id, EventType::LeaseReleased), 0);
    assert!(prefix.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Scheduler(SchedulerPayload::Queued(queued))
            if queued.deadline_monotonic_ms() == u64::MAX
    )));
    assert!(!prefix.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Lease(LeasePayload::Transferred(transfer))
            if transfer.from_lease_id() == lease_id
    )));
    let host = restart_ladder_host(root.path(), registered);
    let events = all_events(&host);
    assert_eq!(
        run_count(&events, &run_id, EventType::LeaseReleased),
        0,
        "no release is recovered for the run"
    );
    assert_eq!(
        run_count(&events, &run_id, EventType::PolicyExecutionRecorded),
        1,
        "the restart settles the run once, from its task.failed"
    );
    let (holder, queued) = host
        .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
        .expect("claims after restart");
    assert!(holder.is_none() && queued.is_empty());
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");
}
